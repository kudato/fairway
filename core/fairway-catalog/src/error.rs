use std::{error, fmt, io, sync::Arc};

/// A catalog initialization, access, compression, or decompression failure.
///
/// The diagnostic identifies the entry or registration when applicable.
/// Compression and decompression errors retain their underlying cause through
/// [`source`](error::Error::source).
#[derive(Clone, Debug)]
pub struct Error {
    message: String,
    source: Option<Arc<io::Error>>,
}

impl Error {
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn with_source(message: impl Into<String>, source: io::Error) -> Self {
        Self {
            message: message.into(),
            source: Some(Arc::new(source)),
        }
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

/// A failure to compress builtin data or write its build artifacts.
///
/// File operation errors identify the affected path and retain the original
/// I/O error through [`source`](error::Error::source).
#[cfg(feature = "build")]
#[derive(Debug)]
pub struct BuildError(Error);

#[cfg(feature = "build")]
impl BuildError {
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self(Error::message(message))
    }

    pub(crate) fn with_source(message: impl Into<String>, source: io::Error) -> Self {
        Self(Error::with_source(message, source))
    }
}

#[cfg(feature = "build")]
impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(feature = "build")]
impl error::Error for BuildError {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.0.source()
    }
}
