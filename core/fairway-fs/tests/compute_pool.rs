//! This test occupies the process-wide compute pool and needs its own test binary.

use std::{convert::Infallible, io, sync::mpsc, time::Duration};

use fairway_codec::Encode;
use fairway_fs as fs;
use tokio::sync::oneshot;

struct BusyCodec {
    started: oneshot::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl Encode for BusyCodec {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let _ = self.started.send(());
        // Dropping the sender also releases the worker if the test fails.
        let _ = self.release.recv();
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn byte_operations_finish_while_the_compute_pool_is_busy() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    let capacity = std::thread::available_parallelism().map_or(1, usize::from);
    let mut workers = Vec::new();
    let mut releases = Vec::new();
    let mut starts = Vec::new();
    for _ in 0..capacity {
        let (started, wait_started) = oneshot::channel();
        let (release, wait_release) = mpsc::channel();
        workers.push(tokio::spawn(fairway_codec::encode(BusyCodec {
            started,
            release: wait_release,
        })));
        releases.push(release);
        starts.push(wait_started);
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        for started in starts {
            started.await.unwrap();
        }
    })
    .await?;

    let progress = tokio::time::timeout(Duration::from_secs(10), async {
        fs::write(&path, vec![0, 255]).await?;
        assert_eq!(fs::read::<Vec<u8>>(&path).await?, [0, 255]);

        let mut output = fs::writer(&path).await?;
        output.write(vec![1, 255]).await?;
        output.finish().await?;
        assert_eq!(fs::read::<Vec<u8>>(&path).await?, [1, 255]);

        let mut editor = fs::editor(&path).await?;
        editor.write(vec![2, 255]).await?;
        editor.finish().await?;
        assert_eq!(fs::read::<Vec<u8>>(&path).await?, [2, 255]);

        fs::edit(&path, |mut bytes: Vec<u8>| async move {
            bytes.push(3);
            Ok::<_, io::Error>(bytes)
        })
        .await?;
        assert_eq!(fs::read::<Vec<u8>>(&path).await?, [2, 255, 3]);
        Ok::<_, io::Error>(())
    })
    .await;

    // Release every worker before reporting a failure or shutting down the runtime.
    drop(releases);
    for worker in workers {
        worker.await.unwrap().unwrap();
    }
    progress.expect("byte operations must not wait for compute capacity")
}
