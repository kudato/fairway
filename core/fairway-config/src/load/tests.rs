//! Source discovery, overlays, defaults, and diagnostics without global publication.

use std::{error::Error as _, fs, path::Path};

use serde::Deserialize;
use tempfile::TempDir;

use super::*;
use crate::Namespace;

static TABLE: Namespace<toml::Table> = Namespace::new("settings");
static TABLE_REGISTRATION: Registration = Registration::new(&TABLE, "test table");

static QUOTED: Namespace<toml::Table> = Namespace::new("settings.named");
static QUOTED_REGISTRATION: Registration = Registration::new(&QUOTED, "quoted name");
static UPPER: Namespace<toml::Table> = Namespace::new("Settings");
static UPPER_REGISTRATION: Registration = Registration::new(&UPPER, "case-sensitive name");

#[tokio::test]
async fn namespace_names_are_literal_and_paths_remain_unexpanded() {
    let home = TempDir::new().unwrap();
    write(
        home.path(),
        "config.toml",
        r#"
["settings.named"]
path = "$HOME/~/relative"
[settings.named]
value = "nested"
[Settings]
value = "uppercase"
"#,
    );
    let values = load(
        home.path(),
        vec![
            &TABLE_REGISTRATION,
            &QUOTED_REGISTRATION,
            &UPPER_REGISTRATION,
        ],
    )
    .await
    .unwrap();
    let literal = values["settings.named"]
        .downcast_ref::<toml::Table>()
        .unwrap();
    assert_eq!(literal["path"].as_str(), Some("$HOME/~/relative"));
    assert_eq!(literal.len(), 1);
    assert_eq!(
        values["settings"].downcast_ref::<toml::Table>().unwrap()["named"]["value"].as_str(),
        Some("nested")
    );
    assert_eq!(
        values["Settings"].downcast_ref::<toml::Table>().unwrap()["value"].as_str(),
        Some("uppercase")
    );
}

fn write(home: &Path, relative: &str, text: impl AsRef<[u8]>) {
    let path = home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

async fn table(home: &Path) -> Result<toml::Table, Error> {
    let mut values = load(home, vec![&TABLE_REGISTRATION]).await?;
    Ok(*values
        .remove("settings")
        .unwrap()
        .downcast::<toml::Table>()
        .unwrap())
}

fn diagnostic(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        text.push_str(&format!("\n{error}"));
        source = error.source();
    }
    text
}

#[tokio::test]
async fn sources_are_optional_and_loading_does_not_create_them() {
    let directory = TempDir::new().unwrap();
    let home = directory.path().join("absent");
    assert!(table(&home).await.unwrap().is_empty());
    assert!(!home.exists());
    write(&home, "config.toml", "");
    assert!(table(&home).await.unwrap().is_empty());
    assert!(!home.join("conf.d").exists());
    fs::remove_file(home.join("config.toml")).unwrap();
    write(&home, "conf.d/nested/value.toml", "[settings]\nvalue = 3\n");
    assert_eq!(table(&home).await.unwrap()["value"].as_integer(), Some(3));
    assert!(!home.join("config.toml").exists());
}

#[tokio::test]
async fn paths_define_precedence_including_nested_and_hidden_sources() {
    let home = TempDir::new().unwrap();
    let root = home.path();
    write(root, "config.toml", "[settings]\nvalue = 0\nkeep = true\n");
    write(root, "conf.d/z/00-first.toml", "[settings]\nvalue = 5\n");
    write(root, "conf.d/a/99-last.toml", "[settings]\nvalue = 4\n");
    write(root, "conf.d/2.toml", "[settings]\nvalue = 3\n");
    write(root, "conf.d/10.toml", "[settings]\nvalue = 2\n");
    write(
        root,
        "conf.d/.hidden/.settings.toml",
        "[settings]\nhidden = true\n",
    );
    write(root, "conf.d/ignored.TOML", "not valid TOML");
    write(root, "conf.d/ignored.toml.bak", "not valid TOML");
    let paths = additional(&root.join("conf.d")).await.unwrap();
    let relative: Vec<_> = paths
        .iter()
        .map(|path| path.strip_prefix(root.join("conf.d")).unwrap())
        .collect();
    assert_eq!(
        relative,
        [
            ".hidden/.settings.toml",
            "10.toml",
            "2.toml",
            "a/99-last.toml",
            "z/00-first.toml"
        ]
        .map(Path::new)
    );
    let result = table(root).await.unwrap();
    assert_eq!(result["value"].as_integer(), Some(5));
    assert_eq!(result["keep"].as_bool(), Some(true));
    assert_eq!(result["hidden"].as_bool(), Some(true));
}

#[tokio::test]
async fn tables_merge_recursively_and_arrays_and_other_values_replace() {
    let home = TempDir::new().unwrap();
    write(
        home.path(),
        "config.toml",
        r#"
[settings]
numbers = [1, 2]
empty = [1]
records = [{name = "old", keep = true}]
scalar = 1
map = {value = 1}
[settings.nested]
keep = true
value = 1
[settings.unchanged]
keep = true
"#,
    );
    write(
        home.path(),
        "conf.d/change.toml",
        r#"
[settings]
numbers = [3]
empty = []
records = [{name = "new"}]
scalar = {value = 2}
map = "replaced"
[settings.nested]
value = 2
[settings.unchanged]
"#,
    );
    let actual = table(home.path()).await.unwrap();
    let expected: toml::Table = toml::from_str(
        r#"
numbers = [3]
empty = []
records = [{name = "new"}]
scalar = {value = 2}
map = "replaced"
nested = {keep = true, value = 2}
unchanged = {keep = true}
"#,
    )
    .unwrap();
    assert_eq!(actual, expected);
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    listen: String,
    ttl: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            listen: "localhost".into(),
            ttl: 300,
        }
    }
}

static SETTINGS: Namespace<Settings> = Namespace::new("settings");
static SETTINGS_REGISTRATION: Registration = Registration::new(&SETTINGS, "test settings");

#[tokio::test]
async fn deserialization_and_defaults_follow_the_complete_overlay() {
    let home = TempDir::new().unwrap();
    write(
        home.path(),
        "config.toml",
        "[settings]\nlisten = false\nttl = 60\n",
    );
    write(
        home.path(),
        "conf.d/fix.toml",
        "[settings]\nlisten = 'server'\n",
    );
    let values = load(home.path(), vec![&SETTINGS_REGISTRATION])
        .await
        .unwrap();
    assert_eq!(
        values["settings"].downcast_ref::<Settings>().unwrap(),
        &Settings {
            listen: "server".into(),
            ttl: 60
        }
    );
    fs::remove_dir_all(home.path().join("conf.d")).unwrap();
    write(
        home.path(),
        "config.toml",
        "[settings]\nlisten = 'server'\n",
    );
    let values = load(home.path(), vec![&SETTINGS_REGISTRATION])
        .await
        .unwrap();
    assert_eq!(
        values["settings"].downcast_ref::<Settings>().unwrap().ttl,
        300
    );
}

#[derive(Default, Deserialize)]
struct Required {
    value: u64,
}
static REQUIRED: Namespace<Required> = Namespace::new("required");
static REQUIRED_REGISTRATION: Registration = Registration::new(&REQUIRED, "test required");

#[derive(Deserialize)]
#[serde(try_from = "RawRange")]
struct Range {
    start: u64,
    end: u64,
}

#[derive(Deserialize)]
struct RawRange {
    start: u64,
    end: u64,
}

impl Default for Range {
    fn default() -> Self {
        Self { start: 0, end: 1 }
    }
}

impl TryFrom<RawRange> for Range {
    type Error = &'static str;
    fn try_from(raw: RawRange) -> Result<Self, Self::Error> {
        if raw.start >= raw.end {
            return Err("start must precede end");
        }
        Ok(Self {
            start: raw.start,
            end: raw.end,
        })
    }
}

static RANGE: Namespace<Range> = Namespace::new("range");
static RANGE_REGISTRATION: Registration = Registration::new(&RANGE, "validated range");

#[tokio::test]
async fn plugin_validation_checks_the_final_combined_settings() {
    let home = TempDir::new().unwrap();
    write(home.path(), "config.toml", "[range]\nstart = 10\nend = 5\n");
    let error = load(home.path(), vec![&RANGE_REGISTRATION])
        .await
        .unwrap_err();
    assert!(diagnostic(&error).contains("start must precede end"));
    write(home.path(), "conf.d/extend.toml", "[range]\nend = 20\n");
    let values = load(home.path(), vec![&RANGE_REGISTRATION]).await.unwrap();
    let range = values["range"].downcast_ref::<Range>().unwrap();
    assert_eq!((range.start, range.end), (10, 20));
}

#[tokio::test]
async fn absent_and_empty_tables_differ_and_required_fields_may_span_files() {
    let home = TempDir::new().unwrap();
    let values = load(home.path(), vec![&REQUIRED_REGISTRATION])
        .await
        .unwrap();
    assert_eq!(
        values["required"].downcast_ref::<Required>().unwrap().value,
        0
    );
    write(home.path(), "config.toml", "[required]\n");
    let error = load(home.path(), vec![&REQUIRED_REGISTRATION])
        .await
        .unwrap_err();
    assert!(diagnostic(&error).contains("missing field `value`"));
    write(
        home.path(),
        "conf.d/required.toml",
        "[required]\nvalue = 5\n",
    );
    let values = load(home.path(), vec![&REQUIRED_REGISTRATION])
        .await
        .unwrap();
    assert_eq!(
        values["required"].downcast_ref::<Required>().unwrap().value,
        5
    );
}

#[tokio::test]
async fn every_document_is_parsed_even_without_registrations_or_before_an_override() {
    let home = TempDir::new().unwrap();
    for bytes in [
        b"[unknown".as_slice(),
        b"[unknown]\nvalue = '\xff'",
        b"[unknown]\nx = 1\nx = 2",
    ] {
        write(home.path(), "config.toml", bytes);
        let error = load(home.path(), vec![]).await.unwrap_err();
        assert!(error.to_string().contains("config.toml"));
        assert!(error.source().is_some());
    }
    write(home.path(), "config.toml", "[unknown]\nvalue = true\n");
    assert!(table(home.path()).await.unwrap().is_empty());
    write(home.path(), "conf.d/a.toml", "[settings\n");
    write(home.path(), "conf.d/b.toml", "[settings]\nvalue = 5\n");
    let error = table(home.path()).await.unwrap_err();
    assert!(error.to_string().contains("a.toml"));
}

#[tokio::test]
async fn every_registered_section_must_be_a_table() {
    let home = TempDir::new().unwrap();
    write(home.path(), "conf.d/fix.toml", "[settings]\nvalue = 1\n");
    for value in ["false", "1", "[]", "'text'", "[{value = 2}]"] {
        write(home.path(), "config.toml", format!("settings = {value}\n"));
        let error = table(home.path()).await.unwrap_err().to_string();
        assert!(
            error.contains("settings") && error.contains("must be a table"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn errors_retain_sources_and_name_every_contributing_file() {
    let home = TempDir::new().unwrap();
    write(
        home.path(),
        "config.toml",
        "[settings]\nlisten = 'server'\n",
    );
    for text in ["[settings]\nttl = -1\n", "[settings]\nunknown = true\n"] {
        write(home.path(), "conf.d/bad.toml", text);
        let error = load(home.path(), vec![&SETTINGS_REGISTRATION])
            .await
            .unwrap_err();
        let error = error.clone();
        assert!(error.to_string().contains("config.toml"));
        assert!(error.to_string().contains("bad.toml"));
        assert!(error.to_string().contains("settings"));
        assert!(
            error
                .source()
                .unwrap()
                .downcast_ref::<toml::de::Error>()
                .is_some()
        );
    }
    fs::remove_file(home.path().join("config.toml")).unwrap();
    fs::create_dir(home.path().join("config.toml")).unwrap();
    let error = table(home.path()).await.unwrap_err();
    assert_ne!(
        error
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[tokio::test]
async fn a_file_in_place_of_conf_d_and_a_disappeared_selected_file_are_errors() {
    let home = TempDir::new().unwrap();
    write(home.path(), "conf.d", "");
    assert!(
        table(home.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("conf.d")
    );
    fs::remove_file(home.path().join("conf.d")).unwrap();
    write(home.path(), "conf.d/gone.toml", "[settings]\n");
    let paths = additional(&home.path().join("conf.d")).await.unwrap();
    fs::remove_file(&paths[0]).unwrap();
    assert!(read(&paths[0], false).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn links_allow_external_sources_and_aliases_but_reject_cycles() {
    use std::os::unix::fs::symlink;
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    write(external.path(), "settings.toml", "[settings]\nvalue = 1\n");
    fs::create_dir(home.path().join("conf.d")).unwrap();
    symlink(external.path(), home.path().join("conf.d/a")).unwrap();
    symlink(external.path(), home.path().join("conf.d/c")).unwrap();
    symlink(
        external.path().join("settings.toml"),
        home.path().join("config.toml"),
    )
    .unwrap();
    write(home.path(), "conf.d/b.toml", "[settings]\nvalue = 2\n");
    assert_eq!(
        table(home.path()).await.unwrap()["value"].as_integer(),
        Some(1)
    );
    symlink(home.path().join("conf.d"), external.path().join("back")).unwrap();
    let error = table(home.path()).await.unwrap_err().to_string();
    assert!(error.contains("cycle") && error.contains("back"), "{error}");
}

#[cfg(unix)]
#[tokio::test]
async fn dangling_selected_links_and_fifos_fail_without_waiting() {
    use std::os::unix::fs::symlink;
    let home = TempDir::new().unwrap();
    fs::create_dir(home.path().join("conf.d")).unwrap();
    let path = home.path().join("conf.d/broken.toml");
    symlink(home.path().join("missing"), &path).unwrap();
    assert!(
        table(home.path())
            .await
            .unwrap_err()
            .to_string()
            .contains("broken.toml")
    );
    fs::remove_file(&path).unwrap();
    // The main path exercises descriptor validation; the extra path exercises traversal.
    for relative in ["config.toml", "conf.d/pipe.toml"] {
        let path = home.path().join(relative);
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let error = tokio::time::timeout(std::time::Duration::from_secs(3), table(home.path()))
            .await
            .unwrap()
            .unwrap_err();
        assert!(diagnostic(&error).contains("regular file"));
        fs::remove_file(path).unwrap();
    }
}

#[cfg(windows)]
#[tokio::test]
async fn windows_junctions_follow_logical_precedence_and_reject_cycles() {
    fn junction(target: &Path, link: &Path) {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link.components().collect::<std::path::PathBuf>())
            .arg(target.components().collect::<std::path::PathBuf>())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let home = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    fs::create_dir(home.path().join("conf.d")).unwrap();
    write(external.path(), "settings.toml", "[settings]\nvalue = 1\n");
    junction(external.path(), &home.path().join("conf.d/a"));
    junction(external.path(), &home.path().join("conf.d/c"));
    write(home.path(), "conf.d/b.toml", "[settings]\nvalue = 2\n");
    assert_eq!(
        table(home.path()).await.unwrap()["value"].as_integer(),
        Some(1)
    );
    let back = external.path().join("back");
    junction(&home.path().join("conf.d"), &back);
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), table(home.path())).await;
    // Remove the cycle explicitly before either temporary directory is dropped.
    fs::remove_dir(&back).unwrap();
    let error = result
        .expect("junction cycle must not hang")
        .unwrap_err()
        .to_string();
    assert!(error.contains("cycle") && error.contains("back"), "{error}");
}
