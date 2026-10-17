//! Builtin tables written by build scripts for [`embed!`](crate::embed).

use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

use crate::{BuildError, address};

/// Checks one namespace's builtin table and writes it for [`embed!`](crate::embed).
///
/// Call from a build script with the `build` feature enabled in the build
/// dependency; owners wrap this in their own build helper. Each item is an
/// address within the namespace and its bytes, already encoded by the owner.
/// The crate then includes the table with `embed!` and the same namespace name.
/// A crate can prepare tables for several namespaces.
///
/// The table is written under `OUT_DIR/fairway-catalog/<namespace>/` and
/// replaces the one from the previous build; addresses never become file
/// names. The caller reads its inputs and declares Cargo rebuild triggers.
///
/// ```no_run
/// // In an owner's build helper, called from a crate's build.rs.
/// fairway_catalog::build("alpha-items", [
///     ("lorem/ipsum".to_owned(), b"encoded lorem".to_vec()),
///     ("lorem/ipsum/elit".to_owned(), b"encoded elit".to_vec()),
/// ])?;
/// # Ok::<(), fairway_catalog::BuildError>(())
/// ```
///
/// # Errors
///
/// Returns an error listing every invalid or repeated address, or if the
/// namespace name is invalid, `OUT_DIR` is absent, or a file operation fails.
/// Propagate it from the build script to Cargo.
pub fn build(
    namespace: &str,
    entries: impl IntoIterator<Item = (String, Vec<u8>)>,
) -> Result<(), BuildError> {
    let mut entries: Vec<_> = entries.into_iter().collect();
    entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    check(namespace, &entries)?;
    let out_dir = env::var_os("OUT_DIR")
        .ok_or_else(|| BuildError::new("catalog build requires Cargo's OUT_DIR".to_owned()))?;
    write(namespace, &entries, Path::new(&out_dir))
}

fn check(namespace: &str, entries: &[(String, Vec<u8>)]) -> Result<(), BuildError> {
    if let Err(reason) = address::check_name(namespace) {
        return Err(BuildError::new(format!(
            "cannot build catalog namespace {namespace:?}: {reason}"
        )));
    }
    let mut problems = Vec::new();
    for group in entries.chunk_by(|left, right| left.0 == right.0) {
        let address = address::full(namespace, &group[0].0);
        if let Err(reason) = address::check(&group[0].0) {
            problems.push(format!("address {address:?} {reason}"));
        }
        if group.len() > 1 {
            problems.push(format!("duplicate address {address:?}"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(BuildError::new(format!(
            "cannot build catalog namespace {namespace:?}:\n  {}",
            problems.join("\n  ")
        )))
    }
}

fn write(namespace: &str, entries: &[(String, Vec<u8>)], out_dir: &Path) -> Result<(), BuildError> {
    // Joined as text, like the path that `embed!` includes.
    let mut directory = OsString::from(out_dir);
    directory.push("/fairway-catalog/");
    directory.push(namespace);
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory).map_err(|source| {
        BuildError::with_source(
            format!(
                "cannot create catalog artifact directory {}",
                directory.display()
            ),
            source,
        )
    })?;

    let mut generated = String::from("&[\n");
    for (index, (address, content)) in entries.iter().enumerate() {
        let filename = format!("{index:016x}");
        write_file(&directory.join(&filename), content)?;
        writeln!(
            generated,
            "    ({address:?}, ::core::include_bytes!({filename:?}) as &'static [u8]),"
        )
        .expect("writing to a String cannot fail");
    }
    generated.push_str("]\n");
    write_file(&directory.join("entries.rs"), generated.as_bytes())?;

    // Files past this table remain from a previous, larger one.
    let mut index = entries.len();
    loop {
        let path = directory.join(format!("{index:016x}"));
        match fs::remove_file(&path) {
            Ok(()) => index += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(BuildError::with_source(
                    format!("cannot remove catalog artifact {}", path.display()),
                    source,
                ));
            }
        }
    }
}

fn write_file(path: &Path, contents: &[u8]) -> Result<(), BuildError> {
    fs::write(path, contents).map_err(|source| {
        BuildError::with_source(
            format!("cannot write catalog artifact {}", path.display()),
            source,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{check, write};
    use std::{error::Error as _, fs, path::Path};

    fn table(addresses: &[&str]) -> Vec<(String, Vec<u8>)> {
        addresses
            .iter()
            .map(|address| (address.to_string(), address.as_bytes().to_vec()))
            .collect()
    }

    fn files(directory: &Path) -> Vec<String> {
        let mut files: Vec<_> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        files
    }

    #[test]
    fn a_table_reports_every_invalid_and_repeated_address() {
        let error = check(
            "alpha-items",
            &table(&["a//b", "lorem/*", "lorem/x", "lorem/x", "lorem/x"]),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "cannot build catalog namespace \"alpha-items\":\n  \
             address \"alpha-items/a//b\" has an empty part\n  \
             address \"alpha-items/lorem/*\" contains `*`\n  \
             duplicate address \"alpha-items/lorem/x\""
        );
        assert_eq!(
            check("alpha-items/lorem", &[]).unwrap_err().to_string(),
            "cannot build catalog namespace \"alpha-items/lorem\": \
             a catalog namespace name cannot contain `/`"
        );
        check("alpha-items", &table(&["lorem/ipsum", "../escape.rs"])).unwrap();
    }

    #[test]
    fn a_smaller_table_removes_the_files_left_from_the_previous_one() {
        let out_dir = tempfile::tempdir().unwrap();
        let alpha = out_dir.path().join("fairway-catalog/alpha-items");
        let beta = out_dir.path().join("fairway-catalog/beta-items");
        write("alpha-items", &table(&["a", "b", "c"]), out_dir.path()).unwrap();
        write("beta-items", &table(&["a", "b"]), out_dir.path()).unwrap();

        write("alpha-items", &table(&["z"]), out_dir.path()).unwrap();
        assert_eq!(files(&alpha), ["0000000000000000", "entries.rs"]);
        assert_eq!(fs::read(alpha.join("0000000000000000")).unwrap(), b"z");
        assert_eq!(
            fs::read_to_string(alpha.join("entries.rs")).unwrap(),
            "&[\n    (\"z\", ::core::include_bytes!(\"0000000000000000\") as &'static [u8]),\n]\n"
        );
        assert_eq!(
            files(&beta),
            ["0000000000000000", "0000000000000001", "entries.rs"]
        );

        write("alpha-items", &[], out_dir.path()).unwrap();
        assert_eq!(files(&alpha), ["entries.rs"]);
    }

    #[test]
    fn artifact_io_errors_retain_the_path_and_cause() {
        let out_dir = tempfile::tempdir().unwrap();
        let path = out_dir.path().join("fairway-catalog");
        fs::write(&path, b"occupied by a file").unwrap();
        let error = write("alpha-items", &[], out_dir.path()).unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
        assert!(error.source().is_some());
    }
}
