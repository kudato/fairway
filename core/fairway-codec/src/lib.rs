//! Conversions between complete byte buffers and Rust values.
//!
//! A codec turns the whole contents of a file, or any other byte buffer, into
//! a value with [`Decode`] and turns a value back into bytes with [`Encode`].
//! Both traits take ownership of their input, so a codec can reuse the
//! allocation instead of copying it: decoding UTF-8 into a [`String`] and
//! encoding the string again moves the same buffer through both steps.
//!
//! Codecs work on complete buffers; there is no streaming or incremental
//! decoding. Conversions are synchronous, perform no I/O, and need no async
//! runtime, so the caller decides which thread runs them. `fairway-fs` runs
//! them on a blocking worker thread while it reads and writes files.
//!
//! # Formats
//!
//! | Type | Decoding | Encoding |
//! |------|----------|----------|
//! | [`Vec<u8>`] | returns the bytes unchanged | returns the bytes unchanged |
//! | `[u8; N]` | not supported | copies the array into a new buffer |
//! | [`String`] | validates UTF-8 without copying | returns the UTF-8 bytes without copying |
//! | [`Json<T>`] | parses one JSON value into `T` | writes compact JSON and a newline |
//! | [`Toml<T>`] | parses a TOML document into `T` | writes TOML; comments and layout are lost |
//! | [`Markdown`] | validates UTF-8 and stores the text | returns the stored text unchanged |
//!
//! `T` may be any type that implements
//! [`Deserialize`](trait@serde::Deserialize) for decoding and
//! [`Serialize`](trait@serde::Serialize) for encoding. It defaults to
//! [`json::Value`] and [`toml::Value`], which hold a document of any
//! structure. The [`json`](mod@crate::json) and [`toml`](mod@crate::toml)
//! modules re-export these value types, and the [`json!`] and [`toml!`]
//! macros build them, so a plugin does not need its own dependency on the
//! parsers.
//!
//! # Examples
//!
//! ```
//! use fairway_codec::{Decode, Encode, Json};
//!
//! let Json(mut words): Json<Vec<String>> = Json::decode(br#"["cat", "dog"]"#.to_vec())?;
//! words.push("fox".to_owned());
//! assert_eq!(Json(words).encode()?, b"[\"cat\",\"dog\",\"fox\"]\n");
//! # Ok::<(), fairway_codec::Error>(())
//! ```
//!
//! # Errors
//!
//! Built-in codecs that can fail return [`Error`]. It names the format,
//! reports the line and column of a decoding error when they are known, and
//! keeps the parser's own error as its [`source`](std::error::Error::source).
//! Codecs that cannot fail use [`Infallible`] as their error type.
//!
//! # Custom formats
//!
//! To support another format, implement [`Decode`] and [`Encode`] for your own
//! type. Such an implementation usually delegates to an existing codec, such
//! as [`String`] or [`Toml`], and converts the result; the trait documentation
//! shows examples.

mod error;
mod formats;

pub use error::Error;
pub use formats::{Json, Markdown, Toml};

/// JSON value types, re-exported from [`serde_json`].
///
/// [`Value`](crate::json::Value) represents a JSON document of any structure
/// and is the default type parameter of [`Json`]. [`json::Error`] is the
/// parser's error; a [`crate::Error`] produced by [`Json`] holds it as its
/// source.
pub mod json {
    pub use serde_json::{Error, Map, Number, Value, map, value};
}

/// TOML value types, re-exported from the `toml` crate.
///
/// [`Value`](crate::toml::Value) represents a TOML value of any structure and
/// is the default type parameter of [`Toml`].
/// [`DecodeError`](crate::toml::DecodeError) and
/// [`EncodeError`](crate::toml::EncodeError) are the parser's errors; a
/// [`crate::Error`] produced by [`Toml`] holds one of them as its source.
pub mod toml {
    pub use ::toml::{
        Value,
        de::Error as DecodeError,
        map,
        map::Map,
        ser::Error as EncodeError,
        value,
        value::{Array, Date, Datetime, DatetimeParseError, Offset, Table, Time},
    };
}

// Re-exported so that plugins can build values without depending on
// serde_json or toml themselves.
pub use ::toml::toml;
pub use serde_json::json;

/// CommonMark event types produced by [`Markdown::events`], re-exported from
/// [`pulldown_cmark`].
pub mod markdown {
    pub use pulldown_cmark::{
        Alignment, BlockQuoteKind, CodeBlockKind, CowStr, Event, HeadingLevel, InlineStr, LinkType,
        MetadataBlockKind, Tag, TagEnd,
    };
}

use std::convert::Infallible;

/// Converts a complete byte buffer into a value.
///
/// The decoder receives the whole input at once and owns it, so it may keep
/// the allocation instead of copying the bytes, as the implementation for
/// [`String`] does. A caller that still needs the bytes afterwards must clone
/// them before decoding.
///
/// # Examples
///
/// A format that delegates to the [`String`] codec:
///
/// ```
/// use fairway_codec::Decode;
///
/// // The non-empty lines of a text file.
/// struct Lines(Vec<String>);
///
/// impl Decode for Lines {
///     type Error = fairway_codec::Error;
///
///     fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
///         let text = String::decode(bytes)?;
///         let lines = text.lines().filter(|line| !line.is_empty());
///         Ok(Lines(lines.map(str::to_owned).collect()))
///     }
/// }
///
/// let Lines(lines) = Lines::decode(b"one\n\ntwo\n".to_vec())?;
/// assert_eq!(lines, ["one", "two"]);
/// # Ok::<(), fairway_codec::Error>(())
/// ```
pub trait Decode: Sized {
    /// The error returned for input that is not a valid encoding of `Self`.
    ///
    /// Decoders that accept every input use [`Infallible`].
    type Error;

    /// Decodes a value from `bytes`.
    ///
    /// The buffer is consumed even if decoding fails.
    ///
    /// # Errors
    ///
    /// Returns an error if `bytes` is not a valid encoding of `Self`.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error>;
}

/// Converts a value into a complete byte buffer.
///
/// The encoder owns the value, so it may hand over an existing allocation
/// instead of copying it, as the implementations for [`String`] and
/// [`Vec<u8>`] do. To keep a value that must be encoded, encode a clone, or
/// wrap a reference where the format allows it: `Json(&value)` serializes
/// `value` without taking it.
///
/// # Examples
///
/// A format that delegates to the [`String`] codec:
///
/// ```
/// use std::convert::Infallible;
///
/// use fairway_codec::Encode;
///
/// // Lines of a text file, each terminated by a newline.
/// struct Lines(Vec<String>);
///
/// impl Encode for Lines {
///     type Error = Infallible;
///
///     fn encode(self) -> Result<Vec<u8>, Self::Error> {
///         let mut text = self.0.join("\n");
///         text.push('\n');
///         text.encode()
///     }
/// }
///
/// let Ok(bytes) = Lines(vec!["one".to_owned(), "two".to_owned()]).encode();
/// assert_eq!(bytes, b"one\ntwo\n");
/// ```
pub trait Encode: Sized {
    /// The error returned for a value that cannot be represented in the format.
    ///
    /// Encoders that accept every value use [`Infallible`].
    type Error;

    /// Encodes this value into bytes.
    ///
    /// The value is consumed even if encoding fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the value cannot be represented in the format.
    fn encode(self) -> Result<Vec<u8>, Self::Error>;
}

impl Decode for String {
    type Error = Error;

    /// Validates UTF-8 and uses the buffer as the string's storage, without
    /// copying it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"text"` and the position of the
    /// first invalid byte if `bytes` is not valid UTF-8.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        String::from_utf8(bytes)
            .map_err(|source| error::utf8_error("text", source.as_bytes(), source.utf8_error()))
    }
}

impl Decode for Vec<u8> {
    type Error = Infallible;

    /// Returns `bytes` unchanged.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(bytes)
    }
}

impl Encode for String {
    type Error = Infallible;

    /// Returns the string's UTF-8 bytes without copying them.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.into_bytes())
    }
}

impl<const N: usize> Encode for [u8; N] {
    type Error = Infallible;

    /// Copies the array into a new buffer.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.into())
    }
}

impl Encode for Vec<u8> {
    type Error = Infallible;

    /// Returns the bytes unchanged.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self)
    }
}

// Compile the Rust examples of the plugin guides as doctests, so that the
// guides cannot drift away from the API.
#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/codec.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/codec.md")]
mod guide_en {}
