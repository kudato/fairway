//! Tests on the policy schema: the shipped defaults must satisfy
//! it, and every departure from it must fail loudly.

use std::path::Path;

use super::*;

fn parse(text: &str) -> Result<GitPolicy, ConfigError> {
    GitPolicy::parse(text, Path::new("git.toml (test)"))
}

#[test]
fn the_shipped_defaults_satisfy_the_schema() {
    let policy = parse(DEFAULT).unwrap();
    assert!(!policy.git_available.value);
    assert_eq!(policy.git_available.action, Action::Stop);
    assert_eq!(policy.head_exists.action, Action::Proceed);
    assert_eq!(policy.work_tree_clean.action, Action::Adjust);
}

#[test]
fn an_unknown_table_is_refused() {
    let text = format!("{DEFAULT}\n[garden]\ngnome = true\n");
    let error = parse(&text).unwrap_err().to_string();
    assert!(error.contains("garden"), "{error}");
}

#[test]
fn an_unknown_key_inside_a_rule_is_refused() {
    let text = DEFAULT.replacen("value = false", "value = false\nseverity = 3", 1);
    assert!(parse(&text).is_err());
}

#[test]
fn a_missing_key_is_refused() {
    let text = DEFAULT.replacen("action = \"stop\"", "", 1);
    let error = parse(&text).unwrap_err().to_string();
    assert!(error.contains("action"), "{error}");
}

#[test]
fn a_wrong_type_is_refused() {
    let text = DEFAULT.replacen("value = false", "value = \"no\"", 1);
    assert!(parse(&text).is_err());
}

#[test]
fn an_unknown_action_is_refused() {
    let text = DEFAULT.replacen("action = \"stop\"", "action = \"halt\"", 1);
    assert!(parse(&text).is_err());
}

/// The action names are part of the file format: exact, lowercase.
#[test]
fn action_names_are_case_sensitive() {
    let text = DEFAULT.replacen("action = \"stop\"", "action = \"Stop\"", 1);
    assert!(parse(&text).is_err());
}

/// The error must carry both ways out: fixing the file by hand and
/// deleting it for a fresh start.
#[test]
fn the_error_names_the_file_and_the_way_out() {
    let error = parse("").unwrap_err().to_string();
    assert!(error.contains("git.toml (test)"), "{error}");
    assert!(error.contains("delete"), "{error}");
}

#[test]
fn actions_map_to_their_verdicts() {
    let text = || String::from("go");
    assert_eq!(Action::Proceed.verdict(text()), Verdict::Proceed(text()));
    assert_eq!(Action::Adjust.verdict(text()), Verdict::Adjust(text()));
    assert_eq!(Action::Stop.verdict(text()), Verdict::Stop(text()));
}
