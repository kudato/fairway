use std::{error::Error, fmt, marker::PhantomData};

use crate::{Decode, Encode};

/// Synchronous decoding of chunked input into a sequence of values.
///
/// Call `next` until it returns `None` after each `push` and again after `finish`.
/// Before EOF, `None` means more input is needed. After EOF it means completion.
/// An error ends decoding. Methods operate in memory and must not perform I/O.
pub trait StreamDecode {
    /// One decoded value.
    type Item;
    /// A format or decoder-state error.
    type Error;

    /// Accepts a chunk, including one split inside a value or UTF-8 character.
    fn push(&mut self, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Returns the next available value without waiting for additional input.
    fn next(&mut self) -> Result<Option<Self::Item>, Self::Error>;

    /// Marks EOF. Call `next` afterwards to drain the remaining values.
    /// Repeating a successful `finish` is allowed. Subsequent `push` is an error.
    fn finish(&mut self) -> Result<(), Self::Error>;
}

/// Synchronous encoding of values into an output buffer.
///
/// Successful calls append bytes and preserve the existing buffer prefix.
/// On error the buffer is unchanged and encoding ends. The caller owns the
/// buffer and may drain it between calls. Methods must not perform I/O.
pub trait StreamEncode {
    /// One value accepted by this format.
    type Item: ?Sized;
    /// A format or encoder-state error.
    type Error;

    /// Appends an encoded value to `output`.
    fn encode(&mut self, item: &Self::Item, output: &mut Vec<u8>) -> Result<(), Self::Error>;

    /// Appends any format trailer. Does not flush I/O, close a pipe, or save a file.
    /// Repeating a successful `finish` appends nothing. Subsequent `encode` is an error.
    fn finish(&mut self, output: &mut Vec<u8>) -> Result<(), Self::Error>;
}

/// Conversion and state errors from [`Decoder`] and [`Encoder`].
#[derive(Debug)]
pub enum StreamError<E> {
    /// The underlying [`Decode`] or [`Encode`] implementation failed.
    Codec(E),
    /// Input is finished, or the encoder has already accepted its one document.
    Closed,
    /// An earlier error ended this conversion.
    Failed,
    /// A document encoder was finished without receiving a value.
    MissingValue,
}

impl<E: fmt::Display> fmt::Display for StreamError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec(error) => error.fmt(formatter),
            Self::Closed => formatter.write_str("this stream cannot accept more input"),
            Self::Failed => formatter.write_str("an earlier error ended this stream"),
            Self::MissingValue => formatter.write_str("no document was supplied to the encoder"),
        }
    }
}

impl<E: Error + 'static> Error for StreamError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Codec(error) => Some(error),
            _ => None,
        }
    }
}

/// Adapts any [`Decode`] type to a stream containing one complete document.
///
/// Buffers all input and produces one value after EOF, then `None`. This is not
/// an incremental parser: the complete document must fit in memory. Use
/// [`crate::Jsonl`] to decode a sequence without retaining completed records.
pub struct Decoder<T> {
    bytes: Vec<u8>,
    finished: bool,
    drained: bool,
    failed: bool,
    value: PhantomData<fn() -> T>,
}

impl<T> Default for Decoder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Decoder<T> {
    /// Creates an empty document decoder.
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            finished: false,
            drained: false,
            failed: false,
            value: PhantomData,
        }
    }

    fn fail(&mut self) {
        self.failed = true;
        self.bytes = Vec::new();
    }
}

impl<T: Decode> StreamDecode for Decoder<T> {
    type Item = T;
    type Error = StreamError<T::Error>;

    fn push(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        if self.failed {
            return Err(StreamError::Failed);
        }
        if self.finished {
            self.fail();
            return Err(StreamError::Closed);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn next(&mut self) -> Result<Option<T>, Self::Error> {
        if !self.finished || self.drained || self.failed {
            return Ok(None);
        }
        self.drained = true;
        let bytes = std::mem::take(&mut self.bytes);
        match T::decode(&bytes) {
            Ok(value) => Ok(Some(value)),
            Err(error) => {
                self.failed = true;
                Err(StreamError::Codec(error))
            }
        }
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        if self.failed {
            return Err(StreamError::Failed);
        }
        self.finished = true;
        Ok(())
    }
}

enum EncodeState {
    Empty,
    Written,
    Finished,
    Failed,
}

/// Adapts any [`Encode`] type to a stream containing one complete document.
///
/// Accepts one value. A second value or finishing without a value is an error:
/// concatenating documents does not generally produce a valid document.
pub struct Encoder<T: ?Sized> {
    state: EncodeState,
    value: PhantomData<fn(&T)>,
}

impl<T: ?Sized> Default for Encoder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ?Sized> Encoder<T> {
    /// Creates an encoder expecting one document.
    pub fn new() -> Self {
        Self {
            state: EncodeState::Empty,
            value: PhantomData,
        }
    }
}

impl<T: Encode + ?Sized> StreamEncode for Encoder<T> {
    type Item = T;
    type Error = StreamError<T::Error>;

    fn encode(&mut self, item: &T, output: &mut Vec<u8>) -> Result<(), Self::Error> {
        match self.state {
            EncodeState::Empty => {}
            EncodeState::Failed => return Err(StreamError::Failed),
            _ => {
                self.state = EncodeState::Failed;
                return Err(StreamError::Closed);
            }
        }
        match item.encode() {
            Ok(bytes) => {
                output.extend_from_slice(&bytes);
                self.state = EncodeState::Written;
                Ok(())
            }
            Err(error) => {
                self.state = EncodeState::Failed;
                Err(StreamError::Codec(error))
            }
        }
    }

    fn finish(&mut self, _: &mut Vec<u8>) -> Result<(), Self::Error> {
        match self.state {
            EncodeState::Empty => {
                self.state = EncodeState::Failed;
                Err(StreamError::MissingValue)
            }
            EncodeState::Failed => Err(StreamError::Failed),
            _ => {
                self.state = EncodeState::Finished;
                Ok(())
            }
        }
    }
}
