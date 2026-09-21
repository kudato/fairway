//! Stream contracts shared by filesystem and process adapters.

use std::{borrow::Cow, convert::Infallible};

use fairway_codec::{
    Decode, Decoder, Encode, Encoder, Error, Json, Jsonl, Markdown, StreamDecode, StreamEncode,
    StreamError, Toml,
};
use serde::{Deserialize, Serialize, ser::SerializeSeq};

fn collect<D: StreamDecode>(
    mut decoder: D,
    bytes: &[u8],
    split: usize,
) -> Result<Vec<D::Item>, D::Error> {
    let mut values = Vec::new();
    for chunk in [&bytes[..split], &bytes[split..]] {
        decoder.push(chunk)?;
        while let Some(value) = decoder.next()? {
            values.push(value);
        }
    }
    decoder.finish()?;
    decoder.finish()?;
    while let Some(value) = decoder.next()? {
        values.push(value);
    }
    assert!(decoder.next()?.is_none());
    Ok(values)
}

#[test]
fn jsonl_stream_traits_round_trip_every_boundary() {
    let values = [
        "кошка".to_owned(),
        "строка\nвнутри".to_owned(),
        "ёж".to_owned(),
    ];
    let mut encoder = Jsonl::<String>::new();
    let mut bytes = Vec::new();
    for value in &values {
        StreamEncode::encode(&mut encoder, value, &mut bytes).unwrap();
    }
    let before_finish = bytes.clone();
    StreamEncode::finish(&mut encoder, &mut bytes).unwrap();
    StreamEncode::finish(&mut encoder, &mut bytes).unwrap();
    assert_eq!(bytes, before_finish);
    assert_eq!(
        bytes.iter().filter(|&&byte| byte == b'\n').count(),
        values.len()
    );
    // Remove the final LF to exercise EOF handling as well as ordinary lines.
    for bytes in [bytes.as_slice(), &bytes[..bytes.len() - 1]] {
        for split in 0..=bytes.len() {
            assert_eq!(
                collect(Jsonl::<String>::new(), bytes, split).unwrap(),
                values
            );
        }
    }
    assert!(StreamEncode::encode(&mut encoder, &values[0], &mut bytes).is_err());
    assert_eq!(bytes, before_finish);
    assert!(StreamEncode::finish(&mut encoder, &mut bytes).is_err());
}

#[derive(Debug, PartialEq, Deserialize)]
struct ReadOnly {
    name: String,
}

#[derive(Serialize)]
struct WriteOnly<'a> {
    name: &'a str,
}

#[test]
fn reading_and_writing_have_independent_bounds_and_state() {
    let mut encoder = Jsonl::<WriteOnly<'_>>::default();
    let name = "dataset".to_owned();
    let mut bytes = Vec::new();
    encoder
        .encode(&WriteOnly { name: &name }, &mut bytes)
        .unwrap();
    StreamEncode::finish(&mut encoder, &mut bytes).unwrap();
    let values = collect(Jsonl::<ReadOnly>::default(), &bytes, 3).unwrap();
    assert_eq!(values, [ReadOnly { name }]);

    let mut both = Jsonl::<u32>::new();
    both.push(b"1\n").unwrap();
    StreamDecode::finish(&mut both).unwrap();
    let mut output = Vec::new();
    both.encode(&2, &mut output).unwrap();
    StreamEncode::finish(&mut both, &mut output).unwrap();
    assert_eq!(both.next().unwrap(), Some(1));
    assert_eq!(output, b"2\n");

    let mut empty = Jsonl::<WriteOnly<'_>>::new();
    let mut output = Vec::new();
    StreamEncode::finish(&mut empty, &mut output).unwrap();
    assert!(output.is_empty());
}

fn document_round_trip<T>(input: &[u8])
where
    T: Decode + Encode,
    <T as Decode>::Error: std::fmt::Debug,
    <T as Encode>::Error: std::fmt::Debug,
{
    for split in 0..=input.len() {
        let mut decoder = Decoder::<T>::new();
        decoder.push(&input[..split]).unwrap();
        assert!(decoder.next().unwrap().is_none());
        decoder.push(&input[split..]).unwrap();
        assert!(decoder.next().unwrap().is_none());
        decoder.finish().unwrap();
        let value = decoder.next().unwrap().expect("one document");
        assert!(decoder.next().unwrap().is_none());
        let expected = value.encode().unwrap();
        let mut encoder = Encoder::<T>::new();
        let mut output = b"prefix".to_vec();
        encoder.encode(&value, &mut output).unwrap();
        encoder.finish(&mut output).unwrap();
        encoder.finish(&mut output).unwrap();
        assert_eq!(&output[..6], b"prefix");
        assert_eq!(&output[6..], expected.as_ref());
    }
}

#[test]
fn document_adapters_cover_json_toml_markdown_text_and_bytes() {
    #[derive(Serialize, Deserialize)]
    struct Document {
        name: String,
    }
    document_round_trip::<Json<Vec<String>>>("[\"кот\",\"ёж\"]".as_bytes());
    document_round_trip::<Toml<Document>>(b"name = 'dataset'\n");
    document_round_trip::<Markdown>("# Кот\n\n[ссылка][id]\n\n[id]: /path\n".as_bytes());
    document_round_trip::<String>("кошка\r\nёж".as_bytes());
    document_round_trip::<Vec<u8>>(&[0, 255, 10, 128]);
    document_round_trip::<String>(b"");
    document_round_trip::<Markdown>(b"");
    document_round_trip::<Vec<u8>>(b"");

    let mut text = Encoder::<str>::new();
    let mut bytes = Vec::new();
    text.encode("текст", &mut bytes).unwrap();
    text.finish(&mut bytes).unwrap();
    assert_eq!(bytes, "текст".as_bytes());

    let mut raw = Encoder::<[u8]>::new();
    let mut bytes = Vec::new();
    raw.encode(&[0xff, 0], &mut bytes).unwrap();
    raw.finish(&mut bytes).unwrap();
    assert_eq!(bytes, [0xff, 0]);

    let mut array = Encoder::<[u8; 2]>::new();
    let mut bytes = Vec::new();
    array.encode(&[10, 20], &mut bytes).unwrap();
    array.finish(&mut bytes).unwrap();
    assert_eq!(bytes, [10, 20]);
}

#[test]
fn document_decoding_waits_for_eof_and_does_not_accept_truncated_values() {
    let mut json = Decoder::<Json<Vec<u32>>>::new();
    json.push(b"[1,").unwrap();
    assert!(json.next().unwrap().is_none());
    json.finish().unwrap();
    assert!(matches!(
        json.next(),
        Err(StreamError::Codec(Error::Decode { format: "json", .. }))
    ));
    assert!(json.next().unwrap().is_none());
    assert!(json.push(b"2]").is_err());
    assert!(json.finish().is_err());

    let mut empty_json = Decoder::<Json<u32>>::new();
    empty_json.finish().unwrap();
    assert!(empty_json.next().is_err());

    let mut markdown = Decoder::<Markdown>::new();
    markdown.push(&[0xc3]).unwrap();
    markdown.finish().unwrap();
    assert!(matches!(markdown.next(), Err(StreamError::Codec(_))));

    let mut closed = Decoder::<String>::new();
    closed.finish().unwrap();
    assert!(matches!(closed.push(b"late"), Err(StreamError::Closed)));
    assert!(closed.next().unwrap().is_none());
}

struct BadRecord;

impl Serialize for BadRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(2))?;
        sequence.serialize_element("already serialized")?;
        Err(serde::ser::Error::custom(
            "failure after partial serialization",
        ))
    }
}

#[test]
fn failed_encoding_preserves_output_and_ends_the_stream() {
    let mut output = b"previous bytes".to_vec();
    let mut jsonl = Jsonl::<BadRecord>::new();
    assert!(matches!(
        jsonl.encode(&BadRecord, &mut output),
        Err(Error::Encode {
            format: "jsonl",
            ..
        })
    ));
    assert_eq!(output, b"previous bytes");
    assert!(jsonl.encode(&BadRecord, &mut output).is_err());
    assert!(StreamEncode::finish(&mut jsonl, &mut output).is_err());
    assert_eq!(output, b"previous bytes");

    let mut json = Encoder::<Json<BadRecord>>::new();
    assert!(matches!(
        json.encode(&Json(BadRecord), &mut output),
        Err(StreamError::Codec(_))
    ));
    assert!(matches!(json.finish(&mut output), Err(StreamError::Failed)));
    assert_eq!(output, b"previous bytes");
}

#[test]
fn a_document_encoder_requires_exactly_one_value() {
    let mut bytes = Vec::new();
    let mut missing = Encoder::<Json<u32>>::new();
    assert!(matches!(
        missing.finish(&mut bytes),
        Err(StreamError::MissingValue)
    ));
    assert!(matches!(
        missing.encode(&Json(1), &mut bytes),
        Err(StreamError::Failed)
    ));
    assert!(bytes.is_empty());

    let mut repeated = Encoder::<Json<u32>>::new();
    repeated.encode(&Json(1), &mut bytes).unwrap();
    assert!(matches!(
        repeated.encode(&Json(2), &mut bytes),
        Err(StreamError::Closed)
    ));
    assert!(repeated.finish(&mut bytes).is_err());
    assert_eq!(bytes, b"1\n");
}

#[test]
fn custom_types_need_neither_serde_nor_send_and_can_use_custom_errors() {
    struct Local(std::rc::Rc<String>);
    impl Decode for Local {
        type Error = u8;
        fn decode(bytes: &[u8]) -> Result<Self, u8> {
            Ok(Self(std::rc::Rc::new(
                String::from_utf8(bytes.to_vec()).map_err(|_| 1_u8)?,
            )))
        }
    }
    impl Encode for Local {
        type Error = Infallible;
        fn encode(&self) -> Result<Cow<'_, [u8]>, Infallible> {
            Ok(Cow::Borrowed(self.0.as_bytes()))
        }
    }
    document_round_trip::<Local>(b"custom");
}
