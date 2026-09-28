//! Buffer reuse by the owned codecs and UTF-8 error positions shared by all
//! text formats.

use std::{error::Error as _, str::Utf8Error};

use fairway_codec::{Decode, Encode, Error, Json, Markdown, Toml};

fn allocation(bytes: &Vec<u8>) -> (usize, usize) {
    (bytes.as_ptr() as usize, bytes.capacity())
}

fn input() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4096);
    bytes.extend_from_slice("# Кот 🐈\r\n\r\n[link][id]\0\n\n[id]: /target\n".as_bytes());
    bytes
}

#[test]
fn whole_document_round_trip_reuses_bytes_including_spare_capacity() {
    for bytes in [Vec::new(), Vec::with_capacity(4096), input()] {
        let expected = bytes.clone();
        let original = allocation(&bytes);
        let bytes = Vec::<u8>::decode(bytes).unwrap().encode().unwrap();
        assert_eq!(allocation(&bytes), original);
        let bytes = String::decode(bytes).unwrap().encode().unwrap();
        assert_eq!(allocation(&bytes), original);
        let document = Markdown::decode(bytes).unwrap();
        assert_eq!(document.as_ref().as_bytes(), expected);
        let bytes = document.encode().unwrap();
        assert_eq!(allocation(&bytes), original);
        let document = Markdown::parse(String::from_utf8(bytes).unwrap());
        let bytes = document.encode().unwrap();
        assert_eq!(allocation(&bytes), original);
        assert_eq!(bytes, expected);
    }
}

#[test]
fn invalid_utf8_errors_keep_their_position_and_incomplete_length() {
    for (bytes, line, column) in [
        (vec![0xff], 1, 1),
        (vec![b'a', 0xff], 1, 2),
        (vec![b'a', 0xe2, 0x82], 1, 2),
        (["кот\r\nя".as_bytes(), &[0xff]].concat(), 2, 3),
        (vec![b'a', b'\n', 0xe2, 0x82], 2, 1),
    ] {
        let expected = std::str::from_utf8(&bytes).unwrap_err();
        for (format, error) in [
            ("text", String::decode(bytes.clone()).unwrap_err()),
            ("markdown", Markdown::decode(bytes.clone()).unwrap_err()),
            ("json", Json::<String>::decode(bytes.clone()).unwrap_err()),
            ("toml", Toml::<String>::decode(bytes.clone()).unwrap_err()),
        ] {
            assert_eq!(
                error.source().unwrap().downcast_ref::<Utf8Error>(),
                Some(&expected)
            );
            assert!(matches!(
                error,
                Error::Decode {
                    format: actual_format,
                    line: Some(actual_line),
                    column: Some(actual_column),
                    ..
                } if (actual_format, actual_line, actual_column) == (format, line, column)
            ));
        }
    }
}

#[test]
fn json_output_can_be_larger_than_the_input_value() {
    let text = "\0\"\\\n🐈".repeat(1024);
    let expected = text.clone();
    let bytes = Json(text).encode().unwrap();
    assert!(bytes.len() > expected.len());
    assert_eq!(Json::<String>::decode(bytes).unwrap().0, expected);
}
