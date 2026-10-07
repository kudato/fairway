//! Build artifacts and macro registration exercised through consumer crates.

#![cfg(feature = "build")]

use std::{fs, path::Path, process::Command};

#[test]
fn build_and_embed_across_consumers_and_rebuilds() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path();
    let files = [
        ("Cargo.toml", include_str!("fixtures/Cargo.toml")),
        (
            "first/Cargo.toml",
            include_str!("fixtures/first/Cargo.toml"),
        ),
        ("first/build.rs", include_str!("fixtures/first/build.rs")),
        (
            "first/contents.txt",
            include_str!("fixtures/first/contents.txt"),
        ),
        (
            "first/src/lib.rs",
            include_str!("fixtures/first/src/lib.rs"),
        ),
        (
            "second/Cargo.toml",
            include_str!("fixtures/second/Cargo.toml"),
        ),
        ("second/build.rs", include_str!("fixtures/second/build.rs")),
        (
            "second/src/lib.rs",
            include_str!("fixtures/second/src/lib.rs"),
        ),
        ("app/Cargo.toml", include_str!("fixtures/app/Cargo.toml")),
        ("app/src/main.rs", include_str!("fixtures/app/src/main.rs")),
    ];
    for (name, contents) in files {
        let path = workspace.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let contents = if name == "Cargo.toml" {
            contents.replace(
                "path = \"../..\"",
                &format!("path = {:?}", env!("CARGO_MANIFEST_DIR")),
            )
        } else {
            contents.to_owned()
        };
        fs::write(path, contents).unwrap();
    }

    build(workspace, false);
    run(workspace, false, "builtin commit\n");

    fs::write(workspace.join("first/contents.txt"), "changed builtin\n").unwrap();
    build(workspace, false);
    run(workspace, false, "changed builtin\n");

    build(workspace, true);
    fs::remove_file(workspace.join("first/contents.txt")).unwrap();
    run(workspace, true, "changed builtin\n");
}

fn build(workspace: &Path, release: bool) {
    let mut command = Command::new(env!("CARGO"));
    command
        .current_dir(workspace)
        .args(["build", "--offline", "--quiet", "-p", "catalog-fixture-app"])
        .arg("--target-dir")
        .arg(workspace.join("target"));
    if release {
        command.arg("--release");
    }
    checked(&mut command);
}

fn run(workspace: &Path, release: bool, expected: &str) {
    let profile = if release { "release" } else { "debug" };
    let executable = workspace.join("target").join(profile).join(format!(
        "catalog-fixture-app{}",
        std::env::consts::EXE_SUFFIX
    ));
    checked(Command::new(executable).arg(expected));
}

fn checked(command: &mut Command) {
    let output = command.output().expect("start fixture command");
    assert!(
        output.status.success(),
        "{command:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
