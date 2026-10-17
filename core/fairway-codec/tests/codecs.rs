//! Behavior of the built-in codecs: validation, error details, round trips,
//! and Markdown traversal.

use std::{convert::Infallible, error::Error as _};

use fairway_codec::{Decode, Encode, Error, Json, Markdown, Toml};
use serde::Deserialize;

#[test]
fn raw_data_is_owned_and_custom_errors_need_no_io_conversion() {
    assert_eq!(String::decode("кот".as_bytes().to_vec()).unwrap(), "кот");
    assert!(String::decode(vec![0xff]).is_err());
    let bytes: Result<_, Infallible> = Vec::<u8>::decode(vec![0, 255]);
    assert_eq!(bytes.unwrap(), [0, 255]);
    let bytes: Result<_, Infallible> = vec![0_u8, 255].encode();
    assert_eq!(bytes.unwrap(), [0, 255]);
    let text: Result<_, Infallible> = "hello".to_owned().encode();
    assert_eq!(text.unwrap(), b"hello");
    let array: Result<_, Infallible> = [0_u8, 255].encode();
    assert_eq!(array.unwrap(), [0, 255]);
    let markdown: Result<_, Infallible> = Markdown::parse("# hello".to_owned()).encode();
    assert_eq!(markdown.unwrap(), b"# hello");

    struct Custom;
    impl Decode for Custom {
        type Error = u8;
        fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
            Err(7)
        }
    }
    impl Encode for Custom {
        type Error = &'static str;
        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            Err("custom encoding failed")
        }
    }
    assert!(matches!(Custom::decode(b"".to_vec()), Err(7)));
    assert_eq!(Custom.encode(), Err("custom encoding failed"));
}

#[test]
fn format_errors_retain_format_position_and_source() {
    let error = Json::<Vec<u8>>::decode(b"[1,\nno]".to_vec()).unwrap_err();
    assert!(error.source().unwrap().is::<serde_json::Error>());
    assert!(matches!(
        error,
        Error::Decode {
            format: "json",
            line: Some(2),
            column: Some(_),
            ..
        }
    ));
    assert!(Json::<u32>::decode(b"1 2".to_vec()).is_err());
    assert_eq!(Json::<u32>::decode(b"1 \r\n\t".to_vec()).unwrap().0, 1);
    let error = Toml::<toml::Table>::decode(b"name = 'x'\nfiles = [".to_vec()).unwrap_err();
    assert!(matches!(
        error,
        Error::Decode {
            format: "toml",
            line: Some(2),
            ..
        }
    ));
    let error = Toml(1).encode().unwrap_err();
    assert!(matches!(error, Error::Encode { format: "toml", .. }));
    assert!(error.source().unwrap().is::<toml::ser::Error>());
    let error = Toml::<toml::Table>::decode(vec![0xff]).unwrap_err();
    assert!(error.source().unwrap().is::<std::str::Utf8Error>());
}

#[test]
fn parser_error_columns_count_bytes_after_non_ascii_text() {
    let json = Json::<Vec<String>>::decode("\r\n[\"я\", ?]".as_bytes().to_vec()).unwrap_err();
    let toml = Toml::<toml::Table>::decode("\r\n'я' = ?".as_bytes().to_vec()).unwrap_err();
    assert!(json.source().unwrap().is::<serde_json::Error>());
    assert!(toml.source().unwrap().is::<toml::de::Error>());
    for (format, error) in [("json", json), ("toml", toml)] {
        assert!(matches!(
            error,
            Error::Decode {
                format: actual,
                line: Some(2),
                column: Some(8),
                ..
            } if actual == format
        ));
    }
}

#[test]
fn markdown_traversals_are_independent_and_can_stop_early() {
    use fairway_codec::markdown::{Event, HeadingLevel, Tag};

    // The reference definition follows the link, so every traversal has to
    // resolve it from the whole document.
    let source = "# Heading\n\n[link][id] and *emphasis*\n\n[id]: /target\n";
    let document = Markdown::parse(source.to_owned());
    let mut first = document.events();
    assert!(matches!(
        first.next(),
        Some(Event::Start(Tag::Heading {
            level: HeadingLevel::H1,
            ..
        }))
    ));
    let expected = "<h1>Heading</h1>\n<p><a href=\"/target\">link</a> and <em>emphasis</em></p>\n";
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, document.events());
    assert_eq!(html, expected);
    assert_eq!(first.next(), Some(Event::Text("Heading".into())));
    drop(first);

    let clone = document.clone();
    drop(document);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, clone.events());
    assert_eq!(html, expected);
    assert_eq!(clone.encode().unwrap(), source.as_bytes());

    // Traversal may normalize the text it yields, but never the stored source.
    for text in [
        "4\\. This is text, not a list\n",
        "# Кот\r\n\r\n\tКод\r\n\r\n*курсив* и _курсив_  \r\n",
    ] {
        let document = Markdown::parse(text.to_owned());
        document.events().for_each(|event| {
            std::hint::black_box(event);
        });
        assert_eq!(document.as_ref(), text);
        assert_eq!(document.encode().unwrap(), text.as_bytes());
    }
}

#[test]
fn markdown_follows_plain_commonmark_without_extensions() {
    use fairway_codec::markdown::{Event, Tag};

    let source = "---\ntitle: front matter\n---\n\n~~struck~~ and a footnote[^1]\n\n\
                  - [x] task\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n[^1]: The note.\n";
    let document = Markdown::parse(source.to_owned());
    let events: Vec<_> = document.events().collect();
    for event in &events {
        assert!(
            !matches!(
                event,
                Event::Start(
                    Tag::Strikethrough
                        | Tag::Table(_)
                        | Tag::FootnoteDefinition(_)
                        | Tag::MetadataBlock(_)
                ) | Event::TaskListMarker(_)
                    | Event::FootnoteReference(_)
            ),
            "{event:?} in {events:#?}"
        );
    }
    // The extension syntax stays in the text of ordinary paragraphs.
    let text: String = events
        .iter()
        .filter_map(|event| match event {
            Event::Text(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect();
    for literal in ["~~struck~~", "[x] task", "| a | b |", "[^1]: The note."] {
        assert!(text.contains(literal), "{literal:?} in {text:?}");
    }
}

#[test]
fn ignored_json_fields_still_require_valid_utf8() {
    #[derive(Deserialize)]
    struct Empty {}
    assert!(Json::<Empty>::decode(b"{\"ignored\":\"\xff\"}".to_vec()).is_err());
}

#[test]
fn default_formats_and_construction_use_only_codec_api() {
    use fairway_codec::{json, toml};
    let mut value: Json = Json::decode(br#"{"name":"example"}"#.to_vec()).unwrap();
    value.0["items"] = json!([1, 2]);
    let value: Json = Json::decode(value.encode().unwrap()).unwrap();
    assert_eq!(value.as_ref()["items"][1], 2);
    assert_eq!(Json(vec![1, 2]).encode().unwrap(), b"[1,2]\n");
    let mut config: Toml = Toml::decode(b"name = 'example'".to_vec()).unwrap();
    config.0["name"] = toml::Value::String("updated".into());
    let config: Toml = Toml::decode(config.encode().unwrap()).unwrap();
    assert_eq!(config.into_inner()["name"].as_str(), Some("updated"));
    let table = toml! { count = 3 };
    assert_eq!(table["count"].as_integer(), Some(3));
}
