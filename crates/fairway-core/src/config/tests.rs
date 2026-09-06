//! Tests on the settings schema: the shipped defaults must satisfy
//! it, and every departure from it must fail loudly.

use std::path::Path;

use super::*;

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::parse(text, Path::new("config.toml (test)"))
}

#[test]
fn the_shipped_defaults_satisfy_the_schema() {
    let config = parse(DEFAULT).unwrap();
    assert_eq!(config.skills.dir, "skills");
}

#[test]
fn an_unknown_table_is_refused() {
    let text = format!("{DEFAULT}\n[garden]\ngnome = true\n");
    let error = parse(&text).unwrap_err().to_string();
    assert!(error.contains("garden"), "{error}");
}

#[test]
fn an_unknown_key_is_refused() {
    let text = DEFAULT.replacen("dir = \"skills\"", "dir = \"skills\"\nglob = \"*\"", 1);
    assert!(parse(&text).is_err());
}

#[test]
fn a_missing_key_is_refused() {
    let text = DEFAULT.replacen("dir = \"skills\"", "", 1);
    let error = parse(&text).unwrap_err().to_string();
    assert!(error.contains("dir"), "{error}");
}

#[test]
fn a_wrong_type_is_refused() {
    let text = DEFAULT.replacen("dir = \"skills\"", "dir = false", 1);
    assert!(parse(&text).is_err());
}

/// Empty means the home root itself: skills would mix with the
/// settings and the ledger, so it is refused.
#[test]
fn an_empty_dir_is_refused() {
    let text = DEFAULT.replacen("dir = \"skills\"", "dir = \"\"", 1);
    let error = parse(&text).unwrap_err().to_string();
    assert!(error.contains("skills.dir"), "{error}");
}

/// The error must carry both ways out: fixing the file by hand and
/// deleting it for a fresh start.
#[test]
fn the_error_names_the_file_and_the_way_out() {
    let error = parse("").unwrap_err().to_string();
    assert!(error.contains("config.toml (test)"), "{error}");
    assert!(error.contains("delete"), "{error}");
}
