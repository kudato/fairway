//! Builtin tables built and embedded through an owner's wrappers.

#![cfg(feature = "build")]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[test]
fn build_and_embed_across_consumers_and_rebuilds() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path();
    let files = [
        ("Cargo.toml", include_str!("fixtures/Cargo.toml")),
        (
            "owner/Cargo.toml",
            include_str!("fixtures/owner/Cargo.toml"),
        ),
        (
            "owner/src/lib.rs",
            include_str!("fixtures/owner/src/lib.rs"),
        ),
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
            "first/glossary.txt",
            include_str!("fixtures/first/glossary.txt"),
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
    assert_eq!(run(workspace, false), "builtin commit\ngit,rust\n");
    assert_eq!(
        glossary_files(workspace),
        ["0000000000000000", "0000000000000001", "entries.rs"]
    );

    // The shorter glossary leaves no files of the previous one.
    fs::write(workspace.join("first/contents.txt"), "changed builtin\n").unwrap();
    fs::write(
        workspace.join("first/glossary.txt"),
        "git: version control\n",
    )
    .unwrap();
    build(workspace, false);
    assert_eq!(run(workspace, false), "changed builtin\ngit\n");
    assert_eq!(
        glossary_files(workspace),
        ["0000000000000000", "entries.rs"]
    );

    build(workspace, true);
    fs::remove_file(workspace.join("first/contents.txt")).unwrap();
    fs::remove_file(workspace.join("first/glossary.txt")).unwrap();
    assert_eq!(run(workspace, true), "changed builtin\ngit\n");
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

fn run(workspace: &Path, release: bool) -> String {
    let profile = if release { "release" } else { "debug" };
    let executable = workspace.join("target").join(profile).join(format!(
        "catalog-fixture-app{}",
        std::env::consts::EXE_SUFFIX
    ));
    String::from_utf8(checked(&mut Command::new(executable)).stdout).unwrap()
}

/// Files of the glossary table in the first crate's debug `OUT_DIR`.
fn glossary_files(workspace: &Path) -> Vec<String> {
    let directories: Vec<_> = fs::read_dir(workspace.join("target/debug/build"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("out/fairway-catalog/glossary"))
        .filter(|directory| directory.is_dir())
        .collect();
    assert_eq!(directories.len(), 1, "{directories:?}");
    let mut files: Vec<_> = fs::read_dir(&directories[0])
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    files
}

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("start fixture command");
    assert!(
        output.status.success(),
        "{command:?} failed with {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
