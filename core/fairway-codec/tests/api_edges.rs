//! Additional public API contracts exercised during the Windows audit.
use fairway_codec::{
    Decode, Decoder, Encode, Encoder, Json, Jsonl, Markdown, StreamDecode, StreamEncode,
    StreamError, Toml,
};
use std::error::Error as _;

#[test]
fn wrappers_defaults_and_error_interfaces() {
    assert_eq!(Json(7).into_inner(), 7);
    assert_eq!(*Toml(9).as_ref(), 9);
    let text = String::from("кошка");
    assert_eq!(text.encode().unwrap().as_ref(), text.as_bytes());
    let bytes: &[u8] = &[0, 128, 255];
    assert_eq!(bytes.encode().unwrap().as_ref(), bytes);
    let markdown = Markdown::parse("# Title\n");
    assert!(markdown.events().next().is_some());
    assert_eq!(markdown.encode().unwrap().as_ref(), b"# Title\n");

    let mut decoder = Decoder::<String>::default();
    StreamDecode::push(&mut decoder, b"hello").unwrap();
    StreamDecode::finish(&mut decoder).unwrap();
    assert_eq!(
        StreamDecode::next(&mut decoder).unwrap().as_deref(),
        Some("hello")
    );
    let mut encoder = Encoder::<str>::default();
    let mut output = b"prefix:".to_vec();
    StreamEncode::encode(&mut encoder, "hello", &mut output).unwrap();
    StreamEncode::finish(&mut encoder, &mut output).unwrap();
    assert_eq!(output, b"prefix:hello");
    let mut lines = Jsonl::<u32>::default();
    StreamDecode::push(&mut lines, b"42\n").unwrap();
    assert_eq!(StreamDecode::next(&mut lines).unwrap(), Some(42));
    StreamDecode::finish(&mut lines).unwrap();

    let error = Json::<u32>::decode(b"bad").unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(error.source().is_some());
    let error = StreamError::Codec(error);
    assert!(error.source().is_some());
    assert!(!error.to_string().is_empty());
    for error in [
        StreamError::<std::io::Error>::Closed,
        StreamError::Failed,
        StreamError::MissingValue,
    ] {
        assert!(error.source().is_none());
        assert!(!error.to_string().is_empty());
    }
}
