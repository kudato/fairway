//! In-memory codecs with synchronous traits and asynchronous compute-pool helpers.
//!
//! ```
//! use fairway_codec::{decode, encode, Json};
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), fairway_codec::Error> {
//! let words: Json<Vec<String>> = decode(r#"["cat", "dog"]"#).await?;
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

use std::{borrow::Cow, convert::Infallible, str::Utf8Error};

/// Decodes a complete byte slice into a value.
pub trait Decode: Sized {
    /// The error produced by this decoder.
    type Error;

    /// Decodes `bytes` without retaining a borrow of them.
    fn decode(bytes: &[u8]) -> Result<Self, Self::Error>;
}

/// Encodes a value, borrowing existing bytes when no conversion is needed.
pub trait Encode {
    /// The error produced by this encoder.
    type Error;

    /// Returns the encoded representation of this value.
    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error>;
}

/// Decodes owned input in the bounded compute pool, without blocking Tokio workers.
/// Cancellation does not interrupt a conversion that has already started.
pub async fn decode<T>(data: impl AsRef<[u8]> + Send + 'static) -> Result<T, T::Error>
where
    T: Decode + Send + 'static,
    T::Error: Send + 'static,
{
    fairway_compute::run(move || T::decode(data.as_ref())).await
}

/// Encodes an owned value in the bounded compute pool and returns owned bytes.
/// Cancellation does not interrupt a conversion that has already started.
pub async fn encode<T>(value: T) -> Result<Vec<u8>, T::Error>
where
    T: Encode + Send + 'static,
    T::Error: Send + 'static,
{
    fairway_compute::run(move || value.encode().map(Cow::into_owned)).await
}

impl Decode for String {
    type Error = Utf8Error;

    fn decode(bytes: &[u8]) -> Result<Self, Self::Error> {
        std::str::from_utf8(bytes).map(str::to_owned)
    }
}

impl Decode for Vec<u8> {
    type Error = Infallible;

    fn decode(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(bytes.to_vec())
    }
}

impl Encode for str {
    type Error = Infallible;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        Ok(Cow::Borrowed(self.as_bytes()))
    }
}

impl Encode for String {
    type Error = Infallible;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        self.as_str().encode()
    }
}

impl Encode for [u8] {
    type Error = Infallible;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        Ok(Cow::Borrowed(self))
    }
}

impl<const N: usize> Encode for [u8; N] {
    type Error = Infallible;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        self.as_slice().encode()
    }
}

impl Encode for Vec<u8> {
    type Error = Infallible;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        self.as_slice().encode()
    }
}

impl<T: Encode + ?Sized> Encode for &T {
    type Error = T::Error;

    fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
        T::encode(self)
    }
}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/ru/plugin-development/codec.md")]
mod guide_ru {}

#[cfg(doctest)]
#[doc = include_str!("../../../docs/en/plugin-development/codec.md")]
mod guide_en {}
