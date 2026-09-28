//! Behavior of the built-in codecs: validation, error details, round trips,
//! and Markdown traversal.

use std::{convert::Infallible, error::Error as _};

use fairway_codec::{Decode, Encode, Error, Json, Markdown, Toml};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct Dataset {
    name: String,
    files: Vec<String>,
}

#[test]
fn json_and_toml_round_trip_owned_and_borrowed_values() {
    let value = Dataset {
        name: "документы".into(),
        files: vec!["a.jsonl".into()],
    };
    let bytes = Json(&value).encode().unwrap();
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    assert_eq!(Json::<Dataset>::decode(bytes).unwrap().as_ref(), &value);
    let bytes = Toml(&value).encode().unwrap();
    assert_eq!(Toml::<Dataset>::decode(bytes).unwrap().into_inner(), value);
    assert_eq!(Json(vec![1, 2]).encode().unwrap(), b"[1,2]\n");
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
    let error = Toml::<Dataset>::decode(b"name = 'x'\nfiles = [".to_vec()).unwrap_err();
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
    let error = Toml::<Dataset>::decode(vec![0xff]).unwrap_err();
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

fn render(text: &str) -> String {
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(text));
    html
}

#[test]
fn markdown_preserves_commonmark_structure() {
    for input in [
        "# Heading\n\nText with **bold**, *emphasis*, and `code`.\n",
        "> Quote\n>\n> - one\n> - two\n\n---\n",
        "3. First\n4. Second\n\n    Nested paragraph\n",
        "[link](https://example.org \"title\") and ![alt](image.png)\n",
        "[reference][id]\n\n[id]: https://example.org \"title\"\n",
        "```rust\nlet x = `a`;\n```\n",
        "<div>raw HTML</div>\n\nline  \nbreak\n",
        "\\*literal\\* \\[text\\] &amp; &#35; heading\n",
        "- [ ] no task-list extension\n\n~~no strikethrough~~\n",
        "```\ninside ``` a fence\n```\n",
        "# Кот 🐈\r\n\r\n[ссылка][id]\r\n\r\n[id]: /путь 'заголовок'\r\n",
        "",
    ] {
        let markdown = Markdown::decode(input.as_bytes().to_vec()).unwrap();
        let mut html = String::new();
        pulldown_cmark::html::push_html(&mut html, markdown.events());
        assert_eq!(render(input), html, "input: {input:?}\noutput: {html:?}");
        assert_eq!(markdown.encode().unwrap(), input.as_bytes());
    }
    assert!(Markdown::decode(vec![255]).is_err());
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
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, document.events());
    assert_eq!(html, render(source));
    assert_eq!(first.next(), Some(Event::Text("Heading".into())));
    drop(first);

    let clone = document.clone();
    drop(document);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, clone.events());
    assert_eq!(html, render(source));
    assert_eq!(clone.encode().unwrap(), source.as_bytes());
}

#[test]
fn markdown_events_can_be_collected_and_owned_explicitly() {
    let source = "Escaped \\*text\\* &amp; **bold** [link][id]\n\n[id]: /target 'title'\n";
    let document = Markdown::parse(source.to_owned());
    let borrowed: Vec<_> = document.events().collect();
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, borrowed.iter().cloned());
    assert_eq!(html, render(source));

    let owned: Vec<_> = document.events().map(|event| event.into_static()).collect();
    assert_eq!(borrowed, owned);
    drop(borrowed);
    drop(document);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, owned.into_iter());
    assert_eq!(html, render(source));
}

#[test]
fn errors_are_send_and_sync() {
    fn assert_traits<T: Send + Sync + std::error::Error>() {}
    assert_traits::<Error>();
}

#[test]
fn markdown_encoding_does_not_reinterpret_text_as_structure() {
    for text in [
        "4\\. This is text, not a list\n",
        "Literal &amp;copy; and &#10;&#10; newlines in a paragraph\n",
        "Heading *across\nmultiple lines*\n======\n",
        "Text\n    ---\n",
        "[unknown][reference]\n\n[reference]: /target 'title'\n",
        "# Кот\r\n\r\n\tКод\r\n\r\n*курсив* и _курсив_  \r\n",
        "Text without a final newline\0with a NUL",
    ] {
        let document = Markdown::parse(text.to_owned());
        assert_eq!(document.as_ref(), text);
        // Events may normalize the text they carry, but traversal must leave
        // the stored source untouched.
        for event in document.events() {
            std::hint::black_box(event);
        }
        assert_eq!(document.encode().unwrap(), text.as_bytes());
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
    let mut value: Json = Json::decode(br#"{"name":"fairway"}"#.to_vec()).unwrap();
    value.0["items"] = json!([1, 2]);
    let value: Json = Json::decode(value.encode().unwrap()).unwrap();
    assert_eq!(value.0["items"][1], 2);
    let mut config: Toml = Toml::decode(b"name = 'fairway'".to_vec()).unwrap();
    config.0["name"] = toml::Value::String("updated".into());
    let config: Toml = Toml::decode(config.encode().unwrap()).unwrap();
    assert_eq!(config.0["name"].as_str(), Some("updated"));
    let table = toml! { count = 3 };
    assert_eq!(table["count"].as_integer(), Some(3));
}
