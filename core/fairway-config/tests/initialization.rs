//! Publication, concurrency, cancellation, and cached results in isolated processes.

use std::{
    path::Path,
    process::Command,
    sync::{
        Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use fairway_config::__private::initialize;
use serde::Deserialize;

static PREPARES: AtomicUsize = AtomicUsize::new(0);
static BLOCKED: AtomicBool = AtomicBool::new(false);
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
        if value == 100 && !BLOCKED.swap(true, Ordering::SeqCst) {
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
            let first = initialize().await.unwrap_err().to_string();
            assert!(first.contains("beta"));
            inaccessible();
            config(home, "20");
            assert_eq!(initialize().await.unwrap_err().to_string(), first);
            inaccessible();
        }
        "reentrant" => {
            config(home, "999");
            let task = tokio::spawn(initialize());
            assert!(task.await.unwrap_err().is_panic());
            inaccessible();
            config(home, "20");
            initialize().await.unwrap();
            assert_eq!(BETA.get().value, 20);
        }
        "success" | "cancel" => {
            STARTED.set(tokio::sync::Notify::new()).unwrap();
            config(home, "100");
            let task = tokio::spawn(initialize());
            tokio::time::timeout(Duration::from_secs(5), STARTED.get().unwrap().notified())
                .await
                .unwrap();
            // Alpha is already deserialized. Neither namespace is published yet.
            inaccessible();
            if mode == "cancel" {
                task.abort();
                assert!(task.await.unwrap_err().is_cancelled());
                config(home, "20");
                release();
                initialize().await.unwrap();
                assert_eq!(BETA.get().value, 20);
            } else {
                let waiters: Vec<_> = (0..8).map(|_| tokio::spawn(initialize())).collect();
                release();
                task.await.unwrap().unwrap();
                for waiter in waiters {
                    waiter.await.unwrap().unwrap();
                }
                assert_eq!(PREPARES.load(Ordering::SeqCst), 2);
                assert_eq!(BETA.get().value, 100);
            }
            let original = ALPHA.get();
            assert_eq!(original.value, 10);
            std::fs::remove_file(home.join("config.toml")).unwrap();
            initialize().await.unwrap();
            assert!(std::ptr::eq(original, ALPHA.get()));
        }
        _ => unreachable!(),
    }
}

#[test]
fn process_configuration() {
    if let Ok(mode) = std::env::var("FAIRWAY_CONFIG_TEST_MODE") {
        let home = std::path::PathBuf::from(std::env::var_os("FAIRWAY_HOME").unwrap());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(scenario(&mode, &home));
        drop(runtime);
        // Published values remain accessible independently of the original runtime.
        if mode != "failure" {
            assert!(ALPHA.get().value > 0);
        }
        return;
    }
    for mode in ["defaults", "success", "failure", "cancel", "reentrant"] {
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
