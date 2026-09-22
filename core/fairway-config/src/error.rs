use std::{fmt, path::Path, sync::Arc};

/// A registration, filesystem, format, or settings validation error.
///
/// Display provides context; [`std::error::Error::source`] retains the original
/// I/O or deserialization error. Clones share the source of a cached failure.
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn caused_by(
        message: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(Arc::new(source)),
        }
    }

    pub(crate) fn file(path: &Path, source: std::io::Error) -> Self {
        Self::caused_by(format!("could not load {}", path.display()), source)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}
