use std::{error, fmt};

/// An error returned by the built-in codecs.
///
/// The error names the format that failed and keeps the parser's own error as
/// its [`source`](error::Error::source), so callers can inspect or downcast
/// it. The [`Display`](fmt::Display) output has the form
/// `cannot decode {format} at line {line}, column {column}: {source}` or
/// `cannot encode {format}: {source}`; the position is omitted when it is not
/// known. Parsers often include the position in their own message as well, and
/// the TOML parser adds an excerpt of the input that spans several lines.
///
/// # Examples
///
/// ```
/// use fairway_codec::{Decode, Error, Json};
///
/// let error = Json::<Vec<u32>>::decode(b"[1,\n 2,\n x]".to_vec()).unwrap_err();
/// assert!(matches!(
///     error,
///     Error::Decode { format: "json", line: Some(3), column: Some(2), .. }
/// ));
/// assert!(error.to_string().starts_with("cannot decode json at line 3, column 2: "));
/// ```
#[derive(Debug)]
pub enum Error {
    /// The input is not a valid encoding of the requested type.
    Decode {
        /// The format that could not be decoded: `"text"`, `"json"`, `"toml"`,
        /// or `"markdown"` for the built-in codecs.
        format: &'static str,
        /// The one-based line of the error, if the decoder reports one.
        line: Option<u64>,
        /// The one-based column of the error, counted in bytes from the start
        /// of the line, if the decoder reports one.
        column: Option<u64>,
        /// The decoder's own error.
        source: Box<dyn error::Error + Send + Sync>,
    },
    /// The value cannot be represented in the format.
    Encode {
        /// The format that could not be encoded: `"json"` or `"toml"` for the
        /// built-in codecs.
        format: &'static str,
        /// The encoder's own error.
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

/// Reports invalid UTF-8 in `bytes` at the first invalid byte.
pub(crate) fn utf8_error(format: &'static str, bytes: &[u8], source: std::str::Utf8Error) -> Error {
    let (line, column) = position(bytes, source.valid_up_to());
    Error::Decode {
        format,
        line: Some(line),
        column: Some(column),
        source: Box::new(source),
    }
}

/// Converts a byte offset in `bytes` into a one-based line and column.
///
/// Only `\n` ends a line, so a `\r` before it counts as part of the line, and
/// the column counts bytes rather than characters. An offset past the end of
/// `bytes` is treated as the end.
pub(crate) fn position(bytes: &[u8], offset: usize) -> (u64, u64) {
    let before = &bytes[..offset.min(bytes.len())];
    let line = 1 + before.iter().filter(|&&b| b == b'\n').count() as u64;
    let start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let column = 1 + (before.len() - start) as u64;
    (line, column)
}

/// Converts a serde_json parsing error, keeping its position.
///
/// serde_json reports line 0 when it has no position, and column 0 when the
/// error is found before the first byte of a line, for example at the end of
/// input that ends with a newline. Both are reported as unknown rather than as
/// an invalid one-based position.
pub(crate) fn json_error(format: &'static str, error: serde_json::Error) -> Error {
    Error::Decode {
        format,
        line: (error.line() != 0).then_some(error.line() as u64),
        column: (error.column() != 0).then_some(error.column() as u64),
        source: Box::new(error),
    }
}
