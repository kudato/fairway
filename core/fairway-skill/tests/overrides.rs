//! Overrides from the configuration, each scenario in its own process: the
//! Fairway directory, the configuration, and initialization are fixed once
//! per process.

use std::{collections::BTreeMap, error::Error as _, fs, path::Path, process::Command};

use fairway_skill::ErrorKind;

const MODE: &str = "FAIRWAY_SKILL_TEST_MODE";

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

/// Prepares the Fairway directory of a scenario.
fn prepare(mode: &str, home: &Path, outside: &Path) {
    match mode {
        "uninitialized" | "empty" => {}
        "overrides" => {
            write(
                &home.join("config.toml"),
                &format!(
                    r#"[skill]
skills_dir = "custom"
"skill/writing/technical" = "technical.md"
"reference/writing/ste" = "../shared/ste.md"
"skill/мои/заметки" = {:?}
"#,
                    outside.join("notes.md").to_str().unwrap()
                ),
            );
            write(
                &home.join("custom/technical.md"),
                "---\ndescription: Technical writing.\n---\n# Technical\n\n{{ ste }}",
            );
            write(&home.join("shared/ste.md"), "Write for {{ reader }}.\n");
            write(
                &outside.join("notes.md"),
                "---\ndescription: |\n  My notes.\n---\nLine 4\n{{ missing }}\n",
            );
        }
        "default" => {
            write(
                &home.join("config.toml"),
                "[skill]\n\"skill/writing/technical\" = \"my/technical.md\"\n",
            );
            write(
                &home.join("skills/my/technical.md"),
                "---\ndescription: From skills.\n---\nBody\n",
            );
        }
        "invalid" => {
            write(
                &home.join("config.toml"),
                r#"[skill]
"skill/writing/missing" = "missing.md"
"skill/writing/plain" = "plain.md"
"skill/writing/valid" = "valid.md"
"reference/writing/front" = "front.md"
"reference/writing/syntax" = "syntax.md"
"#,
            );
            write(&home.join("skills/plain.md"), "No frontmatter.\n");
            write(&home.join("skills/valid.md"), "---\ndescription: x\n---\n");
            write(&home.join("skills/front.md"), "---\ndescription: x\n---\n");
            write(&home.join("skills/syntax.md"), "Text\n{% if %}\n");
        }
        "settings" => {
            write(
                &home.join("config.toml"),
                r#"[skill]
skils_dir = "typo"
"skill/writing" = "two-parts.md"
"skill/writing/empty" = ""
"#,
            );
        }
        _ => unreachable!("unknown mode {mode}"),
    }
}

/// Runs a scenario in the child process.
async fn scenario(mode: &str) {
    match mode {
        "uninitialized" => {
            for error in [
                fairway_skill::skill("skill/writing/technical", ()).unwrap_err(),
                fairway_skill::reference("reference/writing/ste", ()).unwrap_err(),
                fairway_skill::description("skill/writing/technical").unwrap_err(),
            ] {
                assert_eq!(error.kind(), ErrorKind::NotInitialized);
                assert_eq!(error.to_string(), "skills are not initialized successfully");
            }
        }
        "empty" => {
            fairway_skill::initialize().await.unwrap();
            let error = fairway_skill::skill("skill/writing/technical", ()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::NotFound);
            assert_eq!(error.to_string(), "no skill at \"skill/writing/technical\"");
        }
        "overrides" => {
            fairway_skill::initialize().await.unwrap();
            // Repeated calls return the first result.
            fairway_skill::initialize().await.unwrap();

            let ste = fairway_skill::reference(
                "reference/writing/ste",
                BTreeMap::from([("reader", "engineers")]),
            )
            .unwrap();
            assert_eq!(ste.as_ref(), "Write for engineers.\n");
            let technical = fairway_skill::skill(
                "skill/writing/technical",
                BTreeMap::from([("ste", ste.as_ref())]),
            )
            .unwrap();
            assert_eq!(technical.as_ref(), "# Technical\n\nWrite for engineers.\n");
            assert_eq!(
                fairway_skill::description("skill/writing/technical").unwrap(),
                "Technical writing."
            );
            assert_eq!(
                fairway_skill::description("skill/мои/заметки").unwrap(),
                "My notes.\n"
            );

            // A missing variable fails the render, at its line in the file.
            let error = fairway_skill::skill("skill/мои/заметки", ()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Render);
            assert_eq!(
                error.to_string(),
                "cannot render skill \"skill/мои/заметки\""
            );
            let source = error
                .source()
                .unwrap()
                .downcast_ref::<fairway_template::Error>()
                .unwrap();
            assert_eq!(source.line(), Some(6), "{source}");
            let error = fairway_skill::reference("reference/writing/ste", ()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Render);

            for (error, message) in [
                (
                    fairway_skill::skill("reference/writing/ste", ()).unwrap_err(),
                    "address \"reference/writing/ste\" is not a skill address",
                ),
                (
                    fairway_skill::description("reference/writing/ste").unwrap_err(),
                    "address \"reference/writing/ste\" is not a skill address",
                ),
                (
                    fairway_skill::reference("skill/writing/technical", ()).unwrap_err(),
                    "address \"skill/writing/technical\" is not a reference address",
                ),
                (
                    fairway_skill::skill("skill/writing", ()).unwrap_err(),
                    "address \"skill/writing\" must have three parts: \
                     `skill/<namespace>/<name>` or `reference/<namespace>/<name>`",
                ),
            ] {
                assert_eq!(error.kind(), ErrorKind::InvalidAddress);
                assert_eq!(error.to_string(), message);
            }
            let error = fairway_skill::reference("reference/writing/other", ()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::NotFound);
            assert_eq!(
                error.to_string(),
                "no reference at \"reference/writing/other\""
            );
        }
        "default" => {
            fairway_skill::initialize().await.unwrap();
            assert_eq!(
                fairway_skill::skill("skill/writing/technical", ())
                    .unwrap()
                    .as_ref(),
                "Body\n"
            );
        }
        "invalid" => {
            let home = fairway_fs::home().await.unwrap();
            let error = fairway_skill::initialize().await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Initialize);
            let message = error.to_string();
            let skills = home.join("skills");
            let expected = [
                "invalid skill overrides:".to_owned(),
                format!(
                    "  \"reference/writing/front\" ({}): \
                     starts with frontmatter, which a reference cannot have",
                    skills.join("front.md").display()
                ),
                format!(
                    "  \"reference/writing/syntax\" ({}): \
                     reference/writing/syntax:2: cannot compile template: ",
                    skills.join("syntax.md").display()
                ),
                format!(
                    "  \"skill/writing/missing\": cannot open {}: ",
                    skills.join("missing.md").display()
                ),
                format!(
                    "  \"skill/writing/plain\" ({}): \
                     has no frontmatter: a skill starts with a `---` line",
                    skills.join("plain.md").display()
                ),
            ];
            let lines: Vec<_> = message.lines().collect();
            assert_eq!(lines.len(), expected.len(), "{message}");
            for (line, expected) in lines.iter().zip(&expected) {
                assert!(line.starts_with(expected.as_str()), "{message}");
            }

            // Nothing is stored, and the failure stays.
            let error = fairway_skill::skill("skill/writing/valid", ()).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::NotInitialized);
            let again = fairway_skill::initialize().await.unwrap_err();
            assert_eq!(again.to_string(), message);
        }
        "settings" => {
            let error = fairway_skill::initialize().await.unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Initialize);
            let message = chain(&error);
            for problem in [
                "address \"skill/writing\" must have three parts",
                "address \"skill/writing/empty\" has an empty path",
                "unknown key \"skils_dir\"",
            ] {
                assert!(message.contains(problem), "{message}");
            }
        }
        _ => unreachable!("unknown mode {mode}"),
    }
}

fn chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

#[test]
fn process_overrides() {
    if let Ok(mode) = std::env::var(MODE) {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(scenario(&mode));
        return;
    }
    for mode in [
        "uninitialized",
        "empty",
        "overrides",
        "default",
        "invalid",
        "settings",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&home).unwrap();
        prepare(mode, &home, &outside);
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process_overrides", "--nocapture"])
            .env(MODE, mode)
            .env("FAIRWAY_HOME", &home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
