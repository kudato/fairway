//! Public codec contracts, including chunk boundaries and error state transitions.

use std::{borrow::Cow, error::Error as _};

use fairway_codec::{Decode, Encode, Error, Json, Jsonl, Markdown, Toml};
use serde::{Deserialize, Serialize};

#[test]
fn raw_data_is_borrowed_and_custom_errors_need_no_io_conversion() {
    assert_eq!(String::decode("кот".as_bytes()).unwrap(), "кот");
    assert!(String::decode(&[0xff]).is_err());
    assert_eq!(Vec::<u8>::decode(&[0, 255]).unwrap(), [0, 255]);
    assert!(matches!("hello".encode().unwrap(), Cow::Borrowed(b"hello")));
    assert!(matches!([0_u8, 255].encode().unwrap(), Cow::Borrowed(_)));
    let data = vec![1_u8, 2];
    assert_eq!(Encode::encode(&&data).unwrap().as_ptr(), data.as_ptr());

    struct Custom;
    impl Decode for Custom {
        type Error = u8;
        fn decode(_: &[u8]) -> Result<Self, Self::Error> {
            Err(7)
        }
    }
    impl Encode for Custom {
        type Error = u8;
        fn encode(&self) -> Result<Cow<'_, [u8]>, Self::Error> {
            Err(8)
        }
    }
    assert!(matches!(Custom::decode(b""), Err(7)));
    assert_eq!(Custom.encode(), Err(8));
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
    let bytes = Json(&value).encode().unwrap().into_owned();
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    assert_eq!(Json::<Dataset>::decode(&bytes).unwrap().as_ref(), &value);
    let bytes = Toml(&value).encode().unwrap().into_owned();
    assert_eq!(Toml::<Dataset>::decode(&bytes).unwrap().into_inner(), value);
    assert_eq!(Json(vec![1, 2]).encode().unwrap().as_ref(), b"[1,2]\n");
}

#[test]
fn format_errors_retain_format_position_and_source() {
    let error = Json::<Vec<u8>>::decode(b"[1,\nno]").unwrap_err();
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
    assert!(Json::<u32>::decode(b"1 2").is_err());
    assert_eq!(Json::<u32>::decode(b"1 \r\n\t").unwrap().0, 1);
    let error = Toml::<Dataset>::decode(b"name = 'x'\nfiles = [").unwrap_err();
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
    let error = Toml::<Dataset>::decode(&[0xff]).unwrap_err();
    assert!(error.source().unwrap().is::<std::str::Utf8Error>());
}

#[test]
fn jsonl_accepts_every_split_including_inside_utf8_and_crlf() {
    let bytes = "\"кошка\"\r\n\"собака\"\n\"ёж\"".as_bytes();
    for split in 0..=bytes.len() {
        let mut parser = Jsonl::<String>::new();
        let mut result = Vec::new();
        for chunk in [&bytes[..split], &bytes[split..]] {
            parser.push(chunk).unwrap();
            while let Some(value) = parser.next().unwrap() {
                result.push(value);
            }
        }
        parser.finish();
        parser.finish();
        while let Some(value) = parser.next().unwrap() {
            result.push(value);
        }
        assert_eq!(result, ["кошка", "собака", "ёж"], "split {split}");
        assert!(parser.next().unwrap().is_none());
    }
}

#[test]
fn jsonl_errors_are_terminal_and_lines_are_global() {
    let mut parser = Jsonl::<u32>::new();
    parser.push(b"1\n").unwrap();
    assert_eq!(parser.next().unwrap(), Some(1));
    parser.push(b"2\ninvalid\n3\n").unwrap();
    assert_eq!(parser.next().unwrap(), Some(2));
    let error = parser.next().unwrap_err();
    assert!(matches!(
        error,
        Error::Decode {
            format: "jsonl",
            line: Some(3),
            column: Some(1),
            ..
        }
    ));
    assert!(parser.next().unwrap().is_none());
    assert!(parser.push(b"4\n").is_err());
    parser.finish();
    assert!(parser.next().unwrap().is_none());
}

#[test]
fn jsonl_empty_lines_bom_extra_values_and_invalid_utf8_are_errors() {
    for invalid in [
        b"\n".as_slice(),
        b"\r\n",
        b" \n",
        b"1 2\n",
        b"\xef\xbb\xbf1\n",
        b"\"\xff\"\n",
    ] {
        let mut parser = Jsonl::<serde_json::Value>::new();
        parser.push(invalid).unwrap();
        assert!(parser.next().is_err(), "{invalid:?}");
    }
    let mut empty = Jsonl::<u32>::new();
    empty.finish();
    assert!(empty.next().unwrap().is_none());
    assert!(empty.push(b"").is_err());
}

#[test]
fn jsonl_needs_eof_for_an_unterminated_value() {
    let mut parser = Jsonl::<u32>::new();
    parser.push(b"12").unwrap();
    assert!(parser.next().unwrap().is_none());
    parser.push(b"3").unwrap();
    assert!(parser.next().unwrap().is_none());
    parser.finish();
    assert_eq!(parser.next().unwrap(), Some(123));
    assert!(parser.next().unwrap().is_none());
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
    ] {
        let markdown = Markdown::decode(input.as_bytes()).unwrap();
        let output = markdown.encode().unwrap();
        let output = std::str::from_utf8(&output).unwrap();
        assert_eq!(
            render(input),
            render(output),
            "input: {input:?}\noutput: {output:?}"
        );
    }
    assert!(Markdown::decode(&[255]).is_err());
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
    ] {
        let document = Markdown::parse(text);
        assert_eq!(document.encode().unwrap().as_ref(), text.as_bytes());
    }
}

#[test]
fn ignored_json_fields_still_require_valid_utf8() {
    #[derive(Deserialize)]
    struct Empty {}
    assert!(Json::<Empty>::decode(b"{\"ignored\":\"\xff\"}").is_err());
    let mut parser = Jsonl::<Empty>::new();
    parser.push(b"{\"ignored\":\"\xff\"}\n").unwrap();
    assert!(parser.next().is_err());
}
