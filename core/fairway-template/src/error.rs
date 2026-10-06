use std::error::Error as StdError;
use std::fmt;

use fairway_codec::json;

/// The category of a template error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// The template name is empty or contains a control character.
    InvalidName,
    /// The template source has invalid syntax.
    Syntax,
    /// Serialization failed or the context root is neither an object nor null.
    Data,
    /// MiniJinja classified the failure as an undefined value error.
    Undefined,
    /// Another evaluation failure, such as an unknown filter or invalid operand.
    Render,
}

/// An owned diagnostic from template compilation or rendering.
///
/// The display message includes the template name, the line when known, and the
/// reason for the failure. Underlying errors are retained through
/// [`std::error::Error::source`].
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    template_name: String,
    line: Option<usize>,
    detail: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl Error {
    /// Returns the category of the error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Returns the name supplied when compiling the template.
    pub fn template_name(&self) -> &str {
        &self.template_name
    }

    /// Returns the one-based line in the template source, when known.
    ///
    /// Invalid names and context errors are not associated with a source line.
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    pub(crate) fn invalid_name(name: String, detail: &str) -> Self {
        Self {
            kind: ErrorKind::InvalidName,
            template_name: name,
            line: None,
            detail: detail.into(),
            source: None,
        }
    }

    pub(crate) fn context_type(name: &str, value_type: &str) -> Self {
        Self {
            kind: ErrorKind::Data,
            template_name: name.into(),
            line: None,
            detail: format!("context must serialize to an object or null, got {value_type}"),
            source: None,
        }
    }

    pub(crate) fn serialization(name: &str, source: json::Error) -> Self {
        Self {
            kind: ErrorKind::Data,
            template_name: name.into(),
            line: None,
            detail: source.to_string(),
            source: Some(Box::new(source)),
        }
    }

    pub(crate) fn syntax(name: &str, source: minijinja::Error) -> Self {
        Self::engine(ErrorKind::Syntax, name, source)
    }

    pub(crate) fn render(name: &str, source: minijinja::Error) -> Self {
        Self::engine(ErrorKind::Render, name, source)
    }

    fn engine(mut kind: ErrorKind, name: &str, source: minijinja::Error) -> Self {
        let mut cause = &source;
        if kind == ErrorKind::Render {
            // MiniJinja wraps errors from includes. Report the original failure
            // and its location, while retaining the complete chain in source().
            while let Some(inner) = cause
                .source()
                .and_then(|error| error.downcast_ref::<minijinja::Error>())
            {
                cause = inner;
            }
            if cause.kind() == minijinja::ErrorKind::UndefinedError {
                kind = ErrorKind::Undefined;
            }
        }
        Self {
            kind,
            template_name: name.into(),
            line: cause.line().or_else(|| source.line()),
            detail: cause
                .detail()
                .map(str::to_owned)
                .unwrap_or_else(|| cause.kind().to_string()),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.kind == ErrorKind::InvalidName {
            write!(f, "{:?}", self.template_name)?;
        } else {
            f.write_str(&self.template_name)?;
        }
        if let Some(line) = self.line {
            write!(f, ":{line}")?;
        }
        let operation = match self.kind {
            ErrorKind::InvalidName => "invalid template name",
            ErrorKind::Syntax => "cannot compile template",
            ErrorKind::Data => "invalid template context",
            ErrorKind::Undefined | ErrorKind::Render => "cannot render template",
        };
        write!(f, ": {operation}: {}", self.detail)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}
