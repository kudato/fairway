//! Tests on the bundle machinery: materialization, the ledger, and
//! the ownership boundary it draws.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;

const MANIFEST: &str = "name = \"fixture\"\ndescription = \"A bundle for tests.\"\n";

/// A bundle that exists only in tests; the shipped registry stays
/// empty until the first skill release.
static FIXTURE: &[Bundle] = &[Bundle {
    manifest: MANIFEST,
    files: &[
        BundleFile {
            path: "SKILL.md",
            contents: "use fairway\n",
        },
        BundleFile {
            path: "scripts/run.md",
            contents: "step one\n",
        },
    ],
}];

fn home() -> (TempDir, PathBuf, PathBuf) {
    let dir = TempDir::new().unwrap();
    let skills = dir.path().join("skills");
    let ledger = dir.path().join("installed.toml");
    (dir, skills, ledger)
}

#[test]
fn a_bundle_is_materialized_and_recorded() {
    let (_dir, skills, ledger) = home();
    let outcome = converge(FIXTURE, &skills, &ledger).unwrap();
    assert_eq!(outcome, Convergence::Created);
    let bundle = skills.join("fixture");
    assert_eq!(
        fs::read_to_string(bundle.join(MANIFEST_NAME)).unwrap(),
        MANIFEST
    );
    assert_eq!(
        fs::read_to_string(bundle.join("SKILL.md")).unwrap(),
        "use fairway\n"
    );
    assert_eq!(
        fs::read_to_string(bundle.join("scripts/run.md")).unwrap(),
        "step one\n"
    );
    let recorded: Ledger = toml::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    assert_eq!(recorded.version, VERSION);
    assert_eq!(recorded.bundles.len(), 1);
    let entry = &recorded.bundles[0];
    assert_eq!(entry.name, "fixture");
    assert_eq!(entry.source, BUILTIN_SOURCE);
    assert_eq!(entry.files, ["manifest.toml", "SKILL.md", "scripts/run.md"]);
}

/// Once the ledger matches this version, convergence must not
/// write: a hand-edited materialized file stays hand-edited.
#[test]
fn a_second_run_touches_nothing() {
    let (_dir, skills, ledger) = home();
    converge(FIXTURE, &skills, &ledger).unwrap();
    let skill = skills.join("fixture/SKILL.md");
    fs::write(&skill, "edited\n").unwrap();
    let outcome = converge(FIXTURE, &skills, &ledger).unwrap();
    assert_eq!(outcome, Convergence::Current);
    assert_eq!(fs::read_to_string(&skill).unwrap(), "edited\n");
}

/// A ledger from another version freezes the world: updating is an
/// explicit act, never a side effect of some other command.
#[test]
fn a_stale_ledger_freezes_the_world() {
    let (_dir, skills, ledger) = home();
    fs::create_dir_all(&skills).unwrap();
    let old = "version = \"0.0.0\"\nbundles = []\n";
    fs::write(&ledger, old).unwrap();
    let outcome = converge(FIXTURE, &skills, &ledger).unwrap();
    assert_eq!(
        outcome,
        Convergence::Stale {
            installed: String::from("0.0.0")
        }
    );
    assert!(!skills.join("fixture").exists());
    assert_eq!(fs::read_to_string(&ledger).unwrap(), old);
}

/// The user's own files next to the materialized ones are outside
/// the ledger and therefore outside fairway's reach.
#[test]
fn a_foreign_file_is_invisible() {
    let (_dir, skills, ledger) = home();
    let own = skills.join("own-skill");
    fs::create_dir_all(&own).unwrap();
    fs::write(own.join("SKILL.md"), "mine\n").unwrap();
    converge(FIXTURE, &skills, &ledger).unwrap();
    assert_eq!(fs::read_to_string(own.join("SKILL.md")).unwrap(), "mine\n");
    let recorded: Ledger = toml::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    assert_eq!(recorded.bundles.len(), 1);
    assert_eq!(recorded.bundles[0].name, "fixture");
}

/// Collisions abort before a single byte lands: no half-written
/// bundle, no ledger claiming files fairway did not write.
#[test]
fn an_occupied_path_aborts_before_any_write() {
    let (_dir, skills, ledger) = home();
    let occupied = skills.join("fixture");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("SKILL.md"), "mine\n").unwrap();
    let error = converge(FIXTURE, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Occupied { .. }), "{error:?}");
    assert_eq!(
        fs::read_to_string(occupied.join("SKILL.md")).unwrap(),
        "mine\n"
    );
    assert!(!occupied.join(MANIFEST_NAME).exists());
    assert!(!ledger.exists());
}

/// A dangling symlink at the ledger path reads as absent but still
/// occupies it; converging through it would replace the user's
/// link.
#[cfg(unix)]
#[test]
fn a_dangling_ledger_symlink_is_refused() {
    let (dir, skills, ledger) = home();
    std::os::unix::fs::symlink(dir.path().join("missing"), &ledger).unwrap();
    let error = converge(FIXTURE, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Occupied { .. }), "{error:?}");
    assert!(ledger.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(!skills.join("fixture").exists());
}

/// Bundle paths are data; data must not be able to reach outside
/// the skills directory.
#[test]
fn a_traversing_path_is_refused() {
    static EVIL: &[Bundle] = &[Bundle {
        manifest: MANIFEST,
        files: &[BundleFile {
            path: "../escape.md",
            contents: "x\n",
        }],
    }];
    let (_dir, skills, ledger) = home();
    let error = converge(EVIL, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Packaging { .. }), "{error:?}");
    assert!(!ledger.exists());
}

#[test]
fn an_absolute_path_is_refused() {
    static EVIL: &[Bundle] = &[Bundle {
        manifest: MANIFEST,
        files: &[BundleFile {
            path: "/etc/escape.md",
            contents: "x\n",
        }],
    }];
    let (_dir, skills, ledger) = home();
    let error = converge(EVIL, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Packaging { .. }), "{error:?}");
    assert!(!ledger.exists());
}

#[test]
fn a_traversing_bundle_name_is_refused() {
    static EVIL: &[Bundle] = &[Bundle {
        manifest: "name = \"..\"\ndescription = \"A bundle for tests.\"\n",
        files: &[],
    }];
    let (_dir, skills, ledger) = home();
    let error = converge(EVIL, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Packaging { .. }), "{error:?}");
    assert!(!ledger.exists());
}

/// Two bundles claiming the same directory would overwrite each
/// other, with both claims landing on the ledger.
#[test]
fn colliding_bundles_are_refused() {
    static TWINS: &[Bundle] = &[
        Bundle {
            manifest: MANIFEST,
            files: &[],
        },
        Bundle {
            manifest: MANIFEST,
            files: &[],
        },
    ];
    let (_dir, skills, ledger) = home();
    let error = converge(TWINS, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Packaging { .. }), "{error:?}");
    assert!(!ledger.exists());
    assert!(!skills.join("fixture").exists());
}

#[test]
fn a_broken_ledger_is_refused() {
    let (_dir, skills, ledger) = home();
    fs::write(&ledger, "not a ledger [").unwrap();
    let error = converge(FIXTURE, &skills, &ledger).unwrap_err();
    assert!(matches!(error, AssetsError::Ledger { .. }), "{error:?}");
}

#[test]
fn an_empty_registry_still_commits_a_ledger() {
    let (_dir, skills, ledger) = home();
    let outcome = converge(&[], &skills, &ledger).unwrap();
    assert_eq!(outcome, Convergence::Created);
    assert!(skills.is_dir());
    let recorded: Ledger = toml::from_str(&fs::read_to_string(&ledger).unwrap()).unwrap();
    assert_eq!(recorded.version, VERSION);
    assert!(recorded.bundles.is_empty());
}

mod manifest {
    use super::*;

    #[test]
    fn parses_name_and_description() {
        let manifest: Manifest = toml::from_str(MANIFEST).unwrap();
        assert_eq!(manifest.name, "fixture");
        assert_eq!(manifest.description, "A bundle for tests.");
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let text = format!("{MANIFEST}version = \"1\"\n");
        assert!(toml::from_str::<Manifest>(&text).is_err());
    }

    #[test]
    fn a_missing_description_is_refused() {
        assert!(toml::from_str::<Manifest>("name = \"x\"\n").is_err());
    }
}

/// The twin of the registry: the `assets/` tree in the repository
/// must mirror what the binary embeds, file for file.
#[test]
fn the_registry_mirrors_the_assets_tree() {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut entries: Vec<String> = fs::read_dir(&assets)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    let mut expected = vec![String::from("config.toml"), String::from("policy")];
    if !REGISTRY.is_empty() {
        expected.push(String::from("skills"));
    }
    expected.sort();
    assert_eq!(entries, expected);
    for bundle in REGISTRY {
        let manifest: Manifest = toml::from_str(bundle.manifest).unwrap();
        let dir = assets.join("skills").join(&manifest.name);
        assert_eq!(
            fs::read_to_string(dir.join(MANIFEST_NAME)).unwrap(),
            bundle.manifest
        );
        let mut on_disk = walk(&dir, Path::new(""));
        on_disk.sort();
        let mut embedded: Vec<String> = vec![String::from(MANIFEST_NAME)];
        for file in bundle.files {
            assert_eq!(
                fs::read_to_string(dir.join(file.path)).unwrap(),
                file.contents
            );
            embedded.push(file.path.to_owned());
        }
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }
}

/// All files under `dir`, as `/`-separated paths relative to it.
fn walk(dir: &Path, prefix: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        let relative = if prefix.as_os_str().is_empty() {
            name.clone()
        } else {
            format!("{}/{name}", prefix.display())
        };
        if entry.file_type().unwrap().is_dir() {
            files.extend(walk(&entry.path(), Path::new(&relative)));
        } else {
            files.push(relative);
        }
    }
    files
}
