use std::{fmt, marker::PhantomData};

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Encode, Error, Json, StreamDecode, StreamEncode, error::json_error, stream::append_owned,
};

#[derive(Debug)]
struct ClosedInput;

impl fmt::Display for ClosedInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSONL input is already closed")
    }
}

impl std::error::Error for ClosedInput {}

#[derive(Debug)]
struct ClosedOutput;

impl fmt::Display for ClosedOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JSONL output is already closed or failed")
    }
}

impl std::error::Error for ClosedOutput {}

/// A line-oriented JSON codec with independent decoding and encoding state.
pub struct Jsonl<T> {
    bytes: Vec<u8>,
    start: usize,
    scanned: usize,
    line: u64,
    finished: bool,
    failed: bool,
    output_finished: bool,
    output_failed: bool,
    value: PhantomData<fn() -> T>,
}

impl<T> Default for Jsonl<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Jsonl<T> {
    /// Creates a codec with no input or output records.
    pub fn new() -> Self {
        Self {
            bytes: Vec::new(),
            start: 0,
            scanned: 0,
            line: 0,
            finished: false,
            failed: false,
            output_finished: false,
            output_failed: false,
            value: PhantomData,
        }
    }
}

impl<T: DeserializeOwned> Jsonl<T> {
    /// Takes ownership of bytes, which may end inside a line or UTF-8 character.
    pub fn push(&mut self, chunk: Vec<u8>) -> Result<(), Error> {
        if self.finished || self.failed {
            self.fail();
            return Err(Error::Decode {
                format: "jsonl",
                line: None,
                column: None,
                source: Box::new(ClosedInput),
            });
        }
        if self.start != 0 {
            self.bytes.drain(..self.start);
            self.scanned -= self.start;
            self.start = 0;
        }
        append_owned(&mut self.bytes, chunk);
        Ok(())
    }

    /// Returns one complete record, or `None` until more input is available.
    /// After [`Self::finish`], `None` means the sequence is complete.
    #[allow(clippy::should_implement_trait)] // None before EOF means "need more bytes".
    pub fn next(&mut self) -> Result<Option<T>, Error> {
        if self.failed {
            return Ok(None);
        }
        let newline = self.bytes[self.scanned..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|n| n + self.scanned);
        let (end, next) = match newline {
            Some(end) => (end, end + 1),
            None if self.finished && self.start < self.bytes.len() => {
                (self.bytes.len(), self.bytes.len())
            }
            None => {
                self.scanned = self.bytes.len();
                return Ok(None);
            }
        };
        let line = &self.bytes[self.start..end];
        // JSON itself accepts CR as trailing whitespace, including CRLF input.
        let result = std::str::from_utf8(line)
            .map_err(|error| crate::error::utf8_error("jsonl", line, error, self.line))
            .and_then(|_| {
                serde_json::from_slice(line).map_err(|error| json_error("jsonl", error, self.line))
            });
        self.line += 1;
        self.start = next;
        self.scanned = next;
        match result {
            Ok(value) => {
                if self.start == self.bytes.len() {
                    self.bytes.clear();
                    self.start = 0;
                    self.scanned = 0;
                }
                Ok(Some(value))
            }
            Err(error) => {
                self.fail();
                Err(error)
            }
        }
    }

    /// Marks EOF, allowing a final record without a newline. Idempotent.
    pub fn finish(&mut self) {
        self.finished = true;
    }

    fn fail(&mut self) {
        self.failed = true;
        self.bytes = Vec::new();
        self.start = 0;
        self.scanned = 0;
    }
}

impl<T: DeserializeOwned> StreamDecode for Jsonl<T> {
    type Item = T;
    type Error = Error;

    fn push(&mut self, bytes: Vec<u8>) -> Result<(), Error> {
        Self::push(self, bytes)
    }

    fn next(&mut self) -> Result<Option<T>, Error> {
        Self::next(self)
    }

    fn finish(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Decode {
                format: "jsonl",
                line: None,
                column: None,
                source: Box::new(ClosedInput),
            });
        }
        Self::finish(self);
        Ok(())
    }
}

impl<T: Serialize> StreamEncode for Jsonl<T> {
    type Item = T;
    type Error = Error;

    fn encode(&mut self, item: T, output: &mut Vec<u8>) -> Result<(), Error> {
        if self.output_finished || self.output_failed {
            self.output_failed = true;
            return Err(Error::Encode {
                format: "jsonl",
                source: Box::new(ClosedOutput),
            });
        }
        // Encode separately: a serializer can fail after emitting a prefix.
        // Reuse Json's representation, including its single trailing newline.
        match Json(item).encode() {
            Ok(bytes) => {
                append_owned(output, bytes);
                Ok(())
            }
            Err(Error::Encode { source, .. }) => {
                self.output_failed = true;
                Err(Error::Encode {
                    format: "jsonl",
                    source,
                })
            }
            Err(error) => unreachable!("Json::encode returned a decoding error: {error}"),
        }
    }

    fn finish(&mut self, _: &mut Vec<u8>) -> Result<(), Error> {
        if self.output_failed {
            return Err(Error::Encode {
                format: "jsonl",
                source: Box::new(ClosedOutput),
            });
        }
        self.output_finished = true;
        Ok(())
    }
}
