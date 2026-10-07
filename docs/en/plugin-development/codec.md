# fairway-codec

The crate turns a complete byte buffer into a Rust value and back. It supports
[text](#values-and-types), [JSON](#json-and-toml), [TOML](#json-and-toml),
[YAML](#yaml), [Markdown](#markdown), and [Zstd compression](#zstd).

`Decode::decode` parses bytes into a value of the given type, and
`Encode::encode` turns a value into bytes. Both operations are synchronous and
work with the whole buffer.

Plugin dependencies:

```toml
[dependencies]
fairway-codec.workspace = true
anyhow.workspace = true
```

## Quick start

```rust
use fairway_codec::serde::Serialize;
use fairway_codec::{Encode, Error, Json};

#[derive(Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Settings {
    name: String,
    retries: u32,
}

fn settings_json() -> Result<Vec<u8>, Error> {
    let settings = Settings {
        name: "example".to_owned(),
        retries: 3,
    };

    Json(settings).encode()
}
```

Result:

```json
{"name":"example","retries":3}
```

`Settings` is a regular Rust struct. `#[derive(Serialize)]` adds serialization
for its fields, and `Json(settings).encode()` turns the struct value into
JSON bytes with a final newline.

## Values and types

The type determines the format: `Json<Vec<String>>` is a JSON array of
strings, and `Toml<Dataset>` is a TOML document with the structure of
`Dataset`.

| Type | Decoding | Encoding |
|---|---|---|
| `Vec<u8>` | the bytes as they are | the bytes as they are |
| `String` | validates UTF-8 without copying | the string's bytes without copying |
| `[u8; N]` | — | copies the array |
| `Json<T>` | one JSON document | compact JSON and a newline |
| `Toml<T>` | a TOML document | TOML without the original comments and layout |
| `Yaml<T>` | one YAML document | YAML without the original comments and layout |
| `Markdown` | validates UTF-8 and keeps the text | the source text byte for byte |
| `Zstd<T>` | decompresses bytes and decodes `T` | encodes `T` and compresses its bytes |

For `Json<T>`, `Toml<T>`, and `Yaml<T>`, `T` must implement `Deserialize` for
reading and `Serialize` for writing. Serde traits and derive macros are
available through `fairway_codec::serde`. Use
`#[serde(crate = "fairway_codec::serde")]` so generated code uses this
re-export too.

If the shape of the data is known, describe it with a type:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Toml};

#[derive(Deserialize, Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Dataset {
    name: String,
    files: Vec<String>,
}

fn main() -> Result<(), Error> {
    let input = br#"name = "events"
files = ["part-01.jsonl"]
"#
    .to_vec();

    let Toml(mut dataset) = Toml::<Dataset>::decode(input)?;
    dataset.files.push("part-02.jsonl".to_owned());

    let output = Toml(dataset).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

The TOML document is decoded into `Dataset`, then another file is added to
its `files` field. Encoding produces:

```toml
name = "events"
files = ["part-01.jsonl", "part-02.jsonl"]
```

The methods take ownership of their input and may reuse its memory instead of
copying it. If you still need the original bytes, make a copy first. To encode
a value while keeping it in the calling code, pass a reference:
`Json(&value).encode()`, `Toml(&value).encode()`, or `Yaml(&value).encode()`.

## Data without a fixed structure

If the shape of the data is determined at runtime, use generic values. Without
a type parameter, `Json` and `Yaml` use `json::Value`, and `Toml` uses
`toml::Value`.

The `json!` and `toml!` macros build such values without a direct dependency
on `serde_json` or `toml`:

```rust
use fairway_codec::{Decode, Encode, Error, Json, json};

fn main() -> Result<(), Error> {
    let input = br#"{"name":"example","ready":false}"#.to_vec();
    let Json(mut document): Json = Json::decode(input)?;
    document["ready"] = json!(true);

    let output = Json(document).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

`document` has type `json::Value`: fields are accessed by string keys.
Here, `ready` changes while `name` is preserved:

```json
{"name":"example","ready":true}
```

The generic value used by `Yaml` supports data representable in JSON. For
other YAML types, such as mappings with composite keys, specify a concrete
`T`.

## Where the data comes from

The caller prepares the bytes, obtaining them from a file, the network, or
another source. A codec performs only the in-memory conversion, without I/O.

For example, [fairway-fs](fs.md) reads and writes files through the same
traits. To work with it, the type must be `Send + 'static`, and its error must
be `Send` and convert into `Box<dyn Error + Send + Sync>`. For example,
`anyhow::Error`, `io::Error`, `codec::Error`, and `Infallible` all do.

When called directly, conversions run on the calling thread. When reading and
writing files, `fairway-fs` runs them on a separate thread for blocking
operations. If you convert large data directly in an asynchronous task, move
the work to `tokio::task::spawn_blocking` so that it does not hold up other
tasks.

## JSON and TOML

JSON is encoded compactly, followed by a newline. When decoding, only
whitespace may follow the value, and invalid UTF-8 is rejected anywhere in the
input, even in fields that the type skips.

The top level of a TOML document is always a table, so `T` is usually a struct
or a map.

Encoding TOML loses comments, blank lines, and the original layout. To keep
them, edit the document as a `String`.

## YAML

`Yaml<T>` reads and writes one YAML document. For example, this changes a name
in metadata:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Yaml};

#[derive(Deserialize, Serialize)]
#[serde(crate = "fairway_codec::serde")]
struct Metadata {
    name: String,
    description: String,
}

fn main() -> Result<(), Error> {
    let input = b"name: draft\ndescription: Check changes.\n".to_vec();
    let Yaml(mut metadata) = Yaml::<Metadata>::decode(input)?;
    metadata.name = "review".to_owned();

    let output = Yaml(metadata).encode()?;
    print!("{}", String::decode(output)?);
    Ok(())
}
```

After decoding, the Rust struct's `name` field changes while `description`
is preserved. Encoding produces:

```yaml
name: review
description: Check changes.
```

The entire input must be valid UTF-8. Multiple documents in one buffer and
duplicate mapping keys are errors.

Only `true` and `false` are inferred as booleans. Values such as `yes`, `no`,
`on`, and `off` remain strings.

Encoding preserves values, but not the source comments and layout. To preserve
the document's text, edit it as a `String`.

`Yaml` handles YAML itself. The code for a particular format extracts the
header between `---` lines from Markdown and validates skill fields.

## Markdown

`Markdown` keeps the source text of a document. `events()` parses it into a
sequence of events: start and end of a heading, paragraph, list, pieces of
text, and so on. The event types are in `codec::markdown`; they are the
[pulldown-cmark](https://docs.rs/pulldown-cmark) types.

```rust
use fairway_codec::{Markdown, markdown::{Event, HeadingLevel, Tag, TagEnd}};

fn titles(document: &Markdown) -> Vec<String> {
    let mut titles = Vec::new();
    let mut current = None;
    for event in document.events() {
        match event {
            Event::Start(Tag::Heading { level: HeadingLevel::H1, .. }) => {
                current = Some(String::new());
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(title) = &mut current {
                    title.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(title) = &mut current {
                    title.push(' ');
                }
            }
            Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
                titles.extend(current.take());
            }
            _ => {}
        }
    }
    titles
}

fn main() {
    let source = "# Intro\n\nText.\n\n## Details\n\n# Use `fairway`\n";
    let document = Markdown::parse(source.to_owned());
    let headings = titles(&document);

    assert_eq!(headings, ["Intro", "Use fairway"]);
    println!("{headings:?}");
}
```

Only first-level headings are extracted. Inline code is included in the
heading text, while the second-level `Details` heading is skipped:

```text
["Intro", "Use fairway"]
```

CommonMark is supported without extensions: tables, footnotes, strikethrough,
task lists, and front matter are not recognized.

`encode` returns the source text byte for byte, so decoding and encoding never
change a document.

Each `events()` call parses the document again, synchronously on the calling
thread. If you need the events more than once, collect them into a vector.

Events borrow text from the document where possible. To keep them longer than
the document, convert them with `Event::into_static`. Events are read-only. To
change a document, build the new text and wrap it with `Markdown::parse`.

## Zstd

`Zstd<T>` wraps an ordinary value of type `T`. Encoding calls `T::encode()`
and compresses the resulting bytes at level 3. Decoding decompresses the input
and passes the complete buffer to `T::decode()`.

Without a type parameter, `Zstd` uses `Vec<u8>` for arbitrary bytes. Use
`Zstd<String>` for text or compose it with a structured codec, such as
`Zstd<Json<T>>`:

```rust
use fairway_codec::serde::{Deserialize, Serialize};
use fairway_codec::{Decode, Encode, Error, Json, Zstd};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(crate = "fairway_codec::serde")]
struct Settings {
    name: String,
    retries: u32,
}

fn main() -> Result<(), Error> {
    let settings = Settings {
        name: "example".to_owned(),
        retries: 3,
    };

    let packed = Zstd(Json(&settings)).encode()?;
    let Zstd(Json(restored)) = Zstd::<Json<Settings>>::decode(packed)?;

    assert_eq!(restored, settings);
    println!("{restored:?}");
    Ok(())
}
```

Encoding turns `Settings` into JSON, then into compressed bytes. Decoding
reverses these conversions, producing another `Settings` value:

```text
Settings { name: "example", retries: 3 }
```

`Json(&settings)` borrows the original struct, so it remains available
after encoding.

Encoding requires `T: Encode`; decoding requires `T: Decode`. The other
direction is not required. The inner codec's error must convert into
`Box<dyn std::error::Error + Send + Sync>`, as built-in codec errors,
`io::Error`, `anyhow::Error`, and `Infallible` do.

The result is a standard Zstd frame. It has no Fairway header identifying the
inner format; the caller selects the type when decoding. Concatenated frames
are decompressed into one buffer before the inner decoder runs.

Both operations work on complete buffers in memory. Decompression has no
application-defined output size limit.

`into_inner()` returns the wrapped value; `as_ref()` borrows it. The wrapper
itself holds the ordinary value, not its compressed representation.

## Custom formats

For your own format, implement `Decode` and `Encode` for your type. The
built-in codecs are handy inside.

For example, a `Point` can be stored as two integers separated by a comma.
The decoder turns the bytes `12,34` into `Point { x: 12, y: 34 }`, and the
encoder turns the struct back into bytes:

```rust
use std::convert::Infallible;

use anyhow::Context;
use fairway_codec::{Decode, Encode};

#[derive(Debug, PartialEq)]
struct Point {
    x: i32,
    y: i32,
}

impl Decode for Point {
    type Error = anyhow::Error;

    fn decode(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let text = String::decode(bytes)?;
        let (x, y) = text.split_once(',').context("expected x,y")?;
        Ok(Self {
            x: x.parse().context("invalid x coordinate")?,
            y: y.parse().context("invalid y coordinate")?,
        })
    }
}

impl Encode for Point {
    type Error = Infallible;

    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        format!("{},{}", self.x, self.y).encode()
    }
}

fn main() -> anyhow::Result<()> {
    let point = Point::decode(b"12,34".to_vec())?;
    assert_eq!(point, Point { x: 12, y: 34 });

    let Ok(bytes) = Point { x: -5, y: 8 }.encode();
    assert_eq!(bytes, b"-5,8");
    Ok(())
}
```

`decode` and `encode` should perform only the conversion, without I/O. This
lets different consumers use the codec. In particular, `fairway-fs` calls it
while holding the file's lock in `write` and `edit`.

## Errors

Built-in codecs that can fail return `fairway_codec::Error`. The message
includes the format and, when the error's location is known, the line and
column:

```text
cannot decode json at line 3, column 2: expected value at line 3 column 2
```

For handling errors in code, use the error variants and their fields:

| Variant | What happened |
|---|---|
| `Decode { format, line, column, source }` | The data could not be decoded |
| `Encode { format, source }` | The value could not be encoded |

`format` is `"text"` (for `String`), `"json"`, `"toml"`, `"yaml"`,
`"markdown"`, or `"zstd"`. Line and column numbers start at one; the column
counts bytes. For invalid UTF-8, this is the position of the first invalid
byte.

The underlying error is available through `std::error::Error::source()`.
`Error` implements `std::error::Error + Send + Sync` and owns its diagnostic
information.

The original YAML error types are available as `codec::yaml::DecodeError` and
`codec::yaml::EncodeError`; plugins do not need a YAML parser dependency.

A `Zstd` error always has format `"zstd"`, without a line or column. Its
`source()` retains the compression or inner codec error, including any
position reported by the inner decoder.

You choose the error type for a custom codec. In the example above,
`Point::decode` uses `anyhow::Error`, and `Point::encode` uses `Infallible`.
Formats that cannot fail use `Infallible`. The result of such a conversion
can be taken without `unwrap`:

```rust
use fairway_codec::Encode;

let Ok(bytes) = "text".to_owned().encode();
assert_eq!(bytes, b"text");
```

When a file is read or written through `fairway-fs`, a decoding error becomes
the source of an `fs::Error` of kind `InvalidData`, and an encoding error the
source of one of kind `InvalidInput`. A panic in the invoked codec resumes in
the task that awaits `read`, `write`, or `edit`, and the file is left
unchanged.

No partial conversion result is returned on error.
