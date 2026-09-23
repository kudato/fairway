use std::str::Utf8Error;

use serde::{Serialize, de::DeserializeOwned};

use crate::{Decode, Encode, Error, error::json_error};

/// One JSON value. Encoding produces compact JSON followed by a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// Returns the contained value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> AsRef<T> for Json<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned> Decode for Json<T> {
    type Error = Error;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        // Serde visitors may skip strings. Validate UTF-8 regardless of T.
        std::str::from_utf8(&bytes)
            .map_err(|error| crate::error::utf8_error("json", &bytes, error, 0))?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|error| json_error("json", error, 0))
    }
}

impl<T: Serialize> Encode for Json<T> {
    type Error = Error;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let mut bytes = serde_json::to_vec(&self.0).map_err(|source| Error::Encode {
            format: "json",
            source: Box::new(source),
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// One TOML document. Comments and source formatting are not retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toml<T>(pub T);

impl<T> Toml<T> {
    /// Returns the contained value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> AsRef<T> for Toml<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned> Decode for Toml<T> {
    type Error = Error;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let text = std::str::from_utf8(&bytes).map_err(|source| {
            let (line, column) = position(&bytes, source.valid_up_to());
            Error::Decode {
                format: "toml",
                line: Some(line),
                column: Some(column),
                source: Box::new(source),
            }
        })?;
        toml::from_str(text)
            .map(Self)
            .map_err(|source: toml::de::Error| {
                let pos = source.span().map(|span| position(&bytes, span.start));
                Error::Decode {
                    format: "toml",
                    line: pos.map(|p| p.0),
                    column: pos.map(|p| p.1),
                    source: Box::new(source),
                }
            })
    }
}

fn position(bytes: &[u8], offset: usize) -> (u64, u64) {
    let before = &bytes[..offset.min(bytes.len())];
    let line = 1 + before.iter().filter(|&&b| b == b'\n').count() as u64;
    let start = before
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let column = 1 + String::from_utf8_lossy(&before[start..]).chars().count() as u64;
    (line, column)
}

impl<T: Serialize> Encode for Toml<T> {
    type Error = Error;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        toml::to_string(&self.0)
            .map(String::into_bytes)
            .map_err(|source| Error::Encode {
                format: "toml",
                source: Box::new(source),
            })
    }
}

/// A CommonMark source document with on-demand events, without extensions.
///
/// Only the original text is retained. Encoding preserves it byte for byte;
/// structural parsing happens when [`Self::events`] is called.
#[derive(Debug, Clone)]
pub struct Markdown {
    source: String,
}

impl Markdown {
    /// Takes ownership of UTF-8 text without copying or parsing its structure.
    pub fn parse(source: String) -> Self {
        Self { source }
    }

    /// Creates a fresh iterator over the document's structural events.
    ///
    /// Events may borrow the source and are not cached. Each call starts a new
    /// parse; constructing and consuming the iterator do synchronous work on
    /// the calling thread. The parser still allocates its own working state.
    /// For large async workloads, perform the whole traversal inside a custom
    /// [`Decode`] implementation passed to [`crate::decode`].
    ///
    /// Collect explicitly when a reusable list is needed. Convert events with
    /// [`pulldown_cmark::Event::into_static`] to keep them beyond this document:
    ///
    /// ```
    /// use fairway_codec::Markdown;
    ///
    /// let events: Vec<_> = {
    ///     let document = Markdown::parse("# Heading\n".to_owned());
    ///     document.events().map(|event| event.into_static()).collect()
    /// };
    /// assert_eq!(events.len(), 3);
    /// ```
    pub fn events(&self) -> impl Iterator<Item = pulldown_cmark::Event<'_>> + '_ {
        pulldown_cmark::Parser::new(&self.source)
    }
}

impl AsRef<str> for Markdown {
    fn as_ref(&self) -> &str {
        &self.source
    }
}

impl Decode for Markdown {
    type Error = Utf8Error;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        String::decode(bytes).map(Self::parse)
    }
}

impl Encode for Markdown {
    type Error = Error;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        // Reusing the source preserves formatting and escaping without parsing
        // or reconstructing the document from structural events.
        Ok(self.source.into_bytes())
    }
}
