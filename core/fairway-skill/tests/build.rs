//! Builtins checked by a plugin's build script, embedded, and overridden.

#![cfg(feature = "build")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        for (name, contents) in [
            ("Cargo.toml", include_str!("fixtures/Cargo.toml")),
            (
                "plugin/Cargo.toml",
                include_str!("fixtures/plugin/Cargo.toml"),
            ),
            ("plugin/build.rs", include_str!("fixtures/plugin/build.rs")),
            (
                "plugin/src/lib.rs",
                include_str!("fixtures/plugin/src/lib.rs"),
            ),
            (
                "plugin/skills/technical.md",
                include_str!("fixtures/plugin/skills/technical.md"),
            ),
            (
                "plugin/skills/references/ste.md",
                include_str!("fixtures/plugin/skills/references/ste.md"),
            ),
            (
                "plugin/skills/proofread.md",
                include_str!("fixtures/plugin/skills/proofread.md"),
            ),
            (
                "notes/Cargo.toml",
                include_str!("fixtures/notes/Cargo.toml"),
            ),
            (
                "notes/src/lib.rs",
                include_str!("fixtures/notes/src/lib.rs"),
            ),
            ("app/Cargo.toml", include_str!("fixtures/app/Cargo.toml")),
            ("app/src/main.rs", include_str!("fixtures/app/src/main.rs")),
        ] {
            let contents = if name == "Cargo.toml" {
                contents.replace(
                    "path = \"../..\"",
                    &format!("path = {:?}", env!("CARGO_MANIFEST_DIR")),
                )
            } else {
                contents.to_owned()
            };
            fixture.write(name, &contents);
        }
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.directory.path().join(relative)
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn build(&self, release: bool) -> Output {
        let mut command = Command::new(env!("CARGO"));
        command
            .current_dir(self.directory.path())
            .args(["build", "--offline", "--quiet", "-p", "skill-fixture-app"])
            .arg("--target-dir")
            .arg(self.path("target"));
        if release {
            command.arg("--release");
        }
        command.output().unwrap()
    }

    /// Runs the application with the Fairway directory `home`.
    fn run(&self, release: bool, home: &Path, args: &[&str]) -> Output {
        let profile = if release { "release" } else { "debug" };
        let executable = self
            .path("target")
            .join(profile)
            .join(format!("skill-fixture-app{}", std::env::consts::EXE_SUFFIX));
        Command::new(executable)
            .args(args)
            .env("FAIRWAY_HOME", home)
            .output()
            .unwrap()
    }
}

fn successful(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn rejected(output: Output, diagnostics: &[&str]) {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "expected {diagnostics:?}:\n{text}"
    );
    for diagnostic in diagnostics {
        assert!(
            text.contains(diagnostic),
            "expected {diagnostic:?}:\n{text}"
        );
    }
}

const BUILTINS: &str = "# Technical writing

Follow these rules: Use short sentences.

Write technical documentation.
Proofread the text.
Proofread a text.
";

#[test]
fn builtins_are_embedded_checked_and_overridden() {
    let fixture = Fixture::new();
    let home = fixture.path("home");
    fs::create_dir_all(&home).unwrap();

    for release in [false, true] {
        successful(fixture.build(release));
        assert_eq!(successful(fixture.run(release, &home, &[])), BUILTINS);
    }

    // Overrides replace a builtin and add a skill in a namespace no crate
    // declares; a relative path starts at `skills` in the Fairway directory.
    fixture.write(
        "home/skills/technical.md",
        "---\ndescription: My technical writing.\n---\nMine: {{ ste }}\n",
    );
    fixture.write("notes.md", "---\ndescription: Notes.\n---\nMy notes.\n");
    let notes = fixture.path("notes.md");
    fixture.write(
        "home/config.toml",
        &format!(
            "[skill]\n\"skill/writing/technical\" = \"technical.md\"\n\
             \"skill/my/notes\" = {:?}\n",
            notes.to_str().unwrap()
        ),
    );
    assert_eq!(
        successful(fixture.run(false, &home, &["skill/my/notes"])),
        "Mine: Use short sentences.\n\nMy technical writing.\nProofread the text.\n\
         Proofread a text.\nMy notes.\n"
    );
    fs::remove_file(home.join("config.toml")).unwrap();

    // A changed file is embedded again.
    fixture.write("plugin/skills/references/ste.md", "Use the active voice.\n");
    successful(fixture.build(false));
    assert_eq!(
        successful(fixture.run(false, &home, &[])),
        BUILTINS.replace("Use short sentences.", "Use the active voice.")
    );

    // The build script reports every invalid file with its address and path.
    fixture.write(
        "plugin/skills/proofread.md",
        "---\nname: proofread\ndescription: Proofread a text.\n---\n",
    );
    fixture.write(
        "plugin/skills/references/ste.md",
        "---\ndescription: STE\n---\n",
    );
    rejected(
        fixture.build(false),
        &[
            "\"skill/editing/proofread\" (skills/proofread.md): has invalid frontmatter",
            "\"reference/writing/ste\" (skills/references/ste.md): \
             starts with frontmatter, which a reference cannot have",
        ],
    );
    fixture.write(
        "plugin/skills/proofread.md",
        include_str!("fixtures/plugin/skills/proofread.md"),
    );
    fixture.write(
        "plugin/skills/references/ste.md",
        include_str!("fixtures/plugin/skills/references/ste.md"),
    );

    // Namespaces of builtins belong to the crate that embeds them, and every
    // namespace is declared once.
    fixture.write(
        "plugin/src/lib.rs",
        "skill::namespace!(\"writing\");\nskill::embed!();\n",
    );
    successful(fixture.build(false));
    rejected(
        fixture.run(false, &home, &[]),
        &[
            "builtins in skill namespace \"editing\" that crate `skill_fixture_plugin` \
           does not declare with `namespace!`: skill_fixture_plugin (plugin/src/lib.rs:2)",
        ],
    );
    fixture.write(
        "plugin/src/lib.rs",
        include_str!("fixtures/plugin/src/lib.rs"),
    );
    fixture.write("notes/src/lib.rs", "skill::namespace!(\"writing\");\n");
    successful(fixture.build(false));
    rejected(
        fixture.run(false, &home, &[]),
        &["duplicate skill namespace \"writing\": \
           skill_fixture_notes (notes/src/lib.rs:1) and \
           skill_fixture_plugin (plugin/src/lib.rs:1)"],
    );
}
