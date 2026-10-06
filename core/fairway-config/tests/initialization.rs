//! Startup defaults, atomic publication, and failures in isolated processes.

use std::{
    collections::BTreeMap,
    error::Error as _,
    path::Path,
    process::Command,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use fairway_codec::{
    Decode, Toml,
    serde::{Deserialize, Deserializer},
    toml,
};
use fairway_config::initialize;

static PREPARES: AtomicUsize = AtomicUsize::new(0);
static STARTED: OnceLock<tokio::sync::Notify> = OnceLock::new();
static RELEASE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

struct Settings {
    value: u64,
}

impl Default for Settings {
    fn default() -> Self {
        PREPARES.fetch_add(1, Ordering::SeqCst);
        Self { value: 5 }
    }
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D: Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(crate = "fairway_codec::serde", deny_unknown_fields)]
        struct Raw {
            value: u64,
        }
        let Raw { value } = Raw::deserialize(input)?;
        PREPARES.fetch_add(1, Ordering::SeqCst);
        if value == 999 {
            // Actual misuse of the public API while settings are still being prepared.
            ALPHA.get();
        }
        if value == 100 {
            STARTED.get().unwrap().notify_one();
            let (lock, ready) = &RELEASE;
            let _guard = ready
                .wait_while(lock.lock().unwrap(), |released| !*released)
                .unwrap();
        }
        Ok(Self { value })
    }
}

fairway_config::namespace!(ALPHA: Settings, "alpha");
fairway_config::namespace!(BETA: Settings, "beta");
fairway_config::namespace!(DOCUMENTS: BTreeMap<String, BTreeMap<String, String>>, "documents");

fn config(home: &Path, beta: &str) {
    std::fs::write(
        home.join("config.toml"),
        format!("[alpha]\nvalue = 10\n[beta]\nvalue = {beta}\n"),
    )
    .unwrap();
}

fn inaccessible() {
    assert!(std::panic::catch_unwind(|| ALPHA.get()).is_err());
    assert!(std::panic::catch_unwind(|| BETA.get()).is_err());
}

fn release() {
    *RELEASE.0.lock().unwrap() = true;
    RELEASE.1.notify_all();
}

async fn scenario(mode: &str, home: &Path) {
    inaccessible();
    fairway_config::validate_registry();
    assert_eq!(PREPARES.load(Ordering::SeqCst), 0);
    match mode {
        "defaults" => {
            initialize().await.unwrap();
            assert_eq!(ALPHA.get().value, 5);
            assert_eq!(BETA.get().value, 5);
            assert!(!std::ptr::eq(ALPHA.get(), BETA.get()));
            assert!(!home.join("config.toml").exists());
            let automatic = std::fs::read_to_string(home.join("config.auto.toml")).unwrap();
            assert_eq!(
                automatic,
                "# This file is managed automatically by Fairway.\n\
                 # Do not edit it manually.\n\n"
            );
        }
        "automatic" => {
            config(home, "20");
            std::fs::create_dir(home.join("conf.d")).unwrap();
            std::fs::write(home.join("conf.d/last.toml"), "[beta]\nvalue = 30\n").unwrap();
            let automatic = home.join("config.auto.toml");
            std::fs::write(&automatic, "[beta]\nvalue = 40\n").unwrap();
            initialize().await.unwrap();
            assert_eq!(ALPHA.get().value, 10);
            assert_eq!(BETA.get().value, 40);
            std::fs::write(&automatic, "[beta]\nvalue = 50\n").unwrap();
            initialize().await.unwrap();
            assert_eq!(BETA.get().value, 40);
            assert_eq!(PREPARES.load(Ordering::SeqCst), 2);
        }
        "failure" => {
            config(home, "'invalid'");
            let error = initialize().await.unwrap_err().to_string();
            assert!(error.contains("beta"));
            inaccessible();
        }
        "reentrant" => {
            config(home, "999");
            let task = tokio::spawn(initialize());
            assert!(task.await.unwrap_err().is_panic());
            inaccessible();
        }
        "success" => {
            STARTED.set(tokio::sync::Notify::new()).unwrap();
            config(home, "100");
            let task = tokio::spawn(initialize());
            tokio::time::timeout(Duration::from_secs(5), STARTED.get().unwrap().notified())
                .await
                .unwrap();
            // Alpha is already deserialized. Neither namespace is published yet.
            inaccessible();
            release();
            task.await.unwrap().unwrap();
            assert_eq!(PREPARES.load(Ordering::SeqCst), 2);
            assert_eq!(ALPHA.get().value, 10);
            assert_eq!(BETA.get().value, 100);
        }
        "uninitialized" => {
            let error = ALPHA.set(&["value"], 30).await.unwrap_err();
            assert!(error.to_string().contains("not initialized"));
            assert!(!home.exists());
        }
        "set" => {
            config(home, "20");
            let primary = format!(
                "{}[documents.paths]\ninherited = 'base'\n",
                std::fs::read_to_string(home.join("config.toml")).unwrap()
            );
            std::fs::write(home.join("config.toml"), &primary).unwrap();
            std::fs::create_dir(home.join("conf.d")).unwrap();
            let extra = "[alpha]\nvalue = 30\n";
            std::fs::write(home.join("conf.d/last.toml"), extra).unwrap();
            initialize().await.unwrap();

            ALPHA.set(&["value"], 40).await.unwrap();
            BETA.set(&["value"], 50).await.unwrap();
            DOCUMENTS
                .set(&["paths", "skill/git/commit.md"], "original")
                .await
                .unwrap();
            DOCUMENTS
                .set(&["paths", "skill/git/commit.md"], "updated")
                .await
                .unwrap();
            let mut tasks = Vec::new();
            for index in 0..8 {
                tasks.push(tokio::spawn(async move {
                    let key = format!("reference/git/{index}");
                    DOCUMENTS.set(&["paths", &key], index.to_string()).await
                }));
            }
            for task in tasks {
                task.await.unwrap().unwrap();
            }

            let text = std::fs::read_to_string(home.join("config.auto.toml")).unwrap();
            assert!(text.starts_with(
                "# This file is managed automatically by Fairway.\n\
                 # Do not edit it manually.\n\n"
            ));
            assert_eq!(text.matches("# Do not edit it manually.").count(), 1);
            let automatic = Toml::<toml::Table>::decode(text.into_bytes())
                .unwrap()
                .into_inner();
            assert_eq!(automatic["alpha"]["value"].as_integer(), Some(40));
            assert_eq!(automatic["beta"]["value"].as_integer(), Some(50));
            let paths = automatic["documents"]["paths"].as_table().unwrap();
            assert_eq!(paths.len(), 9);
            assert!(!paths.contains_key("inherited"));
            assert_eq!(paths["skill/git/commit.md"].as_str(), Some("updated"));
            for index in 0..8 {
                assert_eq!(
                    paths[&format!("reference/git/{index}")].as_str(),
                    Some(index.to_string().as_str())
                );
            }
            assert_eq!(
                std::fs::read_to_string(home.join("config.toml")).unwrap(),
                primary
            );
            assert_eq!(
                std::fs::read_to_string(home.join("conf.d/last.toml")).unwrap(),
                extra
            );

            assert_eq!(ALPHA.get().value, 30);
            assert_eq!(BETA.get().value, 20);
            assert_eq!(DOCUMENTS.get()["paths"].len(), 1);
            // A lower-priority change remains visible on the next startup.
            std::fs::write(
                home.join("config.toml"),
                primary.replace("'base'", "'changed'"),
            )
            .unwrap();
        }
        "saved" => {
            initialize().await.unwrap();
            assert_eq!(ALPHA.get().value, 40);
            assert_eq!(BETA.get().value, 50);
            let paths = &DOCUMENTS.get()["paths"];
            assert_eq!(paths.len(), 10);
            assert_eq!(paths["inherited"], "changed");
            assert_eq!(paths["skill/git/commit.md"], "updated");
        }
        "invalid" => {
            config(home, "20");
            initialize().await.unwrap();
            let automatic = home.join("config.auto.toml");
            let template = std::fs::read(&automatic).unwrap();
            let error = ALPHA.set(&["value"], -1).await.unwrap_err();
            assert!(error.to_string().contains("alpha"));
            assert!(error.source().is_some());
            assert_eq!(std::fs::read(&automatic).unwrap(), template);
            assert!(ALPHA.set(&["unknown"], 1).await.is_err());
            assert_eq!(std::fs::read(&automatic).unwrap(), template);
            assert!(ALPHA.set(&[], 1).await.is_err());
            assert_eq!(std::fs::read(&automatic).unwrap(), template);
            assert!(ALPHA.set(&["value"], None::<u64>).await.is_err());
            assert_eq!(std::fs::read(&automatic).unwrap(), template);

            ALPHA.set(&["value"], 60).await.unwrap();
            let before = std::fs::read(&automatic).unwrap();
            assert!(ALPHA.set(&["value", "child"], 1).await.is_err());
            assert_eq!(std::fs::read(&automatic).unwrap(), before);

            // Validation rereads all namespaces, including changes made since startup.
            config(home, "'invalid'");
            let error = ALPHA.set(&["value"], 70).await.unwrap_err();
            assert!(error.to_string().contains("beta"));
            assert_eq!(std::fs::read(&automatic).unwrap(), before);
            config(home, "20");
            // An invalid lower value is allowed when the saved override fixes it.
            std::fs::create_dir(home.join("conf.d")).unwrap();
            std::fs::write(
                home.join("conf.d/last.toml"),
                "[alpha]\nvalue = 'invalid'\n",
            )
            .unwrap();
            ALPHA.set(&["value"], 70).await.unwrap();
            assert_eq!(ALPHA.get().value, 10);

            let before = std::fs::read(&automatic).unwrap();
            std::fs::write(&automatic, "[broken").unwrap();
            assert!(ALPHA.set(&["value"], 80).await.is_err());
            assert_eq!(std::fs::read_to_string(&automatic).unwrap(), "[broken");
            std::fs::write(&automatic, before).unwrap();
            std::fs::remove_file(&automatic).unwrap();
            let error = ALPHA.set(&["value"], 80).await.unwrap_err();
            assert_eq!(
                error
                    .source()
                    .unwrap()
                    .downcast_ref::<fairway_fs::Error>()
                    .unwrap()
                    .kind(),
                std::io::ErrorKind::NotFound
            );
            assert!(!automatic.exists());
        }
        _ => unreachable!(),
    }
}

#[test]
fn process_configuration() {
    if let Ok(mode) = std::env::var("FAIRWAY_CONFIG_TEST_MODE") {
        let home = std::path::PathBuf::from(std::env::var_os("FAIRWAY_HOME").unwrap());
        let mut builder = if mode == "success" {
            // One worker pauses inline deserialization while the test checks publication.
            tokio::runtime::Builder::new_multi_thread()
        } else {
            tokio::runtime::Builder::new_current_thread()
        };
        let runtime = builder.worker_threads(2).enable_all().build().unwrap();
        runtime.block_on(scenario(&mode, &home));
        drop(runtime);
        // Published values remain accessible independently of the original runtime.
        if matches!(mode.as_str(), "defaults" | "automatic" | "success") {
            assert!(ALPHA.get().value > 0);
        }
        return;
    }
    let scenarios: &[&[&str]] = &[
        &["defaults"],
        &["automatic"],
        &["success"],
        &["failure"],
        &["reentrant"],
        &["uninitialized"],
        &["set", "saved"],
        &["invalid"],
    ];
    for modes in scenarios {
        let home = tempfile::tempdir().unwrap();
        let config_home = if matches!(modes[0], "defaults" | "uninitialized") {
            home.path().join("nested/fairway")
        } else {
            home.path().to_owned()
        };
        for mode in *modes {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process_configuration", "--nocapture"])
                .env("FAIRWAY_CONFIG_TEST_MODE", mode)
                .env("FAIRWAY_HOME", &config_home)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let start = std::time::Instant::now();
            while child.try_wait().unwrap().is_none() {
                if start.elapsed() > Duration::from_secs(15) {
                    child.kill().unwrap();
                    let output = child.wait_with_output().unwrap();
                    panic!("{mode} timed out: {output:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
