use std::{
    error, fmt, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::BoxError;

/// The step that failed, as reported by [`Error::operation`].
///
/// A single call can fail at different steps: [`write`](crate::write), for
/// example, may fail to [`Lock`](Operation::Lock) the path,
/// [`Encode`](Operation::Encode) the value, or [`Replace`](Operation::Replace)
/// the file. The step is part of the error's message; match on it to react
/// to a particular step. New steps may be added in later versions, so such a
/// `match` needs a wildcard arm.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum Operation {
    /// Making a path absolute; fails for an empty path or if the working
    /// directory is unavailable.
    ResolvePath,
    /// Checking that the target is acceptable, for example that
    /// [`read`](crate::read) is given a regular file or that
    /// [`write`](crate::write) is given a path that names a file.
    ValidateTarget,
    /// Opening a file, or duplicating the handle of an open one.
    Open,
    /// Reading the contents of a file.
    Read,
    /// Decoding the contents of a file; the codec's error is the source.
    Decode,
    /// Encoding a value before it is written; the codec's error is the
    /// source.
    Encode,
    /// Acquiring the lock on a path or maintaining the lock files; a path
    /// locked by another Fairway operation is reported with this step and
    /// the kind [`WouldBlock`](io::ErrorKind::WouldBlock).
    Lock,
    /// Creating a temporary file, either with [`temp_file`](crate::temp_file)
    /// or next to the target of a replacement.
    CreateTemporaryFile,
    /// Creating a temporary directory with [`temp_dir`](crate::temp_dir).
    CreateTemporaryDirectory,
    /// Writing the new contents to the temporary file of a replacement.
    Write,
    /// Copying the metadata of the file being replaced to its replacement.
    CopyMetadata,
    /// Renaming the temporary file over the target.
    Replace,
    /// Creating a directory and its missing parents with
    /// [`mkdir`](crate::mkdir).
    CreateDirectory,
    /// Opening a directory with [`ls`](crate::ls) or reading its next entry.
    ReadDirectory,
    /// Querying the metadata of a file, a directory, or a directory entry.
    Metadata,
    /// Resolving symbolic links in a path, either with
    /// [`canonicalize`](crate::canonicalize) or in the parent directories of
    /// the target of a replacement; a missing parent directory is reported
    /// with this step.
    Canonicalize,
    /// Removing a temporary file with [`TempFile::close`](crate::TempFile::close).
    RemoveFile,
    /// Removing a temporary directory with
    /// [`TempDir::close`](crate::TempDir::close).
    RemoveDirectory,
    /// Determining the Fairway directory; see [`home`](fn@crate::home).
    ResolveHome,
}

impl Operation {
    /// Returns the verb phrase that the error message uses: "cannot
    /// {description} {path}".
    fn description(self) -> &'static str {
        match self {
            Self::ResolvePath => "resolve path",
            Self::ValidateTarget => "validate target",
            Self::Open => "open",
            Self::Read => "read",
            Self::Decode => "decode",
            Self::Encode => "encode",
            Self::Lock => "lock",
            Self::CreateTemporaryFile => "create temporary file",
            Self::CreateTemporaryDirectory => "create temporary directory",
            Self::Write => "write",
            Self::CopyMetadata => "copy metadata for",
            Self::Replace => "replace",
            Self::CreateDirectory => "create directory",
            Self::ReadDirectory => "read directory",
            Self::Metadata => "read metadata for",
            Self::Canonicalize => "canonicalize",
            Self::RemoveFile => "remove file",
            Self::RemoveDirectory => "remove directory",
            Self::ResolveHome => "resolve Fairway home",
        }
    }
}

/// An error returned by the operations of this crate.
///
/// An error records the [`Operation`] that failed and the path it concerned,
/// and [`kind`](Error::kind) classifies the failure with an
/// [`io::ErrorKind`]. Its [`source`](std::error::Error::source) is the
/// underlying cause, such as the [`io::Error`] reported by the operating
/// system or the error of a codec. A failure that this crate detects itself,
/// such as a path that does not name a regular file, has no source.
///
/// The message names the step and the path, for example
/// `cannot open /data/notes.txt`, and adds an explanation if there is no
/// source. It does not repeat the source, so print the whole chain of
/// sources to show the cause, as the alternate format `{:#}` of
/// `anyhow::Error` does.
///
/// Cloning an error is cheap: clones share the same cause.
///
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() -> Result<(), fairway_fs::Error> {
/// use std::{error::Error as _, io::ErrorKind};
///
/// use fairway_fs::Operation;
///
/// let directory = fairway_fs::temp_dir().await?;
/// let path = directory.path().join("missing.txt");
///
/// let error = fairway_fs::read::<String>(&path).await.unwrap_err();
/// assert_eq!(error.operation(), Operation::Open);
/// assert_eq!(error.kind(), ErrorKind::NotFound);
/// assert_eq!(error.path(), Some(path.as_path()));
/// assert!(error.source().unwrap().is::<std::io::Error>());
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Error(Arc<Details>);

/// The contents of an [`Error`], shared between its clones.
#[derive(Debug)]
struct Details {
    operation: Operation,
    path: Option<PathBuf>,
    cause: Cause,
}

/// What caused an [`Error`]; it determines the error's kind and source.
#[derive(Debug)]
pub(crate) enum Cause {
    /// A failure reported by the operating system. The error is the source,
    /// and its kind is the kind of the [`Error`].
    Io(io::Error),
    /// A failure reported by a codec or by the runtime, with the kind that
    /// this crate assigns to it. The error is the source.
    External(io::ErrorKind, BoxError),
    /// A failure detected by this crate itself, with its kind and an
    /// explanation for the message. There is no source.
    Message(io::ErrorKind, &'static str),
}

impl From<io::Error> for Cause {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl Error {
    /// Creates an error from its parts.
    pub(crate) fn new(operation: Operation, path: Option<&Path>, cause: Cause) -> Self {
        Self(Arc::new(Details {
            operation,
            path: path.map(Path::to_owned),
            cause,
        }))
    }

    /// Creates an error for a failure reported by the operating system.
    pub(crate) fn io(operation: Operation, path: Option<&Path>, source: io::Error) -> Self {
        Self::new(operation, path, Cause::Io(source))
    }

    /// Creates an error for a failure reported by a codec or by the runtime.
    pub(crate) fn external(
        operation: Operation,
        path: Option<&Path>,
        kind: io::ErrorKind,
        source: impl Into<BoxError>,
    ) -> Self {
        Self::new(operation, path, Cause::External(kind, source.into()))
    }

    /// Creates an error for a failure detected by this crate itself.
    pub(crate) fn message(
        operation: Operation,
        path: Option<&Path>,
        kind: io::ErrorKind,
        message: &'static str,
    ) -> Self {
        Self::new(operation, path, Cause::Message(kind, message))
    }

    /// Returns the step that failed.
    pub fn operation(&self) -> Operation {
        self.0.operation
    }

    /// Returns the path that the failed step concerned, or `None` if it
    /// concerned no particular path, as when a temporary file cannot be
    /// created.
    ///
    /// The path is absolute, except for an error of
    /// [`ResolvePath`](Operation::ResolvePath), which reports the path as it
    /// was passed. Once [`write`](crate::write) or [`edit`](crate::edit) has
    /// resolved the symbolic links in the parent directories of its target,
    /// its errors report the resolved path, which may differ from the path
    /// passed in.
    pub fn path(&self) -> Option<&Path> {
        self.0.path.as_deref()
    }

    /// Returns the kind of the failure.
    ///
    /// If the operating system reported the failure, this is the kind that it
    /// reported. Otherwise it is the kind that this crate assigns, such as
    /// [`InvalidData`](io::ErrorKind::InvalidData) for a file that cannot be
    /// decoded; the documentation of each operation lists the kinds it uses.
    pub fn kind(&self) -> io::ErrorKind {
        match &self.0.cause {
            Cause::Io(source) => source.kind(),
            Cause::External(kind, _) | Cause::Message(kind, _) => *kind,
        }
    }

    /// Returns the operating system's error code if the operating system
    /// reported the failure, and `None` otherwise.
    ///
    /// See [`io::Error::raw_os_error`].
    pub fn raw_os_error(&self) -> Option<i32> {
        match &self.0.cause {
            Cause::Io(source) => source.raw_os_error(),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    // The source stays out of the message: reporters that print the chain of
    // sources would otherwise show it twice.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot {}", self.operation().description())?;
        if let Some(path) = self.path() {
            write!(f, " {}", path.display())?;
        }
        if let Cause::Message(_, message) = &self.0.cause {
            write!(f, ": {message}")?;
        }
        Ok(())
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.0.cause {
            Cause::Io(source) => Some(source),
            Cause::External(_, source) => Some(source.as_ref()),
            Cause::Message(_, _) => None,
        }
    }
}
