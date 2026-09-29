# fairway-codec

Conversions between complete byte buffers and Rust values.

The crate turns bytes into Rust values and back: text, JSON, TOML, and
Markdown. It does not know where the bytes come from: other crates provide
the sources and destinations of data. For example, [fairway-fs](fs.md) reads
and writes files in the same formats. Use `fairway-codec` directly for
in-memory data, such as data received over the network, and for custom
formats.

Plugin dependency:

```toml
[dependencies]
fairway-codec.workspace = true
```

## Quick start

`Decode::decode` parses bytes into a value of the given type, and
`Encode::encode` turns a value into bytes. The type determines the format:
`Json<Vec<String>>` is a JSON array of strings.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json};

fn add_word(input: Vec<u8>) -> Result<Vec<u8>, codec::Error> {
    let Json(mut words): Json<Vec<String>> = Json::decode(input)?;
    words.push("mouse".to_owned());
    Json(words).encode()
}
```

- Both methods work with the whole buffer; there is no streaming.
- They take ownership of their input and may reuse its memory instead of
  copying it. If you still need the original data, make a copy first.
- Conversions are synchronous and run on the calling thread. When
  `fairway-fs` reads or writes a file, the conversion runs on a separate
  thread for blocking operations. If you convert large data directly in an
  asynchronous task, move the work to `tokio::task::spawn_blocking` so that it
  does not hold up other tasks.

## Built-in formats

| Type | Decoding | Encoding |
|---|---|---|
| `Vec<u8>` | the bytes as they are | the bytes as they are |
| `String` | validates UTF-8 without copying | the string's bytes without copying |
| `[u8; N]` | — | copies the array |
| `Json<T>` | one JSON document | compact JSON and a newline |
| `Toml<T>` | a TOML document | TOML without the original comments and layout |
| `Markdown` | validates UTF-8 and keeps the text | the source text byte for byte |

## JSON and TOML

`Json<T>` and `Toml<T>` wrap a value that implements `serde`. If the shape of
the data is known, describe it with a type:

```rust
use fairway_codec::{self as codec, Decode, Encode, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Dataset {
    name: String,
    files: Vec<String>,
}

fn add_part(input: Vec<u8>) -> Result<Vec<u8>, codec::Error> {
    let Toml(mut dataset): Toml<Dataset> = Toml::decode(input)?;
    dataset.files.push("part-02.jsonl".to_owned());
    Toml(dataset).encode()
}
```

Without a type parameter, the generic `json::Value` or `toml::Value` is used.
The `json!` and `toml!` macros build such values without a direct dependency
on `serde_json` or `toml`.

```rust
use fairway_codec::{self as codec, Decode, Encode, Json, json};

fn version(input: Vec<u8>) -> Result<Option<String>, codec::Error> {
    let Json(document): Json = Json::decode(input)?;
    Ok(document["version"].as_str().map(str::to_owned))
}

fn status(name: &str, ready: bool) -> Result<Vec<u8>, codec::Error> {
    Json(json!({ "name": name, "ready": ready })).encode()
}
```

- JSON is encoded compactly, followed by a newline. When decoding, only
  whitespace may follow the value, and invalid UTF-8 is rejected anywhere in
  the input, even in fields that the type skips.
- The top level of a TOML document is always a table, so `T` is usually
  a struct or a map.
- Encoding TOML loses comments, blank lines, and the original layout. To keep
  them, edit the document as a `String`.
- To encode a value without giving it up, pass a reference:
  `Json(&value).encode()`.

## Markdown

`Markdown` keeps the source text of a document. `events()` parses it into
a sequence of events: start and end of a heading, paragraph, list, pieces of
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
    let document = Markdown::parse("# Intro\n\nText.\n\n# Use `fairway`\n".to_owned());
    assert_eq!(titles(&document), ["Intro", "Use fairway"]);

    for source in ["First\nsecond\n===\n", "First  \nsecond\n===\n"] {
        let document = Markdown::parse(source.to_owned());
        assert_eq!(titles(&document), ["First second"]);
    }
}
```

- CommonMark is supported without extensions: tables, footnotes,
  strikethrough, task lists, and front matter are not recognized.
- `encode` returns the source text byte for byte, so decoding and encoding
  never change a document.
- Each `events()` call parses the document again, synchronously on the
  calling thread. If you need the events more than once, collect them into
  a vector.
- Events borrow text from the document where possible. To keep them longer
  than the document, convert them with `Event::into_static`.
- Events are read-only. To change a document, build the new text and wrap it
  with `Markdown::parse`.

## Errors

Built-in formats that can fail return `codec::Error`:

- `Decode { format, line, column, source }`: the data could not be parsed;
- `Encode { format, source }`: the value could not be encoded.

`format` is `"text"` (for `String`), `"json"`, `"toml"`, or `"markdown"`.
`line` and `column` start at 1 and are set when the position is known; the
column counts bytes. For invalid UTF-8, this is the position of the first
invalid byte. The parser's error is available in `source`.

Formats that cannot fail use the `Infallible` error type. The result of such
a conversion can be taken without `unwrap`:

```rust
use fairway_codec::Encode;

let Ok(bytes) = "text".to_owned().encode();
assert_eq!(bytes, b"text");
```

When a file is read or written through `fairway-fs`, a decoding error becomes
the source of an `fs::Error` of kind `InvalidData`, and an encoding error the
source of one of kind `InvalidInput`.

## Custom formats

To convert your own format and use it with `fairway-fs`, implement `Decode`
and `Encode` for your type. The built-in formats are handy inside.

Example: Markdown with a required TOML header between `+++` lines.

```text
+++
title = "Dataset"
+++
# Description

The first part of the dataset.
```

```rust
use anyhow::Context;
use fairway_codec::{Decode, Encode, Markdown, Toml};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Frontmatter {
    title: String,
}

struct Article {
    frontmatter: Frontmatter,
    body: Markdown,
}

impl Decode for Article {
    type Error = anyhow::Error;

    fn decode(bytes: Vec<u8>) -> anyhow::Result<Self> {
        let text = String::decode(bytes)?;
        let content = text
            .strip_prefix("+++\r\n")
            .or_else(|| text.strip_prefix("+++\n"))
            .context("the document must start with TOML frontmatter")?;
        let mut header_end = 0;
        for line in content.split_inclusive('\n') {
            if line.trim_end_matches(['\r', '\n']) == "+++" {
                let header = &content[..header_end];
                let body = &content[header_end + line.len()..];
                let Toml(frontmatter) = Toml::decode(header.as_bytes().to_vec())?;
                return Ok(Self {
                    frontmatter,
                    body: Markdown::parse(body.to_owned()),
                });
            }
            header_end += line.len();
        }
        anyhow::bail!("the closing +++ line is missing")
    }
}

impl Encode for Article {
    type Error = anyhow::Error;

    fn encode(self) -> anyhow::Result<Vec<u8>> {
        let mut bytes = b"+++\n".to_vec();
        bytes.extend(Toml(self.frontmatter).encode()?);
        bytes.extend_from_slice(b"+++\n");
        bytes.extend(self.body.encode()?);
        Ok(bytes)
    }
}

fn change_title(input: Vec<u8>, title: String) -> anyhow::Result<Vec<u8>> {
    let mut article = Article::decode(input)?;
    article.frontmatter.title = title;
    article.encode()
}

fn main() -> anyhow::Result<()> {
    for newline in ["\n", "\r\n"] {
        let body = format!("# Description{newline}{newline}Text.{newline}");
        let input = format!("+++{newline}title = \"Old\"{newline}+++{newline}{body}");
        let output = change_title(input.into_bytes(), "New".to_owned())?;
        let article = Article::decode(output)?;
        assert_eq!(article.frontmatter.title, "New");
        assert_eq!(article.body.as_ref(), body);
    }
    Ok(())
}
```

- `decode` and `encode` are synchronous. Do only the conversion in them,
  without I/O: in `write` and `edit`, `fairway-fs` calls them while it holds
  the file's lock.
- You choose the error type. The built-in formats use `codec::Error`, the
  example above uses `anyhow::Error`, and a format that cannot fail uses
  `Infallible`.
- To work with `fairway-fs`, the type must be `Send + 'static`, and its error
  must be `Send` and convert into `Box<dyn Error + Send + Sync>`. For example,
  `anyhow::Error`, `io::Error`, `codec::Error`, and `Infallible` all do.
- A panic in `decode` or `encode` called by `fairway-fs` resumes in the task
  that awaits `read`, `write`, or `edit`, and the file is left unchanged.

## API documentation

The full description of the types, traits, and errors is in the crate
documentation. To open it locally, run

```sh
cargo doc -p fairway-codec --open
```

Published versions are available on [docs.rs](https://docs.rs/fairway-codec).
