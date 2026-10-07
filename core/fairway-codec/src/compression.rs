use crate::{Decode, Encode, Error};

type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A value encoded with Zstd compression.
///
/// The wrapper holds the ordinary value of type `T`. Encoding first calls
/// [`T::encode`](Encode::encode), then compresses the resulting bytes at
/// level 3. Decoding first decompresses the bytes, then calls
/// [`T::decode`](Decode::decode). The output is a standard Zstd frame, with
/// no additional header identifying the inner codec. Decoding concatenates
/// all frames before passing the complete buffer to `T`.
///
/// `T` defaults to [`Vec<u8>`], which keeps the uncompressed bytes unchanged.
/// Use [`String`] for UTF-8 text or another codec, such as [`Json`](crate::Json),
/// for structured data. Encoding requires only [`Encode`], and decoding
/// requires only [`Decode`]. The inner codec's error must convert into
/// `Box<dyn std::error::Error + Send + Sync>`; built-in codec errors,
/// [`std::io::Error`], and [`std::convert::Infallible`] satisfy this bound.
///
/// Both operations are synchronous and work on complete buffers in memory.
/// Decompression has no application-defined output size limit.
/// Errors use format `"zstd"` and keep the compression or inner codec error
/// as their [`source`](std::error::Error::source). Positions reported by an
/// inner decoder remain in that source error.
///
/// # Examples
///
/// ```
/// use fairway_codec::{Decode, Encode, Zstd};
///
/// let packed = Zstd("hello".to_owned()).encode()?;
/// let Zstd(text) = Zstd::<String>::decode(packed)?;
/// assert_eq!(text, "hello");
/// # Ok::<(), fairway_codec::Error>(())
/// ```
///
/// Compose compression with JSON while borrowing the original value:
///
/// ```
/// use fairway_codec::{Decode, Encode, Json, Zstd};
///
/// let numbers = vec![1_u32, 2, 3];
/// let packed = Zstd(Json(&numbers)).encode()?;
/// let Zstd(Json(restored)) = Zstd::<Json<Vec<u32>>>::decode(packed)?;
/// assert_eq!(restored, numbers);
/// # Ok::<(), fairway_codec::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zstd<T = Vec<u8>>(pub T);

impl<T> Zstd<T> {
    /// Consumes the wrapper and returns the value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> AsRef<T> for Zstd<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T> Decode for Zstd<T>
where
    T: Decode,
    T::Error: Into<BoxError>,
{
    type Error = Error;

    /// Decompresses the entire input and decodes its contents as `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"zstd"` if decompression or
    /// the inner decoder fails. The outer error has no line or column.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let decoded = zstd::stream::decode_all(bytes.as_slice()).map_err(decode_error)?;
        T::decode(decoded).map(Self).map_err(decode_error)
    }
}

impl<T> Encode for Zstd<T>
where
    T: Encode,
    T::Error: Into<BoxError>,
{
    type Error = Error;

    /// Encodes `T` and compresses its bytes at level 3.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Encode`] with format `"zstd"` if the inner encoder
    /// or compression fails.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let bytes = self.0.encode().map_err(encode_error)?;
        zstd::bulk::compress(&bytes, 3).map_err(encode_error)
    }
}

fn decode_error(source: impl Into<BoxError>) -> Error {
    Error::Decode {
        format: "zstd",
        line: None,
        column: None,
        source: source.into(),
    }
}

fn encode_error(source: impl Into<BoxError>) -> Error {
    Error::Encode {
        format: "zstd",
        source: source.into(),
    }
}
