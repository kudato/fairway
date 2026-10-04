//! UTF-8 error positions shared by all text formats.

use std::{error::Error as _, str::Utf8Error};

use fairway_codec::{Decode, Error, Json, Markdown, Toml};

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
