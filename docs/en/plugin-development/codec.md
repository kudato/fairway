# Codec

`fairway-codec` decodes text and bytes in memory and encodes values back into bytes.

## Quick start

`decode(data).await` decodes data into the requested type, and `encode(value).await`
encodes a value into a `Vec<u8>`. The value's type selects the format:
for example, `Json<Vec<String>>` represents a JSON array of strings.

Both functions perform the conversion in Fairway's compute pool. A conversion that
passes bytes as is (`IS_NOOP`), such as decoding into `Vec<u8>`, runs in place.
Decoding takes ownership of a `Vec<u8>`; encoding consumes the value and returns
an owned buffer. This contract is the same for data from files, processes,
network connections, and memory. If the input must remain with the caller,
clone it explicitly before conversion.

```rust
use fairway_codec::{self as codec, Json};

async fn encode_words() -> Result<Vec<u8>, codec::Error> {
    let words: Json<Vec<String>> = codec::decode(r#"["кошка", "собака"]"#.as_bytes().to_vec()).await?;
    codec::encode(words).await
}
```

## Synchronous conversion

The `Decode` and `Encode` trait methods run on the calling thread.
They suit short operations and calls to one codec from inside another.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json};

fn encode_three_numbers() -> Result<Vec<u8>, codec::Error> {
    let numbers: Json<Vec<u64>> = Json::decode(b"[10, 20, 30]".to_vec())?;
    numbers.encode()
}
```

Use `codec::decode` or `codec::encode` to run lengthy conversions in the pool.
A series of short synchronous calls also occupies the thread until the series completes.

## Migrating to owned inputs

`Decode::decode` and `codec::decode` now take `Vec<u8>`. `Encode::encode` takes
`self` and returns `Vec<u8>` instead of `Cow<[u8]>`; `.into_owned()` is no longer
needed. Custom codecs must update these signatures. There are no parallel
`decode_owned` or `encode_owned` methods.

Streaming uses the same rule: `push` consumes each `Vec<u8>`, and
`StreamEncode::encode` consumes each item. `Markdown::parse` consumes a `String`.
To keep a value, clone it at the call site. To inspect a Markdown source without
consuming the document, use `document.as_ref()`.

## Whole-document formats

### JSON and TOML

`Json<T>` and `Toml<T>` contain a decoded value of type `T`.
Use `into_inner` to extract and modify it. Wrap the modified value in `Json`
or `Toml` to encode it again.

For example, a dataset description in TOML:

```toml
name = "documents"
files = ["part-01.jsonl"]
```

```rust
use fairway_codec::{self as codec, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Dataset {
    name: String,
    files: Vec<String>,
}

async fn add_part(input: String) -> Result<Vec<u8>, codec::Error> {
    let dataset: Toml<Dataset> = codec::decode(input.into_bytes()).await?;
    let mut dataset = dataset.into_inner();
    dataset.files.push("part-02.jsonl".into());
    codec::encode(Toml(dataset)).await
}
```

### Markdown

`Markdown` retains the document's original text. Its `events` method creates an
iterator over text and the start and end of headings, paragraphs, lists, and
other constructs. No complete event list is created or cached.
`encode` consumes the document and returns its original byte buffer unchanged, without parsing it.

```rust
use fairway_codec::{self as codec, Markdown, markdown::Event};

async fn inspect_markdown(input: String) -> anyhow::Result<Vec<u8>> {
    let document: Markdown = codec::decode(input.into_bytes()).await?;
    for event in document.events() {
        if let Event::Text(text) = event {
            println!("{text}");
        }
    }
    Ok(codec::encode(document).await?)
}
```

Encoding preserves indentation, markers, escaping, and line endings byte for
byte. Each call to `events()` starts a new parse. The parser allocates its own
working state: an iterator does not imply constant memory usage.

Constructing and consuming the iterator are synchronous operations on the
calling thread. `codec::decode::<Markdown>().await` only retains the text after
validating UTF-8; subsequent traversal does not automatically run in the compute
pool. For large documents, perform the entire analysis inside a custom type's
`Decode` implementation and call it through `codec::decode`, as described in
[Custom types](#custom-types).

Collect events explicitly when needed:

```rust
use fairway_codec::Markdown;

let document = Markdown::parse("# Heading\n".to_owned());
let events: Vec<_> = document.events().collect();
assert_eq!(events.len(), 3);

// These events can outlive the document.
let owned: Vec<_> = document.events().map(|event| event.into_static()).collect();
drop(events);
drop(document);
assert_eq!(owned.len(), 3);
```

This changes the API: `events()` previously returned an `&[Event<'static>]` slice.
It now returns an iterator yielding `Event<'_>` values that may borrow the
document's text. Use `.count()` instead of `.len()` to count events, or collect
a `Vec` explicitly for indexing or reusing already parsed events. Calling
`.iter()` before traversal is no longer needed. Collecting the complete list
again requires memory for every event.

## Streaming

### JSONL

`Jsonl<T>` decodes and encodes a sequence of JSON values, one per line.
Its methods are synchronous. To process a batch of records in the pool,
call them inside a custom `Decode` or `Encode` implementation.

#### Decoding JSONL

`push` takes ownership of a `Vec<u8>` chunk. After each chunk, call `next` until it returns `None`
to retrieve all available records.
A chunk can end inside a line or multibyte UTF-8 character.

Once all bytes have been passed, call `finish` and read through `next` until `None` again.
This also decodes a final line without a trailing `\n`.

```rust
use fairway_codec::{self as codec, Jsonl};

fn main() -> Result<(), codec::Error> {
    let input = "\"кошка\"\n\"собака\"";
    let mut words = Jsonl::<String>::new();

    for chunk in input.as_bytes().chunks(5) {
        words.push(chunk.to_vec())?;
        while let Some(word) = words.next()? {
            println!("{word}");
        }
    }

    words.finish();
    while let Some(word) = words.next()? {
        println!("{word}");
    }
    Ok(())
}
```

#### Encoding as JSONL

`StreamEncode::encode` appends a record's JSON representation and a trailing `\n`
to the output buffer, consuming the record. After passing the bytes to a consumer,
the output buffer can be cleared for the next record. Its allocation may be replaced
when empty.

```rust
use fairway_codec::{self as codec, Jsonl, StreamEncode};

fn encode_words(words: Vec<String>) -> Result<Vec<u8>, codec::Error> {
    let mut encoder = Jsonl::<String>::new();
    let mut bytes = Vec::new();
    for word in words {
        encoder.encode(word, &mut bytes)?;
    }
    StreamEncode::finish(&mut encoder, &mut bytes)?;
    Ok(bytes)
}
```

`StreamEncode::finish` completes encoding. Sending the remaining bytes,
closing stdin, or saving a file are separate operations.

### A document from chunks

`Decoder<T>` and `Encoder<T>` connect `Decode` and `Encode` types to the streaming API:

- `Decoder<T>` accepts chunks of bytes and returns one document after `finish`.
- `Encoder<T>` accepts one document and appends its representation to the output buffer.

They support `Json<T>`, `Toml<T>`, `Markdown`, text, bytes, and custom types.
The document must fit in memory. Use `Jsonl<T>` for a sequence of records.

```rust
use anyhow::Context;
use fairway_codec::{Decoder, Encoder, Markdown, StreamDecode, StreamEncode};

fn markdown_from_chunks(chunks: Vec<Vec<u8>>) -> anyhow::Result<Vec<u8>> {
    let mut decoder = Decoder::<Markdown>::new();
    for chunk in chunks {
        decoder.push(chunk)?;
    }
    decoder.finish()?;
    let document = decoder.next()?.context("The decoder returned no document")?;

    let mut encoder = Encoder::<Markdown>::new();
    let mut bytes = Vec::new();
    encoder.encode(document, &mut bytes)?;
    encoder.finish(&mut bytes)?;
    Ok(bytes)
}
```

## Custom types

`Decode` defines how bytes are decoded into a custom type, and `Encode` defines the reverse conversion.
The traits are implemented independently. Nested formats are called through synchronous methods:
the entire conversion runs in one task when called through `codec::decode` or `codec::encode`.

A custom type can add required TOML frontmatter to Markdown,
with a `title` field between `+++` lines:

```text
+++
title = "Dataset"
+++
# Description

The first part of the dataset.
```

```rust
use anyhow::Context;
use fairway_codec::{self as codec, Decode, Encode, Markdown, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Frontmatter {
    title: String,
}

struct Article {
    frontmatter: Frontmatter,
    body: Markdown,
}

impl codec::Decode for Article {
    type Error = anyhow::Error;

    fn decode(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let text = String::decode(bytes)?.replace("\r\n", "\n");
        let content = text
            .strip_prefix("+++\n")
            .context("The document must start with TOML frontmatter")?;
        let (header, body) = content
            .split_once("\n+++\n")
            .or_else(|| content.strip_suffix("\n+++").map(|header| (header, "")))
            .context("Missing closing +++ line")?;
        let frontmatter: Toml<Frontmatter> = Toml::decode(header.as_bytes().to_vec())?;

        Ok(Self {
            frontmatter: frontmatter.into_inner(),
            body: Markdown::parse(body.to_owned()),
        })
    }
}

impl codec::Encode for Article {
    type Error = anyhow::Error;

    fn encode(self) -> anyhow::Result<Vec<u8>> {
        let header = Toml(self.frontmatter).encode()?;
        let mut bytes = b"+++\n".to_vec();
        bytes.extend_from_slice(&header);
        if !bytes.ends_with(b"\n") {
            bytes.push(b'\n');
        }
        bytes.extend_from_slice(b"+++\n");
        bytes.extend_from_slice(&self.body.encode()?);
        Ok(bytes)
    }
}

async fn change_title(input: String, title: String) -> anyhow::Result<Vec<u8>> {
    let mut article: Article = codec::decode(input.into_bytes()).await?;
    article.frontmatter.title = title;
    codec::encode(article).await
}
```

A plugin implements the traits for its own type. Implementing `codec::Decode`
or `codec::Encode` directly for `codec::Markdown` is prohibited
by [Rust's orphan rules](https://doc.rust-lang.org/reference/items/implementations.html#orphan-rules).

## API

### Conversion

- `decode::<T>(data).await -> Result<T, T::Error>` calls `T::decode` in the pool
  (in place with `IS_NOOP = true`).
  Requires `T: Decode + Send + 'static` and `T::Error: Send + 'static`.
  The `data` argument is an owned `Vec<u8>`.
- `encode(value).await -> Result<Vec<u8>, T::Error>` calls `T::encode` in the pool
  (in place with `IS_NOOP = true`) and returns an owned buffer.
  Requires `T: Encode + Send + 'static` and `T::Error: Send + 'static`.

The functions consume their inputs, including on error. Pass a `Vec<u8>` for
decoding, or transfer a `String` with `.into_bytes()` without copying. A slice
requires an explicit `.to_vec()` at the call site. For encoding, pass a value
such as `Json(value)`. References to local values do not satisfy `'static`.

### Synchronous traits

`Decode: Sized` defines decoding:

- `type Error` is the decoding error type.
- `const IS_NOOP: bool` states that decoding passes bytes as is, without parsing
  or validation. Defaults to `false`; among built-in types, only `Vec<u8>` sets `true`.
- `decode(bytes: Vec<u8>) -> Result<Self, Self::Error>` decodes bytes into a value.

`Encode: Sized` defines encoding:

- `type Error` is the encoding error type.
- `const IS_NOOP: bool` states that encoding returns the value's bytes as is.
  Defaults to `false`; `Vec<u8>`, `[u8; N]`, `String`, and `Markdown` set `true`.
- `encode(self) -> Result<Vec<u8>, Self::Error>` returns the byte representation.

Both traits consume their inputs and return owned results. Implementations may
reuse the input storage, grow it, or allocate a different representation; the
result is not limited to the input size. `String`, `Vec<u8>`, and `Markdown`
reuse their byte allocation. JSON and TOML build a decoded value or serialized
representation and may allocate additional memory.

Both traits are synchronous and require neither `Send` nor `'static`.
Each implementation defines its own error type. A wrapper such as
`Json(&value)` can explicitly borrow a serializable model for synchronous
encoding; consuming the wrapper does not clone that model.

### Formats

The complete document being decoded and the result of one `Encode` call must fit in memory.

#### Text and bytes

- `String` implements `Decode`: it validates UTF-8 and takes over the input allocation.
- `Vec<u8>` implements `Decode`: it returns the input buffer unchanged.
- `String` and `Vec<u8>` implement `Encode` by transferring their byte allocation.
- `[u8; N]` implements `Encode` by moving its bytes into a `Vec<u8>`.
- Slices and references have no blanket `Encode` implementation. Use `.to_owned()`,
  `.to_vec()`, or `.clone()` explicitly if the original must remain available.

#### Json and Toml

- `Json<T>` contains one JSON value. Only whitespace may follow it in the input.
  Encoding produces compact JSON with a trailing `\n`.
- `Toml<T>` contains a UTF-8 document parsed according to the [TOML specification](https://toml.io/en/).
  Encoding does not retain comments or original formatting.

Construct values with `Json(value)` and `Toml(value)`. Both types provide these methods:

- `into_inner() -> T` extracts the contents.
- `as_ref() -> &T` returns a reference through `AsRef<T>`.

The trait implementations are independent:

- `Decode` requires `T: serde::de::DeserializeOwned`.
- `Encode` requires `T: serde::Serialize`.

Synchronous encoding supports references, such as `Json(&value)`.
The `into_inner` and `as_ref` methods are also synchronous.

#### Markdown

`Markdown` implements `Decode` and `Encode` for [CommonMark](https://spec.commonmark.org/)
without extensions. Its synchronous methods are:

- `parse(text: String) -> Markdown` takes ownership of UTF-8 text without copying or parsing it.
- `as_ref() -> &str` borrows the retained source through `AsRef<str>`.
- `events() -> impl Iterator<Item = markdown::Event<'_>> + '_` starts a new
  synchronous traversal without caching events.

`codec::markdown` reexports `Event`, `Tag`, `TagEnd`, and related types from
[pulldown-cmark](https://docs.rs/pulldown-cmark/latest/pulldown_cmark/).

### Streaming conversions

`StreamDecode` and `StreamEncode` operate synchronously on data in memory.
They can be implemented independently for a custom format or protocol.
Neither trait requires `Send` or `'static`. An asynchronous adapter handles I/O
and submits lengthy conversions to the compute pool.

#### StreamDecode

- `type Item` is the type of one decoded record.
- `type Error` is the decoding error type.
- `push(&mut self, bytes: Vec<u8>) -> Result<(), Self::Error>` accepts a chunk of bytes.
  A chunk boundary can fall inside a value or UTF-8 character.
- `next(&mut self) -> Result<Option<Self::Item>, Self::Error>` returns an available value.
  Before EOF, `None` means more input is needed. After EOF, it means completion.
- `finish(&mut self) -> Result<(), Self::Error>` marks EOF.
  After successful completion, another call is allowed, but `push` is an error.

After each `push` and after `finish`, call `next` until it returns `None`.
Any error ends decoding.

#### StreamEncode

- `type Item` is the type of a value to encode.
- `type Error` is the encoding error type.
- `encode(&mut self, item: Self::Item, output: &mut Vec<u8>) -> Result<(), Self::Error>`
  appends the next value's bytes to the buffer.
- `finish(&mut self, output: &mut Vec<u8>) -> Result<(), Self::Error>`
  appends a format trailer if required.

Buffer and completion rules:

- The caller owns the buffer and can clear it after passing the bytes to a consumer.
- Successful calls append bytes, preserving the buffer's existing contents.
  An empty buffer may adopt the encoded value's allocation.
- The input value is consumed even on error. An error leaves the output buffer
  unchanged and prevents further encoding.
- Repeating a successful `finish` appends nothing. Calling `encode` after `finish` is an error.
- Finishing an encoder, sending its bytes, and closing an I/O stream are separate operations.

#### JSONL

`Jsonl<T>` implements the streaming traits with `Item = T` and `Error = codec::Error`:

- `StreamDecode` when `T: serde::de::DeserializeOwned`.
- `StreamEncode` when `T: serde::Serialize`.

Decoding and encoding bounds and states are independent.
Neither bound is required by the `Jsonl::<T>::new()` constructor or `Default`.

Inherent methods of `Jsonl<T>`:

- `new() -> Jsonl<T>` creates a codec with no input or output records.
- `push(&mut self, chunk: Vec<u8>) -> Result<(), Error>` adds a chunk of bytes.
- `next(&mut self) -> Result<Option<T>, Error>` decodes one available line.
- `finish(&mut self)` marks EOF. Repeated calls are allowed.

The `push`, `next`, and `finish` methods require `T: serde::de::DeserializeOwned`.

Decoding follows the `StreamDecode` contract:

- Input is UTF-8 without a BOM, with one JSON value per line.
- `\n`, `\r\n`, and a final line without a newline are accepted.
- An empty line is an error. Empty input contains zero records.
- Incomplete lines, including incomplete UTF-8 characters, are retained between `push` calls.
- When no unread input remains, the next nonempty chunk is adopted without copying.
  Otherwise unread bytes are compacted and the new chunk is appended; this can
  copy bytes or grow the buffer.
- Completed records are not accumulated. Unread bytes and one decoded record must fit in memory.
- After an error, `next` returns `Ok(None)`.
- Calling `push` after an error or `finish` returns `Error::Decode` with format `"jsonl"`.

Encoding follows the `StreamEncode` contract:

- Each record is encoded as `Json(value)` with a trailing `\n`.
- `StreamEncode::finish` appends no bytes.
- Zero records produce empty output.
- Encoding errors use format `"jsonl"`.

Input and output have separate completion calls:

- `Jsonl::finish(&mut codec)` ends input and returns `()`.
- `StreamDecode::finish(&mut codec)` ends input and returns `Result<(), Error>`.
- `StreamEncode::finish(&mut codec, &mut output)` ends output and returns `Result<(), Error>`.

#### Decoder

`Decoder<T>` implements `StreamDecode` when `T: Decode`.
`Item = T`, and `Error = StreamError<T::Error>`.

- `Decoder::<T>::new()` creates a single-document decoder. It also implements `Default`.

The decoder adopts its first nonempty chunk, appends later chunks, and transfers
the accumulated buffer to `T::decode` after EOF. Appending may grow the buffer
and move bytes; ownership does not make split records free to assemble:

- Before `finish`, `next` returns `None`.
- After `finish`, the first `next` returns the document or a conversion error.
  Subsequent calls return `Ok(None)`.
- Empty input is validated according to the chosen `Decode` implementation.
- After an error, `next` returns `Ok(None)`.

#### Encoder

`Encoder<T>` implements `StreamEncode` when `T: Encode`.
`Item = T`, and `Error = StreamError<T::Error>`.

- `Encoder::<T>::new()` creates a single-document encoder. It also implements `Default`.

The encoder reuses the existing `Encode` implementation and requires exactly one value:

- A second `encode` call is an error.
- Calling `finish` without a value is an error. Pass an empty value to encode empty text or bytes.

### Execution

The call determines where the conversion runs, regardless of input size:

- `codec::decode` and `codec::encode` submit conversions to the shared [fairway-compute](compute.md) pool.
  Conversions with `IS_NOOP = true` run in place.
- `Decode`, `Encode`, `StreamDecode`, and `StreamEncode` methods run on the current thread
  and must not perform I/O.

The pool is shared by the process. Its thread count is `std::thread::available_parallelism()`,
falling back to one thread if it cannot be determined. Configuring Tokio threads
does not change this limit. Waiting for capacity is asynchronous.

Conversions submitted to the pool occupy neither Tokio worker threads nor its blocking I/O pool.
This follows [Tokio's recommendations](https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code).

Waiting for capacity can be cancelled. A conversion already submitted to the pool
runs even if it has not started yet, retaining its capacity until completion.
If the wait was cancelled, its result is discarded.

### Errors

The error type depends on the conversion:

- Decoding `String` and `Markdown`: `std::str::Utf8Error` for invalid UTF-8.
- Decoding `Vec<u8>` and encoding text or bytes: `std::convert::Infallible`.
- JSON, TOML, JSONL, and Markdown encoding: `codec::Error`.
- `Decoder<T>` and `Encoder<T>`: `StreamError<E>`, where `E` is the chosen conversion's error.

#### Error

`codec::Error` implements `std::error::Error`, `Send`, and `Sync`. Its variants are:

- `Decode { format, line, column, source }` indicates a format, parser-state, or type conversion error.
- `Encode { format, source }` indicates a failure to encode a value as bytes.

Error fields:

- `format: &'static str` identifies the format: `"json"`, `"jsonl"`, `"toml"`, or `"markdown"`.
- `line: Option<u64>` and `column: Option<u64>` give the position, if known.
  Numbering starts at `1`. JSONL line numbers refer to the complete input.
- `source: Box<dyn std::error::Error + Send + Sync>` contains the underlying error.

#### StreamError

- `Codec(E)` is the original `Decode` or `Encode` error.
- `Closed` means input has ended or the encoder has already accepted its document.
- `Failed` means an earlier error ended the conversion.
- `MissingValue` means `Encoder::finish` was called without a document.

`StreamError<E>` implements `std::error::Error` when `E: std::error::Error + 'static`.
For `Codec(E)`, `source()` returns the underlying error.
