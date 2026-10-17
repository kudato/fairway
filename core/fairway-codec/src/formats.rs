use std::convert::Infallible;

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Decode, Encode, Error,
    error::{json_error, position, utf8_error, yaml_error},
};

/// A JSON value of type `T`.
///
/// `T` defaults to [`json::Value`](crate::json::Value), which holds a document
/// of any structure. Decoding requires `T` to implement
/// [`Deserialize`](trait@serde::Deserialize), and encoding requires
/// [`Serialize`](trait@Serialize). References implement `Serialize` too, so
/// `Json(&value).encode()` serializes `value` without taking it.
///
/// Decoding accepts exactly one JSON value, optionally surrounded by
/// whitespace, and rejects invalid UTF-8 anywhere in the input, including in
/// fields that `T` ignores. Encoding produces compact JSON followed by a
/// newline.
///
/// # Examples
///
/// ```
/// use fairway_codec::{Decode, Encode, Json};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize, Serialize)]
/// struct Settings {
///     name: String,
///     retries: u32,
/// }
///
/// let bytes = br#"{ "name": "example", "retries": 3 }"#.to_vec();
/// let Json(mut settings) = Json::<Settings>::decode(bytes)?;
/// settings.retries += 1;
/// assert_eq!(
///     Json(&settings).encode()?,
///     b"{\"name\":\"example\",\"retries\":4}\n"
/// );
/// # Ok::<(), fairway_codec::Error>(())
/// ```
///
/// Without a type parameter, any JSON document is accepted:
///
/// ```
/// use fairway_codec::{Decode, Json};
///
/// let Json(value): Json = Json::decode(br#"{ "tags": ["a", "b"] }"#.to_vec())?;
/// assert_eq!(value["tags"][1], "b");
/// # Ok::<(), fairway_codec::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Json<T = crate::json::Value>(pub T);

impl<T> Json<T> {
    /// Consumes the wrapper and returns the value.
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

    /// Decodes exactly one JSON value into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"json"` if `bytes` is not valid
    /// UTF-8, is not valid JSON, has anything but whitespace after the value,
    /// or does not match the structure of `T`.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        // serde_json skips strings that `T` ignores without checking their
        // UTF-8, so validate the whole input first.
        std::str::from_utf8(&bytes).map_err(|error| utf8_error("json", &bytes, error))?;
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|error| json_error("json", error))
    }
}

impl<T: Serialize> Encode for Json<T> {
    type Error = Error;

    /// Serializes the value as compact JSON followed by a newline.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Encode`] with format `"json"` if the value cannot be
    /// represented in JSON, such as a map with keys that JSON cannot write as
    /// strings, or if its [`Serialize`](trait@Serialize) implementation fails.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        let mut bytes = serde_json::to_vec(&self.0).map_err(|source| Error::Encode {
            format: "json",
            source: Box::new(source),
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

/// A TOML document deserialized into `T`.
///
/// `T` defaults to [`toml::Value`], which holds a document of any structure.
/// Decoding requires `T` to implement
/// [`Deserialize`](trait@serde::Deserialize), and encoding requires
/// [`Serialize`](trait@Serialize). References implement `Serialize` too, so
/// `Toml(&value).encode()` serializes `value` without taking it.
///
/// The top level of a TOML document is always a table, so `T` is normally a
/// struct or a map. Encoding writes new text from the value: comments, blank
/// lines, and the layout of the original document are lost. To keep them,
/// edit the document as a [`String`] instead.
///
/// # Examples
///
/// ```
/// use fairway_codec::{Decode, Encode, Toml};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Deserialize, Serialize)]
/// struct Record {
///     name: String,
///     tags: Vec<String>,
/// }
///
/// let bytes = b"# Example\nname = \"alpha\"\ntags = []\n".to_vec();
/// let Toml(mut record) = Toml::<Record>::decode(bytes)?;
/// record.tags.push("text".to_owned());
/// assert_eq!(
///     Toml(&record).encode()?,
///     b"name = \"alpha\"\ntags = [\"text\"]\n"
/// );
/// # Ok::<(), fairway_codec::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toml<T = crate::toml::Value>(pub T);

impl<T> Toml<T> {
    /// Consumes the wrapper and returns the value.
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

    /// Decodes a TOML document into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"toml"` if `bytes` is not valid
    /// UTF-8, is not valid TOML, or does not match the structure of `T`.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let text =
            std::str::from_utf8(&bytes).map_err(|source| utf8_error("toml", &bytes, source))?;
        toml::from_str(text)
            .map(Self)
            .map_err(|source: toml::de::Error| {
                // The parser reports a byte range; its start is the position.
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

impl<T: Serialize> Encode for Toml<T> {
    type Error = Error;

    /// Serializes the value as a TOML document.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Encode`] with format `"toml"` if the value cannot be
    /// represented in TOML, for example when it is not a table at the top
    /// level, as with `Toml(1)`.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        toml::to_string(&self.0)
            .map(String::into_bytes)
            .map_err(|source| Error::Encode {
                format: "toml",
                source: Box::new(source),
            })
    }
}

/// A YAML document deserialized into `T`.
///
/// Decoding requires `T: DeserializeOwned`; encoding requires `T: Serialize`.
/// `T` defaults to [`json::Value`](crate::json::Value), which supports
/// JSON-compatible values. Use a concrete type for other YAML values, such as
/// mappings with composite keys or non-finite floating-point numbers.
///
/// Decoding accepts one document and rejects duplicate mapping keys. Only
/// `true` and `false` are inferred as booleans; `yes`, `no`, `on`, and `off`
/// remain strings. The entire input must be valid UTF-8.
/// Encoding writes new YAML without preserving source comments or layout.
///
/// # Examples
///
/// ```
/// use fairway_codec::{Decode, Encode, Yaml};
///
/// let Yaml(mut names): Yaml<Vec<String>> = Yaml::decode(b"- cat\n".to_vec())?;
/// names.push("dog".to_owned());
/// let bytes = Yaml(&names).encode()?;
/// assert_eq!(Yaml::<Vec<String>>::decode(bytes)?.into_inner(), names);
/// # Ok::<(), fairway_codec::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yaml<T = crate::json::Value>(pub T);

impl<T> Yaml<T> {
    /// Consumes the wrapper and returns the value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> AsRef<T> for Yaml<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

impl<T: DeserializeOwned> Decode for Yaml<T> {
    type Error = Error;

    /// Decodes one YAML document into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"yaml"` if the input is not
    /// valid UTF-8 or YAML, contains multiple documents or duplicate keys,
    /// or cannot be deserialized into `T`.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        let text =
            std::str::from_utf8(&bytes).map_err(|source| utf8_error("yaml", &bytes, source))?;
        let options = serde_saphyr::options! {
            strict_booleans: true,
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            budget: serde_saphyr::budget! { max_documents: 1 },
        };
        // The single-document entry point ignores malformed input after `...`.
        // The stream entry point validates the tail, and the document limit
        // rejects a second document before deserializing it.
        let mut documents = serde_saphyr::from_multiple_with_options(text, options.clone())
            .map_err(|source| yaml_error(text, source))?;
        let value = match documents.pop() {
            Some(value) => value,
            // The stream API skips null and empty documents. The input has
            // already been validated, so decode these into T separately.
            None => serde_saphyr::from_str_with_options(text, options)
                .map_err(|source| yaml_error(text, source))?,
        };
        Ok(Self(value))
    }
}

impl<T: Serialize> Encode for Yaml<T> {
    type Error = Error;

    /// Serializes a value as a YAML document. References are accepted too.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Encode`] with format `"yaml"` if serialization fails.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        // Expanded indentation also keeps sequence-valued mapping keys valid.
        serde_saphyr::to_string_with_options(
            &self.0,
            serde_saphyr::ser_options! { compact_list_indent: false },
        )
        .map(String::into_bytes)
        .map_err(|source| Error::Encode {
            format: "yaml",
            source: Box::new(source),
        })
    }
}

/// A CommonMark document that keeps its source text and parses it on demand.
///
/// Decoding validates UTF-8 and stores the text without parsing it.
/// [`events`](Self::events) parses the stored text each time it is called, and
/// encoding returns the text byte for byte, so decoding and encoding a
/// document never changes it. The text is also available through
/// [`AsRef<str>`].
///
/// The parser follows plain CommonMark: extensions such as tables, footnotes,
/// strikethrough, task lists, and front matter are not recognized. Events are
/// read-only; to change a document, build the new text and wrap it with
/// [`Markdown::parse`].
///
/// # Examples
///
/// ```
/// use fairway_codec::markdown::{Event, Tag, TagEnd};
/// use fairway_codec::{Decode, Encode, Markdown};
///
/// let document = Markdown::decode(b"# Lorem ipsum\n\nDolor *sit* amet.\n".to_vec())?;
///
/// let mut in_heading = false;
/// let mut headings = Vec::new();
/// for event in document.events() {
///     match event {
///         Event::Start(Tag::Heading { .. }) => in_heading = true,
///         Event::End(TagEnd::Heading(_)) => in_heading = false,
///         Event::Text(text) if in_heading => headings.push(text.into_string()),
///         _ => {}
///     }
/// }
/// assert_eq!(headings, ["Lorem ipsum"]);
///
/// let Ok(bytes) = document.encode();
/// assert_eq!(bytes, b"# Lorem ipsum\n\nDolor *sit* amet.\n");
/// # Ok::<(), fairway_codec::Error>(())
/// ```
#[derive(Debug, Clone)]
pub struct Markdown {
    source: String,
}

impl Markdown {
    /// Wraps CommonMark text without copying or parsing it.
    ///
    /// Every string is a valid CommonMark document, so this cannot fail. The
    /// text is parsed later, by [`events`](Self::events).
    pub fn parse(source: String) -> Self {
        Self { source }
    }

    /// Parses the document and returns an iterator over its CommonMark events.
    ///
    /// Every call parses the text again; nothing is cached between calls. The
    /// block structure is parsed when the iterator is created and inline
    /// content as the iterator advances, synchronously on the calling thread.
    /// Collect the events if you need to go over them more than once.
    ///
    /// Events borrow text from the document where possible. To keep them after
    /// the document is dropped, convert them with
    /// [`Event::into_static`](crate::markdown::Event::into_static).
    ///
    /// # Examples
    ///
    /// ```
    /// use fairway_codec::Markdown;
    ///
    /// let events: Vec<_> = {
    ///     let document = Markdown::parse("# Heading\n".to_owned());
    ///     document.events().map(|event| event.into_static()).collect()
    /// };
    /// // The start of the heading, its text, and its end.
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
    type Error = Error;

    /// Validates UTF-8 and stores the text without copying or parsing it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] with format `"markdown"` and the position of
    /// the first invalid byte if `bytes` is not valid UTF-8.
    fn decode(bytes: Vec<u8>) -> Result<Self, Self::Error> {
        String::from_utf8(bytes)
            .map(Self::parse)
            .map_err(|source| utf8_error("markdown", source.as_bytes(), source.utf8_error()))
    }
}

impl Encode for Markdown {
    type Error = Infallible;

    /// Returns the source text unchanged, without copying it.
    fn encode(self) -> Result<Vec<u8>, Self::Error> {
        // Rendering the events back to Markdown would normalize the markup and
        // escaping; returning the source keeps the document as it was written.
        Ok(self.source.into_bytes())
    }
}
