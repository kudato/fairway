//! Filesystem error context and preservation of original causes.

use std::{error::Error as _, io, path::Path};

use fairway_codec::{Decode, Json};
use fairway_fs::{self as fs, Operation};

#[tokio::test]
async fn native_errors_preserve_the_cause_and_share_it_when_cloned() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("absent");
    let native = std::fs::File::open(&path).unwrap_err();
    let error = fs::read::<Vec<u8>>(&path).await.unwrap_err();
    assert_eq!(error.operation(), Operation::Open);
    assert_eq!(error.path(), Some(path.as_path()));
    assert_eq!(error.kind(), native.kind());
    assert_eq!(error.raw_os_error(), native.raw_os_error());
    let source = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(source.raw_os_error(), native.raw_os_error());
    assert_eq!(source.to_string(), native.to_string());
    assert!(error.to_string().contains(path.to_str().unwrap()));
    assert!(!error.to_string().contains(&native.to_string()));

    let cloned = error.clone();
    assert!(std::ptr::eq(
        error.source().unwrap(),
        cloned.source().unwrap()
    ));
    drop(error);
    assert_eq!(cloned.raw_os_error(), native.raw_os_error());
    Ok(())
}

#[tokio::test]
async fn codec_errors_are_direct_sources_with_the_parser_chain_intact() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data.json");
    std::fs::write(&path, b"[1,\n x]")?;
    let error = fs::read::<Json<Vec<u32>>>(&path).await.unwrap_err();
    assert_eq!(error.operation(), Operation::Decode);
    assert_eq!(error.path(), Some(path.as_path()));
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(error.raw_os_error(), None);
    let codec = error
        .source()
        .unwrap()
        .downcast_ref::<fairway_codec::Error>()
        .unwrap();
    assert!(matches!(
        codec,
        fairway_codec::Error::Decode {
            format: "json",
            line: Some(2),
            column: Some(2),
            ..
        }
    ));
    assert!(codec.source().unwrap().is::<fairway_codec::json::Error>());
    Ok(())
}

#[tokio::test]
async fn custom_decoder_errors_keep_their_type_and_data() -> anyhow::Result<()> {
    #[derive(Debug)]
    struct Rejected(Vec<u8>);

    impl std::fmt::Display for Rejected {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("custom format rejected")
        }
    }
    impl std::error::Error for Rejected {}

    #[derive(Debug)]
    struct Document;

    impl Decode for Document {
        type Error = Rejected;

        fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
            Err(Rejected(bytes))
        }
    }

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, b"original bytes")?;
    let error = fs::read::<Document>(&path).await.unwrap_err();
    assert_eq!(error.operation(), Operation::Decode);
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<Rejected>()
            .unwrap()
            .0,
        b"original bytes"
    );
    Ok(())
}

#[tokio::test]
async fn own_validation_errors_have_context_without_an_artificial_source() -> anyhow::Result<()> {
    let empty = fs::read::<Vec<u8>>("").await.unwrap_err();
    assert_eq!(empty.operation(), Operation::ResolvePath);
    assert_eq!(empty.path(), Some(Path::new("")));
    assert_eq!(empty.kind(), io::ErrorKind::NotFound);
    assert!(empty.source().is_none());
    assert_eq!(empty.raw_os_error(), None);
    assert!(empty.to_string().contains("empty filesystem path"));

    let directory = tempfile::tempdir()?;
    let path = std::fs::canonicalize(directory.path())?;
    let error = fs::write(&path, b"cannot replace a directory".to_vec())
        .await
        .unwrap_err();
    assert_eq!(error.operation(), Operation::ValidateTarget);
    assert_eq!(error.path(), Some(path.as_path()));
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(error.source().is_none());
    assert_eq!(error.raw_os_error(), None);
    assert!(error.to_string().contains("regular file"));
    Ok(())
}

#[tokio::test]
async fn a_busy_error_identifies_the_target_and_has_no_lower_level_cause() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let path = std::fs::canonicalize(directory.path())?.join("data");
    std::fs::write(&path, b"old")?;
    fs::edit(&path, |text: String| async {
        let error = fs::write(&path, "conflict".to_owned()).await.unwrap_err();
        assert_eq!(error.operation(), Operation::Lock);
        assert_eq!(error.path(), Some(path.as_path()));
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(error.source().is_none());
        Ok::<_, fs::Error>(text)
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn directory_operations_keep_their_operation_and_path() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("absent");
    let metadata = fs::metadata(&missing).await.unwrap_err();
    let listing = fs::ls(&missing).await.err().unwrap();
    let canonical = fs::canonicalize(&missing).await.unwrap_err();
    for (error, operation) in [
        (metadata, Operation::Metadata),
        (listing, Operation::ReadDirectory),
        (canonical, Operation::Canonicalize),
    ] {
        assert_eq!(error.operation(), operation);
        assert_eq!(error.path(), Some(missing.as_path()));
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.source().unwrap().is::<io::Error>());
    }
    assert!(!fs::exists(&missing).await?);

    let path = directory.path().join("file");
    std::fs::write(&path, b"data")?;
    let error = fs::mkdir(&path).await.unwrap_err();
    assert_eq!(error.operation(), Operation::CreateDirectory);
    assert_eq!(error.path(), Some(path.as_path()));
    assert!(error.source().unwrap().is::<io::Error>());
    Ok(())
}

#[tokio::test]
async fn closing_a_temporary_file_reports_the_path_and_native_error() -> anyhow::Result<()> {
    let file = fs::temp_file().await?;
    let path = file.path().to_owned();
    std::fs::remove_file(&path)?;
    std::fs::create_dir(&path)?;
    let result = file.close().await;
    std::fs::remove_dir(&path)?;
    let error = result.unwrap_err();
    assert_eq!(error.operation(), Operation::RemoveFile);
    assert_eq!(error.path(), Some(path.as_path()));
    assert!(error.source().unwrap().is::<io::Error>());
    Ok(())
}

#[tokio::test]
async fn edit_converts_filesystem_errors_into_the_handlers_error_type() -> anyhow::Result<()> {
    #[derive(Debug)]
    enum ApplicationError {
        File(fs::Error),
        Handler(std::rc::Rc<u32>),
    }

    impl From<fs::Error> for ApplicationError {
        fn from(error: fs::Error) -> Self {
            Self::File(error)
        }
    }

    let directory = tempfile::tempdir()?;
    let path = directory.path().join("data");
    std::fs::write(&path, [0xff])?;
    let error = fs::edit(&path, |text: String| async move {
        Ok::<_, ApplicationError>(text)
    })
    .await
    .unwrap_err();
    let ApplicationError::File(error) = error else {
        panic!("expected a filesystem error")
    };
    assert_eq!(error.operation(), Operation::Decode);
    assert!(error.source().unwrap().is::<fairway_codec::Error>());

    std::fs::write(&path, "old")?;
    let error = fs::edit(&path, |_: String| async {
        Err::<String, _>(ApplicationError::Handler(std::rc::Rc::new(37)))
    })
    .await
    .unwrap_err();
    let ApplicationError::Handler(actual) = error else {
        panic!("expected the handler's error")
    };
    assert_eq!(*actual, 37);
    assert_eq!(std::fs::read(&path)?, b"old");
    Ok(())
}
