use std::{error, fmt, sync::Arc};

/// The kind of an [`Error`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// [`initialize`](crate::initialize) has not succeeded.
    NotInitialized,
    /// [`initialize`](crate::initialize) failed: the configuration, the
    /// catalog, or an override is invalid, or an override file cannot be read.
    Initialize,
    /// The address is invalid or of another kind: for example, a reference
    /// address passed to [`skill`](crate::skill).
    InvalidAddress,
    /// No skill or reference is stored at the address.
    NotFound,
    /// The template cannot be rendered with the given variables.
    Render,
}

/// An error of [`initialize`](crate::initialize) or of getting a skill or a
/// reference.
///
/// Messages show full addresses. An initialization error lists one problem
/// per line, each with its address and file. A render error keeps the
/// template error as its [`source`](error::Error::source).
#[derive(Clone, Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Arc<dyn error::Error + Send + Sync>>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: String) -> Self {
        Self {
            kind,
            message,
            source: None,
        }
    }

    pub(crate) fn caused_by(
        kind: ErrorKind,
        message: String,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message,
            source: Some(Arc::new(source)),
        }
    }

    /// Returns the kind of the error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn error::Error + 'static))
    }
}

/// A failure to check builtin skills and references or to write their build
/// artifacts.
///
/// Problems with the files are listed together, one per line, each with its
/// address and file. A failed file operation on the artifacts identifies the
/// path and retains the original error through
/// [`source`](error::Error::source).
///
/// [`Debug`] shows the message and its sources as text: a build script that
/// returns the error from `main` prints it this way.
#[cfg(feature = "build")]
pub struct BuildError {
    message: String,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

#[cfg(feature = "build")]
impl BuildError {
    pub(crate) fn new(message: String) -> Self {
        Self {
            message,
            source: None,
        }
    }

    pub(crate) fn caused_by(
        message: String,
        source: impl Into<Box<dyn error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            message,
            source: Some(source.into()),
        }
    }
}

#[cfg(feature = "build")]
impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(feature = "build")]
impl fmt::Debug for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&chain(self))
    }
}

#[cfg(feature = "build")]
impl error::Error for BuildError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn error::Error + 'static))
    }
}

/// The message of an error followed by the messages of its sources.
pub(crate) fn chain(error: &dyn error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

/// A message with a header and one problem per line; the further lines of a
/// problem are indented below it.
pub(crate) fn report(header: &str, problems: &[String]) -> String {
    let mut message = format!("{header}:");
    for problem in problems {
        message.push_str("\n  ");
        message.push_str(&problem.replace('\n', "\n    "));
    }
    message
}
