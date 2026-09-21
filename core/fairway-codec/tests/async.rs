//! The asynchronous API runs conversion work outside Tokio's execution pools.

use std::{
    borrow::Cow,
    convert::Infallible,
    sync::{Mutex, mpsc},
};

use fairway_codec::{self as codec, Decode, Encode, Json, Markdown, Toml};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

#[tokio::test]
async fn owned_async_conversions_cover_all_document_types() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Document {
        name: String,
    }
    let json: Json<Vec<u8>> = codec::decode("[1, 2]".to_owned()).await?;
    assert_eq!(codec::encode(json).await?, b"[1,2]\n");
    let toml: Toml<Document> = codec::decode(b"name = 'dataset'".to_vec()).await?;
    let bytes = codec::encode(toml).await?;
    let toml: Toml<Document> = codec::decode(bytes).await?;
    assert_eq!(toml.0.name, "dataset");
    let md: Markdown = codec::decode("# Heading\n\nText.".to_owned()).await?;
    assert_eq!(codec::encode(md).await?, b"# Heading\n\nText.");
    assert_eq!(codec::decode::<String>("кот").await?, "кот");
    assert_eq!(codec::decode::<Vec<u8>>([0, 255]).await?, [0, 255]);
    assert_eq!(codec::encode("кот").await?, "кот".as_bytes());
    assert_eq!(codec::encode(vec![0, 255]).await?, [0, 255]);
    assert!(codec::decode::<String>([255]).await.is_err());
    Ok(())
}

#[test]
fn a_busy_codec_does_not_occupy_tokio_workers_or_blocking_io_capacity() {
    struct GatedEncode {
        started: Mutex<Option<oneshot::Sender<()>>>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Encode for GatedEncode {
        type Error = Infallible;
        fn encode(&self) -> Result<Cow<'_, [u8]>, Infallible> {
            assert!(
                std::thread::current()
                    .name()
                    .unwrap()
                    .starts_with("fairway-codec-")
            );
            self.started
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Ok(Cow::Borrowed(b"done"))
        }
    }
    // Exactly one Tokio worker and one blocking slot. Neither can be consumed
    // by the gated codec if file I/O and async tasks are to make progress.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let (started, wait_started) = oneshot::channel();
        let (release, wait_release) = mpsc::channel();
        let work = tokio::spawn(codec::encode(GatedEncode {
            started: Mutex::new(Some(started)),
            release: Mutex::new(wait_release),
        }));
        wait_started.await.unwrap();
        let progress = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::task::spawn_blocking(|| 17),
        )
        .await;
        // Release even on a test failure so runtime shutdown cannot hang.
        release.send(()).unwrap();
        assert_eq!(progress.unwrap().unwrap(), 17);
        assert_eq!(work.await.unwrap().unwrap(), b"done");
    });
}

#[tokio::test]
async fn synchronous_traits_allow_borrowing_and_async_helpers_accept_custom_error_types() {
    struct Custom;
    impl Decode for Custom {
        type Error = u8;
        fn decode(_: &[u8]) -> Result<Self, u8> {
            Err(7)
        }
    }
    impl Encode for Custom {
        type Error = u8;
        fn encode(&self) -> Result<Cow<'_, [u8]>, u8> {
            Err(8)
        }
    }
    assert!(matches!(codec::decode::<Custom>("input").await, Err(7)));
    assert_eq!(codec::encode(Custom).await, Err(8));
    let local = String::from("borrowed");
    assert!(matches!(local.encode().unwrap(), Cow::Borrowed(_)));
}
