//! Build real plugin crates and link them into the actual Fairway entry point.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Fixture {
    directory: tempfile::TempDir,
    target: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let fairway = Path::new(env!("CARGO_MANIFEST_DIR"));
        let core = fairway.parent().unwrap();
        let cli = core.join("fairway-cli");
        let target = core.parent().unwrap().join("target");
        fs::create_dir_all(&target).unwrap();
        let fixture = Self {
            directory: tempfile::tempdir_in(&target).unwrap(),
            target: target.join("cli-build-tests"),
        };
        fixture.write(
            "Cargo.toml",
            &format!(
                r#"
[workspace]
members = ["app", "plugin-a", "plugin-b"]
resolver = "3"

[workspace.dependencies]
fw = {{ package = "fairway-cli", path = {cli:?} }}
clap = {{ version = "4", features = ["derive"] }}
anyhow = "1"

[profile.release]
lto = "thin"
"#
            ),
        );
        for name in ["plugin-a", "plugin-b"] {
            fixture.write(
                &format!("{name}/Cargo.toml"),
                &format!(
                    r#"
[package]
name = "{name}"
version = "0.0.0"
edition = "2024"

[dependencies]
fw.workspace = true
clap.workspace = true
anyhow.workspace = true
"#
                ),
            );
        }
        fixture.write(
            "app/Cargo.toml",
            &format!(
                r#"
[package]
name = "fixture-app"
version = "0.0.2"
edition = "2024"

[dependencies]
fairway-cli = {{ path = {cli:?} }}
clap.workspace = true
anyhow.workspace = true
tokio = {{ version = "1.26", features = ["rt", "rt-multi-thread", "signal", "sync", "time", "macros"] }}
tokio-util = "0.7"
plugin-a = {{ path = "../plugin-a" }}
plugin-b = {{ path = "../plugin-b" }}
"#
            ),
        );
        let main = fs::read_to_string(fairway.join("src/main.rs")).unwrap();
        fixture.write(
            "app/src/main.rs",
            &format!("{main}\nuse plugin_a as _;\nuse plugin_b as _;\n"),
        );
        for module in ["app.rs", "signal.rs"] {
            let source = fs::read_to_string(fairway.join("src").join(module)).unwrap();
            fixture.write(&format!("app/src/{module}"), &source);
        }
        // Keep the fixture's dependencies aligned with the workspace.
        fs::copy(
            core.parent().unwrap().join("Cargo.lock"),
            fixture.directory.path().join("Cargo.lock"),
        )
        .unwrap();
        fixture
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.directory.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn cargo(&self, action: &str, release: bool) -> Command {
        let mut command = Command::new(env!("CARGO"));
        command
            .current_dir(self.directory.path())
            .args([
                action,
                "--offline",
                "--quiet",
                "-p",
                "fixture-app",
                "--target-dir",
            ])
            .arg(&self.target);
        if release {
            command.arg("--release");
        }
        command
    }

    fn build(&self, release: bool) -> Output {
        self.cargo("build", release).output().unwrap()
    }

    fn check(&self, release: bool) -> Output {
        let output = self
            .cargo("test", release)
            .args(["--locked", "--bin", "fixture-app", "cli_registry_is_valid"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
            "the application registry test did not run: {output:?}"
        );
        output
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(
            self.target
                .join("debug")
                .join(format!("fixture-app{}", std::env::consts::EXE_SUFFIX)),
        )
        .args(args)
        .output()
        .unwrap()
    }
}

fn successful(output: Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn rejected(output: Output, diagnostic: &str) {
    assert!(
        !output.status.success(),
        "invalid declarations were accepted: expected {diagnostic}"
    );
    let error = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        error.contains(diagnostic),
        "expected {diagnostic:?}:\n{error}"
    );
}

#[test]
fn plugins_register_and_application_tests_reject_conflicts() {
    let fixture = Fixture::new();
    let a = "#![deny(warnings)]\nfw::namespace!(CLI, \"shared\", \"First plugin\");\n";
    let b = "#![deny(warnings)]\nfw::namespace!(CLI, \"second\", \"Second plugin\");\n";
    fixture.write("plugin-a/src/lib.rs", a);
    fixture.write("plugin-b/src/lib.rs", b);
    successful(fixture.build(false));
    let help = fixture.run(&["--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("First plugin"), "{help}");
    assert!(help.contains("Second plugin"), "{help}");
    successful(fixture.check(false));

    // Type and handler-mode errors remain compile errors.
    let cases = [
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Own", run);
fw::command!(CLI, "named", "Named", run);
"#,
            "conflicting implementations",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "named", "Named", run);
fw::command!(CLI, "Own", run);
"#,
            "conflicting implementations",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "First", run);
fw::command!(CLI, "Second", run);
"#,
            "conflicting implementations",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run(_: String) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Bad arguments", run);
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
#[derive(clap::Args)] struct Args {}
async fn run(_: fw::Shutdown, _: Args) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Wrong argument order", run);
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() {}
fw::command!(CLI, "Wrong result", run);
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
fn run() -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Synchronous handler", run);
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() -> anyhow::Result<()> {
    let local = std::rc::Rc::new(1);
    std::future::pending::<()>().await;
    drop(local);
    Ok(())
}
fw::command!(CLI, "Non-Send future", run);
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
#[derive(clap::Args)] struct Args { #[arg(long)] workers: String }
async fn run(_: Args) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Wrong worker count", run, workers = |args: &Args| args.workers.clone());
"#,
            "usize",
        ),
        (
            "fw::namespace!(CLI, \"\", \"Empty\");",
            "a CLI name cannot be empty",
        ),
        (
            "fw::namespace!(CLI, \"two words\", \"Invalid name\");",
            "a CLI name cannot contain whitespace or control characters",
        ),
    ];
    for (source, diagnostic) in cases {
        fixture.write("plugin-b/src/lib.rs", source);
        rejected(fixture.build(false), diagnostic);
    }

    // Names and clap definitions can compile, but must fail the app's test.
    let cases = [
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
async fn run() -> anyhow::Result<()> { Ok(()) }
mod first {
    use super::CLI;
    fw::command!(CLI, "duplicate", "First", super::run);
}
mod second {
    use super::CLI as ALIAS;
    fw::command!(ALIAS, r"duplicate", "Second", super::run);
}
"#,
            "command \"sample duplicate\" is declared twice",
        ),
        (
            r#"
fw::namespace!(CLI, "sample", "Sample");
#[derive(clap::Args)]
struct Args {
    #[arg(long = "value")] first: String,
    #[arg(long = "value")] second: String,
}
async fn run(_: Args) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "run", "Bad options", run);
"#,
            "Long option names must be unique",
        ),
    ];
    for (source, diagnostic) in cases {
        fixture.write("plugin-b/src/lib.rs", source);
        successful(fixture.build(false));
        rejected(fixture.check(false), diagnostic);
    }

    fixture.write(
        "plugin-b/src/lib.rs",
        r#"
#![deny(warnings)]
fw::namespace!(CLI, "sample", "Sample");
fw::namespace!(OTHER, "another", "Another namespace");
#[derive(clap::Args)]
struct Args { #[arg(long)] workers: usize }
async fn run(_: Args) -> anyhow::Result<()> { panic!("handler must not run in validation") }
fw::command!(CLI, "run", "Run", run, workers = |_: &Args| -> usize {
    panic!("worker selector must not run in validation")
});
fw::command!(OTHER, "run", "Same command name in another namespace", run);
"#,
    );
    successful(fixture.build(false));
    successful(fixture.check(false));

    fixture.write("plugin-a/src/lib.rs", a);
    fixture.write("plugin-b/src/lib.rs", b);
    successful(fixture.build(true));
    successful(fixture.check(true));

    // Cross-crate duplicates compile in both profiles, but the shared test
    // must reject them and identify both original declarations.
    fixture.write("plugin-b/src/lib.rs", a);
    for release in [false, true] {
        successful(fixture.build(release));
        let output = fixture.check(release);
        let diagnostic = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
        assert!(
            diagnostic.contains("plugin_a (plugin-a/src/lib.rs:2)"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("plugin_b (plugin-b/src/lib.rs:2)"),
            "{diagnostic}"
        );
        rejected(output, "namespace \"shared\" is declared twice");
    }
}
