use std::{error, fmt};

/// A format, parser-state, or serialization error, retaining its source.
#[derive(Debug)]
pub enum Error {
    /// Input could not be decoded or the incremental parser was used after EOF.
    Decode {
        /// Format name, such as `json` or `toml`.
        format: &'static str,
        /// One-based line number, when known.
        line: Option<u64>,
        /// One-based column number, when known.
        column: Option<u64>,
        /// The underlying error.
        source: Box<dyn error::Error + Send + Sync>,
    },
    /// A value could not be encoded.
    Encode {
        /// Format name.
        format: &'static str,
        /// The underlying error.
        source: Box<dyn error::Error + Send + Sync>,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode {
                format,
                line,
                column,
                source,
            } => {
                write!(f, "cannot decode {format}")?;
                if let Some(line) = line {
                    write!(f, " at line {line}")?;
                    if let Some(column) = column {
                        write!(f, ", column {column}")?;
                    }
                }
                write!(f, ": {source}")
            }
            Self::Encode { format, source } => write!(f, "cannot encode {format}: {source}"),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        Some(match self {
            Self::Decode { source, .. } | Self::Encode { source, .. } => source.as_ref(),
        })
    }
}

pub(crate) fn utf8_error(
    format: &'static str,
    bytes: &[u8],
    source: std::str::Utf8Error,
    offset: u64,
) -> Error {
    let before = &bytes[..source.valid_up_to()];
    let line = offset + 1 + before.iter().filter(|&&b| b == b'\n').count() as u64;
    let column = 1 + before.len()
        - before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
    Error::Decode {
        format,
        line: Some(line),
        column: Some(column as u64),
        source: Box::new(source),
    }
}

pub(crate) fn json_error(format: &'static str, error: serde_json::Error, line: u64) -> Error {
    Error::Decode {
        format,
        line: (error.line() != 0).then_some(line + error.line() as u64),
        column: (error.column() != 0).then_some(error.column() as u64),
        source: Box::new(error),
    }
}
