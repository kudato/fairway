//! Small public API contracts: wrapper accessors, identity codecs, and the
//! error interface.

use fairway_codec::{Decode, Encode, Json, Markdown, Toml};
use std::error::Error as _;

#[test]
fn wrappers_defaults_and_error_interfaces() {
    assert_eq!(Json(7).into_inner(), 7);
    assert_eq!(*Toml(9).as_ref(), 9);
    let text = String::from("кошка");
    assert_eq!(text.encode().unwrap(), "кошка".as_bytes());
    let bytes: &[u8] = &[0, 128, 255];
    assert_eq!(bytes.to_vec().encode().unwrap(), bytes);
    let markdown = Markdown::parse("# Title\n".to_owned());
    assert!(markdown.events().next().is_some());
    assert_eq!(markdown.encode().unwrap(), b"# Title\n");

    let error = Json::<u32>::decode(b"bad".to_vec()).unwrap_err();
    assert!(!error.to_string().is_empty());
    assert!(error.source().is_some());
}
