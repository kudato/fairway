//! In-memory codecs with synchronous traits and asynchronous compute-pool helpers.
//!
//! ```
//! use fairway_codec::{decode, encode, Json};
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), fairway_codec::Error> {
//! let words: Json<Vec<String>> = decode(br#"["cat", "dog"]"#.to_vec()).await?;
//! assert_eq!(encode(words).await?, b"[\"cat\",\"dog\"]\n");
//! # Ok(())
//! # }
//! ```

mod error;
mod formats;
mod jsonl;
mod stream;

pub use error::Error;
pub use formats::{Json, Markdown, Toml};
pub use jsonl::Jsonl;
pub use stream::{Decoder, Encoder, StreamDecode, StreamEncode, StreamError};

/// CommonMark event types used by [`Markdown::events`].
pub mod markdown {
    pub use pulldown_cmark::{
        Alignment, BlockQuoteKind, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType,
        MetadataBlockKind, Tag, TagEnd,
    };
}

use std::{convert::Infallible, str::Utf8Error};

/// Decodes an owned byte buffer into a value.
///
/// Implementations may reuse the buffer or build a different representation.
/// Clone the input before calling if it must also remain with the caller.
pub trait Decode: Sized {
    /// The error produced by this decoder.
    type Error;

    /// Consumes `bytes`, including on error, and returns the decoded value.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error>;
}

/// Consumes a value and returns owned bytes, reusing its storage when possible.
pub trait Encode: Sized {
    /// The error produced by this encoder.
    type Error;

    /// Consumes this value, including on error, and returns its encoded representation.
    fn encode(self) -> Result<Vec<u8>, Self::Error>;
}

/// Decodes owned input in the bounded compute pool, without blocking Tokio workers.
/// Cancellation does not interrupt a conversion that has already started.
pub async fn decode<T>(bytes: Vec<u8>) -> Result<T, T::Error>
where
    T: Decode + Send + 'static,
    T::Error: Send + 'static,
{
    fairway_compute::run(move || T::decode(bytes)).await
}

/// Encodes an owned value in the bounded compute pool and returns owned bytes.
/// Cancellation does not interrupt a conversion that has already started.
pub async fn encode<T>(value: T) -> Result<Vec<u8>, T::Error>
where
    T: Encode + Send + 'static,
    T::Error: Send + 'static,
{
    fairway_compute::run(move || value.encode()).await
}

impl Decode for String {
    type Error = Utf8Error;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        String::from_utf8(bytes).map_err(|error| error.utf8_error())
    }
}

impl Decode for Vec<u8> {
    type Error = Infallible;

    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        Ok(bytes)
    }
}

impl Encode for String {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.into_bytes())
    }
}

impl<const N: usize> Encode for [u8; N] {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.into())
    }
}

impl Encode for Vec<u8> {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        Ok(self)
    }
}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/codec.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/codec.md")]
mod guide_en {}
