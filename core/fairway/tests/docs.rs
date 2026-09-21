//! Run CLI guide transcripts against Fairway with the plugins from the same Markdown.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

fn rust_examples(markdown: &str) -> Vec<String> {
    let mut examples = Vec::new();
    let mut source = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))
                if info.as_ref() == "rust" =>
            {
                source = Some(String::new());
            }
            Event::Text(text) => {
                if let Some(source) = &mut source {
                    source.push_str(&text);
                }
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some(source) = source.take() {
                    examples.push(source);
                }
            }
            _ => {}
        }
    }
    assert!(!examples.is_empty(), "the CLI guide has no Rust examples");
    examples
}

#[test]
fn cli_guides() {
    let fairway = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = fairway.parent().unwrap().parent().unwrap();
    let cli = root.join("core/fairway-cli");
    let filesystem = root.join("core/fairway-fs");
    let target = root.join("target/cli-doc-tests");
    fs::create_dir_all(&target).unwrap();

    for language in ["ru", "en"] {
        let guide = root.join(format!("docs/{language}/plugin-development/cli.md"));
        let markdown = fs::read_to_string(&guide).unwrap();
        let directory = tempfile::tempdir_in(&target).unwrap();
        let src = directory.path().join("src");
        fs::create_dir(&src).unwrap();

        // Each complete plugin example gets its own module; its source stays in Markdown.
        let mut library = String::new();
        for (index, source) in rust_examples(&markdown).iter().enumerate() {
            fs::write(src.join(format!("example_{index}.rs")), source).unwrap();
            library.push_str(&format!("mod example_{index};\n"));
        }
        fs::write(src.join("lib.rs"), library).unwrap();

        // Reuse the actual application so transcripts also exercise startup and exit codes.
        let main = fs::read_to_string(fairway.join("src/main.rs")).unwrap();
        fs::write(
            src.join("main.rs"),
            format!("{main}\nuse fairway_cli_docs as _;\n"),
        )
        .unwrap();
        for module in ["app.rs", "signal.rs"] {
            fs::copy(fairway.join("src").join(module), src.join(module)).unwrap();
        }
        fs::write(
            directory.path().join("Cargo.toml"),
            format!(
                r#"
[workspace]

[package]
name = "fairway-cli-docs"
version = "{version}"
edition = "2024"
description = "CLI documentation examples"

[dependencies]
fairway-cli = {{ path = {cli:?} }}
fairway-fs = {{ path = {filesystem:?} }}
clap = {{ version = "4", features = ["derive"] }}
tokio = {{ version = "1.26", features = ["rt", "rt-multi-thread", "signal", "sync", "time", "macros", "net"] }}
tokio-util = "0.7"
anyhow = "1"
axum = {{ version = "0.8", default-features = false, features = ["http1", "tokio"] }}
"#,
                version = env!("CARGO_PKG_VERSION"),
            ),
        )
        .unwrap();
        fs::copy(root.join("Cargo.lock"), directory.path().join("Cargo.lock")).unwrap();

        let output = Command::new(env!("CARGO"))
            .current_dir(directory.path())
            .args(["build", "--offline", "--quiet", "--target-dir"])
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "could not build {}:\n{}\n{}",
            guide.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let binary = target
            .join("debug")
            .join(format!("fairway-cli-docs{}", std::env::consts::EXE_SUFFIX));
        trycmd::TestCases::new()
            .register_bin("fairway", binary)
            .timeout(Duration::from_secs(10))
            .case(&guide)
            .run();
    }
}
