use std::{env, fmt::Write as _, fs, path::Path};

use crate::{BuildError, compression};

/// Compresses builtin entries and prepares this consumer's embedding artifacts.
///
/// Call from the consumer's `build.rs` with the `build` feature enabled in its
/// build dependency. Each item is a full literal address and its original bytes.
/// Artifacts are written under that build script's `OUT_DIR`; [`crate::embed!`]
/// includes them from the consumer's source code.
///
/// The consumer reads its inputs and declares any corresponding Cargo rebuild
/// triggers. Addresses never become filesystem paths. A call prepares one
/// complete table, including an empty table if the iterator is empty.
///
/// ```no_run
/// // In the consumer's build.rs.
/// fairway_catalog::build([
///     ("skill/git/commit".to_owned(), b"commit instructions".to_vec()),
///     ("resource/git/convention".to_owned(), b"conventions".to_vec()),
/// ])?;
/// # Ok::<(), fairway_catalog::BuildError>(())
/// ```
///
/// # Errors
///
/// Returns an error if `OUT_DIR` is absent, compression fails, or an artifact
/// cannot be written. Propagate the failure from the build script to Cargo.
pub fn build(entries: impl IntoIterator<Item = (String, Vec<u8>)>) -> Result<(), BuildError> {
    let out_dir = env::var_os("OUT_DIR")
        .ok_or_else(|| BuildError::message("catalog build requires Cargo's OUT_DIR"))?;
    write(entries, Path::new(&out_dir))
}

fn write(
    entries: impl IntoIterator<Item = (String, Vec<u8>)>,
    out_dir: &Path,
) -> Result<(), BuildError> {
    let directory = out_dir.join("fairway-catalog");
    fs::create_dir_all(&directory).map_err(|source| {
        BuildError::with_source(
            format!(
                "cannot create catalog artifact directory {}",
                directory.display()
            ),
            source,
        )
    })?;

    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut generated = String::from("&[\n");

    for (index, (address, content)) in entries.into_iter().enumerate() {
        let compressed = compression::compress(&content).map_err(|source| {
            BuildError::with_source(
                format!("cannot compress builtin catalog entry {address:?}"),
                source,
            )
        })?;
        let filename = format!("{index:016x}.zst");
        let path = directory.join(&filename);
        fs::write(&path, compressed).map_err(|source| {
            BuildError::with_source(
                format!("cannot write catalog artifact {}", path.display()),
                source,
            )
        })?;
        writeln!(
            generated,
            "    ({address:?}, ::core::include_bytes!({filename:?}) as &'static [u8]),"
        )
        .expect("writing to a String cannot fail");
    }
    generated.push_str("]\n");

    let path = directory.join("entries.rs");
    fs::write(&path, generated).map_err(|source| {
        BuildError::with_source(
            format!("cannot write catalog artifact {}", path.display()),
            source,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::write;
    use std::{error::Error as _, fs};

    #[test]
    fn artifact_io_errors_retain_the_path_and_cause() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fairway-catalog");
        fs::write(&path, b"occupied by a file").unwrap();
        let error = write([], directory.path()).unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
        assert!(error.source().is_some());
    }
}
