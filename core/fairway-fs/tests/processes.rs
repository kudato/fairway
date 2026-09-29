//! Behavior that needs separate processes: locks shared between processes,
//! the cleanup at startup, the Fairway directory, which each process resolves
//! only once, process-wide umask changes, and operations that could hang.
//!
//! The tests run this test binary again as a child process, in which the
//! `child_process` test performs the action named by `FAIRWAY_FS_TEST_CHILD`.

use std::{
    io::{self, BufRead, Write},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use fairway_fs as fs;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// Performs the action named by `FAIRWAY_FS_TEST_CHILD` on the path in
/// `FAIRWAY_FS_TEST_TARGET` and exits; does nothing in a normal test run.
#[test]
fn child_process() {
    let Ok(action) = std::env::var("FAIRWAY_FS_TEST_CHILD") else {
        return;
    };
    let target = std::env::var_os("FAIRWAY_FS_TEST_TARGET").unwrap();
    let result = runtime().block_on(async {
        match action.as_str() {
            "hold" => {
                fs::edit(&target, |_: String| async move {
                    println!("HELD");
                    io::stdout().flush()?;
                    let mut line = String::new();
                    io::stdin().read_line(&mut line)?;
                    Ok::<_, anyhow::Error>("new".to_owned())
                })
                .await?;
            }
            "increment" => {
                for _ in 0..6 {
                    fs::edit(&target, |value: String| async move {
                        let number: u32 = value.parse().map_err(io::Error::other)?;
                        tokio::task::yield_now().await;
                        Ok::<_, anyhow::Error>((number + 1).to_string())
                    })
                    .await?;
                }
            }
            "initialize" => fs::__private::initialize()?,
            "reject-non-regular" => {
                assert_eq!(
                    fs::read::<Vec<u8>>(&target).await.unwrap_err().kind(),
                    io::ErrorKind::InvalidInput
                );
            }
            "home" => {
                fs::__private::initialize()?;
                std::env::set_current_dir(&target)?;
                println!("HOME:{}", fs::home().await?.display());
            }
            #[cfg(unix)]
            "temp-dir-permissions" => {
                use std::os::unix::fs::PermissionsExt;

                let mask =
                    libc::mode_t::from_str_radix(&std::env::var("FAIRWAY_FS_TEST_UMASK")?, 8)?;
                // SAFETY: this child runs only this test. Changing its umask
                // cannot affect file creation in the parent or other tests.
                #[allow(unsafe_code)]
                unsafe {
                    libc::umask(mask);
                }
                let directory = fs::temp_dir().await?;
                assert_eq!(
                    directory.path().metadata()?.permissions().mode() & 0o777,
                    0o700,
                    "temporary directories must be private to their owner"
                );
                directory.close().await?;
            }
            #[cfg(unix)]
            "home-error" => {
                use std::error::Error as _;

                std::fs::remove_dir(std::env::current_dir()?)?;
                let first = fs::home().await.unwrap_err();
                let source = first.source().unwrap().downcast_ref::<io::Error>().unwrap();
                assert!(source.raw_os_error().is_some());
                assert_eq!(first.raw_os_error(), source.raw_os_error());
                std::env::set_current_dir(&target)?;
                let second = fs::home().await.unwrap_err();
                assert_eq!(first.operation(), second.operation());
                assert_eq!(first.path(), second.path());
                assert_eq!(first.kind(), second.kind());
                assert_eq!(first.raw_os_error(), second.raw_os_error());
                assert!(std::ptr::eq(
                    first.source().unwrap(),
                    second.source().unwrap()
                ));
            }
            _ => panic!("unknown child action"),
        }
        Ok::<_, anyhow::Error>(())
    });
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
    std::process::exit(0);
}

/// Returns a command that performs `action` on `target` in a child process
/// whose Fairway directory is `home`.
fn child(action: &str, target: &Path, home: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "child_process", "--nocapture"])
        .env("FAIRWAY_FS_TEST_CHILD", action)
        .env("FAIRWAY_FS_TEST_TARGET", target)
        .env("FAIRWAY_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// A child process that is killed when dropped, so that a failing test does
/// not leave it running.
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Running {
    /// Waits up to 15 seconds for the process to exit and checks that it
    /// succeeded.
    fn wait(&mut self) {
        let start = Instant::now();
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "child exited with {status}");
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "child did not finish"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Waits up to 10 seconds for a `hold` process to report, from its edit
    /// handler, that it holds the lock.
    fn held(&mut self) {
        let stdout = self.0.stdout.take().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in io::BufReader::new(stdout).lines() {
                if line.unwrap() == "HELD" {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("child must acquire the lock");
    }
}

#[cfg(unix)]
#[test]
fn temporary_directories_are_private_under_different_umasks() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let home = directory.path().join("home");
    for mask in ["000", "022", "077"] {
        let output = child("temp-dir-permissions", directory.path(), &home)
            .env("FAIRWAY_FS_TEST_UMASK", mask)
            .output()?;
        assert!(
            output.status.success(),
            "umask {mask}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn read_rejects_non_regular_files_without_waiting_for_a_writer() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir()?;
    let home = directory.path().join("home");
    let pipe = directory.path().join("pipe");
    assert!(Command::new("mkfifo").arg(&pipe).status()?.success());
    let pipe_link = directory.path().join("pipe-link");
    symlink(&pipe, &pipe_link)?;
    let device_link = directory.path().join("device-link");
    symlink("/dev/null", &device_link)?;

    // Each read runs in a child process: if it hangs in `open`, the process can
    // be killed, whereas a hung blocking thread in this process could not be
    // stopped.
    for path in [
        pipe.as_path(),
        pipe_link.as_path(),
        Path::new("/dev/null"),
        device_link.as_path(),
        directory.path(),
    ] {
        let mut process = Running(child("reject-non-regular", path, &home).spawn()?);
        process.wait();
    }
    Ok(())
}

#[test]
fn another_process_cannot_write_until_the_first_process_finishes() -> anyhow::Result<()> {
    let runtime = runtime();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    let home = runtime.block_on(fs::home())?;
    std::fs::write(&path, "old")?;
    let mut writer = Running(child("hold", &path, &home).spawn()?);
    writer.held();
    runtime.block_on(async {
        assert_eq!(fs::read::<String>(&path).await?, "old");
        assert_eq!(
            fs::write(&path, "conflict".to_owned())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(80),
                fs::edit(
                    &path,
                    |text: String| async move { Ok::<_, anyhow::Error>(text) }
                )
            )
            .await
            .is_err()
        );
        Ok::<_, anyhow::Error>(())
    })?;
    writer.0.stdin.as_mut().unwrap().write_all(b"finish\n")?;
    writer.wait();
    assert_eq!(std::fs::read_to_string(&path)?, "new");
    runtime.block_on(fs::write(&path, "after".to_owned()))?;
    Ok(())
}

#[test]
fn concurrent_process_edits_do_not_lose_updates() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("counter");
    let home = directory.path().join("home");
    std::fs::write(&path, "0")?;
    let mut children = Vec::new();
    for _ in 0..5 {
        children.push(Running(child("increment", &path, &home).spawn()?));
    }
    for process in &mut children {
        process.wait();
    }
    assert_eq!(std::fs::read_to_string(path)?, "30");
    assert_eq!(
        std::fs::read_dir(home.join("locks"))?.count(),
        1,
        "only the permanent coordination lock remains"
    );
    Ok(())
}

#[test]
fn startup_keeps_active_locks_and_removes_crash_leftovers() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    let home = directory.path().join("home");
    std::fs::write(&path, "old")?;
    let mut writer = Running(child("hold", &path, &home).spawn()?);
    writer.held();
    // The coordination lock and the lock of `path`.
    assert_eq!(std::fs::read_dir(home.join("locks"))?.count(), 2);
    let mut initialize = Running(child("initialize", &path, &home).spawn()?);
    initialize.wait();
    assert_eq!(std::fs::read_dir(home.join("locks"))?.count(), 2);
    writer.0.kill()?;
    writer.0.wait()?;
    let mut initialize = Running(child("initialize", &path, &home).spawn()?);
    initialize.wait();
    assert_eq!(std::fs::read_dir(home.join("locks"))?.count(), 1);
    assert_eq!(std::fs::read_to_string(path)?, "old");
    Ok(())
}

#[test]
fn home_is_fixed_before_later_cwd_changes_and_is_not_created() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let other = tempfile::tempdir()?;
    let output = child("home", other.path(), Path::new("relative-home"))
        .current_dir(directory.path())
        .output()?;
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let actual = std::path::PathBuf::from(
        stdout
            .lines()
            .find_map(|line| line.strip_prefix("HOME:"))
            .expect("home output"),
    );
    assert_eq!(actual.file_name().unwrap(), "relative-home");
    assert_eq!(
        std::fs::canonicalize(actual.parent().unwrap())?,
        std::fs::canonicalize(directory.path())?
    );
    assert!(!actual.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn home_caches_the_original_os_error_without_losing_its_cause() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let cwd = directory.path().join("cwd");
    std::fs::create_dir(&cwd)?;
    let output = child("home-error", directory.path(), Path::new("relative-home"))
        .current_dir(&cwd)
        .output()?;
    assert!(output.status.success(), "{output:?}");
    Ok(())
}
