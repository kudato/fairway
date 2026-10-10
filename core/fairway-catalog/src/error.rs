use std::{error, fmt};

#[cfg(feature = "build")]
use std::io;

/// An invalid address, or every problem found by [`initialize`](crate::initialize).
///
/// Messages show full addresses, `namespace/address`. An initialization error
/// lists one problem per line, with the declarations and `embed!` calls involved.
#[derive(Clone, Debug)]
pub struct Error {
    message: String,
}

impl Error {
    pub(crate) fn new(message: String) -> Self {
        Self { message }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {}

/// A failure to check a builtin table or to write its build artifacts.
///
/// File operation errors identify the affected path and retain the original
/// I/O error through [`source`](error::Error::source).
#[cfg(feature = "build")]
#[derive(Debug)]
pub struct BuildError {
    message: String,
    source: Option<io::Error>,
}

#[cfg(feature = "build")]
impl BuildError {
    pub(crate) fn new(message: String) -> Self {
        Self {
            message,
            source: None,
        }
    }

    pub(crate) fn with_source(message: String, source: io::Error) -> Self {
        Self {
            message,
            source: Some(source),
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
impl error::Error for BuildError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn error::Error + 'static))
    }
}
