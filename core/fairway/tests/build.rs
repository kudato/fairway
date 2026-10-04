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
        let filesystem = core.join("fairway-fs");
        let config = core.join("fairway-config");
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
cfg = {{ package = "fairway-config", path = {config:?} }}
serde = {{ version = "1", features = ["derive"] }}
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
cfg.workspace = true
serde.workspace = true
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
fairway-fs = {{ path = {filesystem:?} }}
fairway-config = {{ path = {config:?} }}
clap.workspace = true
anyhow.workspace = true
tokio = {{ version = "1.26", features = ["rt", "rt-multi-thread", "signal", "sync", "time", "macros"] }}
tokio-util = "0.7"
plugin-a = {{ path = "../plugin-a" }}
plugin-b = {{ path = "../plugin-b" }}
"#
            ),
        );
        for module in ["main.rs", "app.rs", "signal.rs"] {
            let source = fs::read_to_string(fairway.join("src").join(module)).unwrap();
            fixture.write(&format!("app/src/{module}"), &source);
        }
        // The fixture's plugins replace the ones linked into the application.
        fixture.write(
            "app/src/plugins.rs",
            "use plugin_a as _;\nuse plugin_b as _;\n",
        );
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
        self.check_registry(release, "cli_registry_is_valid")
    }

    fn check_registry(&self, release: bool, test: &str) -> Output {
        let output = self
            .cargo("test", release)
            .args(["--locked", "--bin", "fixture-app", test])
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
        .env("FAIRWAY_HOME", self.directory.path().join("home"))
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
    let a = r#"#![deny(warnings)]
fw::namespace!(CLI, "shared");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { panic!("validation must not run handlers") }
fw::command!(CLI, "First plugin", run, long_about = "First plugin");
fw::command!(CLI, "run", "Run", run, long_about = "Run");
"#;
    let b = r#"#![deny(warnings)]
fw::namespace!(CLI, "second");
use fw::help as usage;
fw::command!(CLI, "Second plugin", usage, long_about = "Show the second plugin commands.");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { panic!("validation must not run handlers") }
fw::command!(CLI, "run", "Run", run, long_about = "Run");
"#;
    fixture.write("plugin-a/src/lib.rs", a);
    fixture.write("plugin-b/src/lib.rs", b);
    successful(fixture.build(false));
    let help = fixture.run(&["--help"]);
    assert_eq!(help.status.code(), Some(0), "{help:?}");
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("First plugin"), "{help}");
    assert!(help.contains("Second plugin"), "{help}");
    successful(fixture.check(false));
    let namespace_help = fixture.run(&["second"]);
    assert_eq!(namespace_help.status.code(), Some(0), "{namespace_help:?}");
    assert!(namespace_help.stderr.is_empty(), "{namespace_help:?}");
    assert!(
        String::from_utf8_lossy(&namespace_help.stdout)
            .contains("Show the second plugin commands.")
    );
    assert_eq!(
        namespace_help.stdout,
        fixture.run(&["second", "--help"]).stdout
    );

    fixture.write(
        "plugin-b/src/lib.rs",
        r#"#![deny(warnings)]
fw::namespace!(CLI, "sample");
#[derive(clap::Args)]
struct Own {
    value: String,
    #[arg(long)] port: u16,
    #[arg(long, default_value_t = 2)] workers: usize,
}
#[derive(clap::Args)]
struct Status {
    #[arg(long)] format: String,
    #[arg(long, default_value_t = 3)] workers: usize,
}
async fn help(args: Own, _: fw::Shutdown) -> anyhow::Result<()> {
    println!("OWN:{}:{}", args.value, args.port);
    Ok(())
}
async fn status(args: Status, _: fw::Shutdown) -> anyhow::Result<()> {
    println!("STATUS:{}", args.format);
    Ok(())
}
fw::command!(CLI, "status", "Status", status, long_about = "Show status.", workers = |args: &Status| {
    println!("STATUS_WORKERS:{}", args.workers);
    args.workers
});
fw::command!(CLI, "Sample", help, long_about = "Run the sample.", workers = |args: &Own| {
    println!("OWN_WORKERS:{}", args.workers);
    args.workers
});
"#,
    );
    successful(fixture.build(false));
    successful(fixture.check(false));
    for (args, expected) in [
        (
            vec!["sample", "value", "--port", "8080"],
            "OWN_WORKERS:2\nOWN:value:8080\n",
        ),
        (
            vec!["sample", "status", "--format", "short", "--workers", "4"],
            "STATUS_WORKERS:4\nSTATUS:short\n",
        ),
        (
            vec!["sample", "--port", "8080", "--", "status"],
            "OWN_WORKERS:2\nOWN:status:8080\n",
        ),
    ] {
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        assert!(output.stderr.is_empty(), "{args:?}: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
    for (args, expected, absent) in [
        (vec!["sample", "--help"], "--port", "--format"),
        (vec!["sample", "status", "--help"], "--format", "--port"),
    ] {
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(0), "{args:?}: {output:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(help.contains("status"), "{help}");
        assert!(help.contains(expected), "{help}");
        assert!(!help.contains(absent), "{help}");
        assert!(!help.contains("WORKERS:"), "{help}");
    }
    for args in [
        vec!["sample"],
        vec!["sample", "value"],
        vec!["sample", "status"],
        vec!["sample", "--port", "8080", "status", "--format", "short"],
        vec!["sample", "status", "--format", "short", "--port", "8080"],
    ] {
        let output = fixture.run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
    }

    fixture.write(
        "plugin-b/src/lib.rs",
        "#![deny(warnings)]\nfw::namespace!(CLI, \"empty\");\n",
    );
    for release in [false, true] {
        successful(fixture.build(release));
        let output = fixture.check(release);
        let diagnostic = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
        rejected(output, "namespace \"empty\" has no commands");
        assert!(
            diagnostic.contains("plugin_b (plugin-b/src/lib.rs:2)"),
            "{diagnostic}"
        );
    }

    // Type and macro errors need only one build profile. Registration checks
    // below still exercise debug and release with thin LTO.
    let cases = [
        (
            r#"
fw::namespace!(CLI, "sample");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "First", run, long_about = "First");
fw::command!(CLI, "Second", run, long_about = "Second");
"#,
            "conflicting implementations",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
async fn run(_: String, _: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Bad arguments", run, long_about = "Bad arguments");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
async fn run() -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Missing shutdown", run, long_about = "Missing shutdown");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
#[derive(clap::Args)] struct Args {}
async fn run(_: Args) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "run", "Missing shutdown after arguments", run, long_about = "Missing shutdown after arguments");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
#[derive(clap::Args)] struct Args {}
async fn run(_: fw::Shutdown, _: Args) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Wrong argument order", run, long_about = "Wrong argument order");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
async fn run(_: fw::Shutdown) {}
fw::command!(CLI, "Wrong result", run, long_about = "Wrong result");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Synchronous handler", run, long_about = "Synchronous handler");
"#,
            "Handler",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
#[derive(clap::Args)] struct Args { #[arg(long)] workers: String }
async fn run(_: Args, _: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "Wrong worker count", run, long_about = "Wrong worker count", workers = |args: &Args| args.workers.clone());
"#,
            "usize",
        ),
        ("fw::namespace!(CLI, \"\");", "a CLI name cannot be empty"),
        (
            "fw::namespace!(CLI, \"two words\");",
            "a CLI name cannot contain whitespace or control characters",
        ),
    ];
    for (source, diagnostic) in cases {
        fixture.write("plugin-b/src/lib.rs", source);
        rejected(fixture.build(false), diagnostic);
    }

    let cases = [
        (
            r#"fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help);"#,
            "unexpected end of macro invocation",
        ),
        (
            r#"fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample commands");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "run", "Run", run);"#,
            "unexpected end of macro invocation",
        ),
        (
            r#"fw::namespace!(CLI, "sample");
fw::command!(CLI, fw::help, long_about = "Sample commands");"#,
            "no rules expected",
        ),
        (
            r#"fw::namespace!(CLI, "sample");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "run", "Run", run, long_about = "Run the command");"#,
            "NamespaceCommand",
        ),
        (
            r#"fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample commands", workers = 2);"#,
            "fairway_cli::help does not use workers",
        ),
        (
            r#"fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample commands");
fw::command!(CLI, "nested", "Nested", fw::help, long_about = "Nested commands");"#,
            "fairway_cli::help can only handle a namespace",
        ),
    ];
    for (source, diagnostic) in cases {
        fixture.write("plugin-b/src/lib.rs", source);
        rejected(fixture.build(false), diagnostic);
    }

    for name in [
        r"two\u{a0}words",
        r"two\u{3000}words",
        r"two\u{85}words",
        r"two\u{9f}words",
    ] {
        let namespace = format!(r#"fw::namespace!(CLI, "{name}");"#);
        let command = format!(
            r#"
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> {{ Ok(()) }}
fw::command!(CLI, "{name}", "Invalid subcommand", run, long_about = "Invalid subcommand");
"#
        );
        for (source, declaration) in [(namespace, "fw::namespace!"), (command, "fw::command!")] {
            fixture.write("plugin-b/src/lib.rs", &source);
            let output = fixture.build(false);
            let error = String::from_utf8_lossy(&output.stderr).replace('\\', "/");
            rejected(
                output,
                "a CLI name cannot contain whitespace or control characters",
            );
            assert!(error.contains("plugin-b/src/lib.rs:"), "{name}: {error}");
            assert!(error.contains(declaration), "{name}: {error}");
        }
    }

    // Check the name's value, including raw and escaped string literals,
    // and point to the declaration.
    let cases = [
        (r#"fw::namespace!(CLI, r"help");"#, "fw::namespace!"),
        (
            r#"
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "h\u{65}lp", "Reserved subcommand", run, long_about = "Reserved subcommand");
"#,
            "fw::command!",
        ),
    ];
    for (source, declaration) in cases {
        fixture.write("plugin-b/src/lib.rs", source);
        let output = fixture.build(false);
        let error = String::from_utf8_lossy(&output.stderr).replace('\\', "/");
        assert!(error.contains("plugin-b/src/lib.rs:"), "{error}");
        assert!(error.contains(declaration), "{error}");
        rejected(output, "the CLI name `help` is reserved for built-in help");
    }

    // Names and clap definitions can compile, but must fail the app's test.
    let cases = [
        (
            r#"
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
async fn run(_: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
mod first {
    use super::CLI;
    fw::command!(CLI, "duplicate", "First", super::run, long_about = "First");
}
mod second {
    use super::CLI as ALIAS;
    fw::command!(ALIAS, r"duplicate", "Second", super::run, long_about = "Second");
}
"#,
            "command \"sample duplicate\" is declared twice",
        ),
        (
            r#"
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
#[derive(clap::Args)]
struct Args {
    #[arg(long = "value")] first: String,
    #[arg(long = "value")] second: String,
}
async fn run(_: Args, _: fw::Shutdown) -> anyhow::Result<()> { Ok(()) }
fw::command!(CLI, "run", "Bad options", run, long_about = "Bad options");
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
fw::namespace!(CLI, "sample");
fw::command!(CLI, "Sample", fw::help, long_about = "Sample");
fw::namespace!(OTHER, "another");
fw::command!(OTHER, "Another namespace", fw::help, long_about = "Another namespace");
fw::namespace!(HELPERS, "help-tools");
fw::command!(HELPERS, "help", fw::help, long_about = "help");
fw::namespace!(UNICODE, "café");
fw::command!(UNICODE, "Unicode namespace", fw::help, long_about = "Unicode namespace");
#[derive(clap::Args)]
struct Args { #[arg(long)] workers: usize }
async fn run(_: Args, _: fw::Shutdown) -> anyhow::Result<()> { panic!("handler must not run in validation") }
fw::command!(CLI, "run", "Run", run, long_about = "Run", workers = |_: &Args| -> usize {
    panic!("worker selector must not run in validation")
});
fw::command!(OTHER, "run", "Same command name in another namespace", run, long_about = "Same command name in another namespace");
fw::command!(HELPERS, "helpful", "help", run, long_about = "help");
fw::command!(UNICODE, "данные-猫-🌍", "Unicode subcommand", run, long_about = "Unicode subcommand");
"#,
    );
    successful(fixture.build(false));
    successful(fixture.check(false));
    successful(fixture.run(&["café", "данные-猫-🌍", "--help"]));

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

    // Configuration uses the same application-level check, with a renamed
    // dependency and two namespaces of one settings type in a real plugin.
    let config_a = r#"#![deny(warnings)]
use serde::Deserialize;
#[derive(Default, Deserialize)]
struct Settings {}
cfg::namespace!(FIRST: Settings, "first");
cfg::namespace!(SECOND: Settings, "second");
cfg::namespace!(UNICODE: Settings, "настройки.猫.🌍");
"#;
    let config_b = r#"#![deny(warnings)]
#[derive(Default, serde::Deserialize)]
struct Settings {}
cfg::namespace!(THIRD: Settings, "third");
"#;
    fixture.write("plugin-a/src/lib.rs", config_a);
    fixture.write("plugin-b/src/lib.rs", config_b);
    for release in [false, true] {
        successful(fixture.build(release));
        successful(fixture.check_registry(release, "config_registry_is_valid"));
    }
    for (source, diagnostic) in [
        (
            "#[derive(Default)] struct Settings; cfg::namespace!(CONFIG: Settings, \"config\");",
            "Deserialize",
        ),
        (
            "#[derive(serde::Deserialize)] struct Settings {} cfg::namespace!(CONFIG: Settings, \"config\");",
            "Default",
        ),
        (
            "#[derive(Default, serde::Deserialize)] struct Settings {} cfg::namespace!(CONFIG: Settings, \"\");",
            "cannot be empty",
        ),
        (
            "#[derive(Default, serde::Deserialize)] struct Settings {} cfg::namespace!(CONFIG: Settings, \"two words\");",
            "cannot contain whitespace",
        ),
        (
            "#[derive(Default, serde::Deserialize)] struct Settings {} cfg::namespace!(CONFIG: Settings, \"two\\u{3000}words\");",
            "cannot contain whitespace",
        ),
        (
            "#[derive(Default, serde::Deserialize)] struct Settings {} cfg::namespace!(CONFIG: Settings, \"two\\u{85}words\");",
            "cannot contain whitespace",
        ),
        (
            "#[derive(Default, serde::Deserialize)] struct Settings { marker: std::marker::PhantomData<std::rc::Rc<()>> } cfg::namespace!(CONFIG: Settings, \"config\");",
            "Send",
        ),
    ] {
        fixture.write("plugin-b/src/lib.rs", source);
        rejected(fixture.build(false), diagnostic);
    }
    fixture.write(
        "plugin-b/src/lib.rs",
        &config_b.replace("\"third\"", "\"first\""),
    );
    for release in [false, true] {
        successful(fixture.build(release));
        let output = fixture.check_registry(release, "config_registry_is_valid");
        let diagnostic = String::from_utf8_lossy(&output.stdout).replace('\\', "/");
        assert!(
            diagnostic.contains("plugin_a (plugin-a/src/lib.rs:5)"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("plugin_b (plugin-b/src/lib.rs:4)"),
            "{diagnostic}"
        );
        rejected(
            output,
            "configuration namespace \"first\" is declared twice",
        );
    }
}
