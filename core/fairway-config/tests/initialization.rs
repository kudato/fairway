//! Startup defaults, atomic publication, and failures in isolated processes.

use std::{
    path::Path,
    process::Command,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use fairway_config::__private::initialize;
use serde::Deserialize;

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
    fn deserialize<D: serde::Deserializer<'de>>(input: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
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
    fairway_config::__private::assert_valid();
    assert_eq!(PREPARES.load(Ordering::SeqCst), 0);
    match mode {
        "defaults" => {
            initialize().await.unwrap();
            assert_eq!(ALPHA.get().value, 5);
            assert_eq!(BETA.get().value, 5);
            assert!(!std::ptr::eq(ALPHA.get(), BETA.get()));
            assert!(!home.join("config.toml").exists());
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
        if matches!(mode.as_str(), "defaults" | "success") {
            assert!(ALPHA.get().value > 0);
        }
        return;
    }
    for mode in ["defaults", "success", "failure", "reentrant"] {
        let home = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process_configuration", "--nocapture"])
            .env("FAIRWAY_CONFIG_TEST_MODE", mode)
            .env("FAIRWAY_HOME", home.path())
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
