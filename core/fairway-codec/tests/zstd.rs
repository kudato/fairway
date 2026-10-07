//! Zstd interoperability, nested codecs, and errors from both conversion stages.

use std::{error::Error as _, io};

use fairway_codec::{Decode, Encode, Error, Json, Toml, Yaml, Zstd};
use serde::{Deserialize, Serialize};

#[test]
fn raw_buffers_round_trip_and_interoperate_with_standard_zstd_streams() {
    for contents in [Vec::new(), (0..=255).collect(), b"hello\n".repeat(4096)] {
        let packed = Zstd(contents.clone()).encode().unwrap();
        assert_eq!(
            zstd::stream::decode_all(packed.as_slice()).unwrap(),
            contents
        );
        let restored: Zstd = Zstd::decode(packed).unwrap();
        assert_eq!(restored.into_inner(), contents);

        let external = zstd::stream::encode_all(contents.as_slice(), 9).unwrap();
        let restored: Zstd = Zstd::decode(external).unwrap();
        assert_eq!(restored.into_inner(), contents);
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct Settings {
    name: String,
    retries: u32,
}

#[test]
fn text_and_structured_codecs_compose_with_compression() {
    let packed = Zstd("hello\n".to_owned()).encode().unwrap();
    assert_eq!(Zstd::<String>::decode(packed).unwrap().as_ref(), "hello\n");

    let settings = Settings {
        name: "example".to_owned(),
        retries: 3,
    };
    let packed = Zstd(Json(&settings)).encode().unwrap();
    assert_eq!(
        Zstd::<Json<Settings>>::decode(packed).unwrap().0.0,
        settings
    );
    let packed = Zstd(Toml(&settings)).encode().unwrap();
    assert_eq!(
        Zstd::<Toml<Settings>>::decode(packed).unwrap().0.0,
        settings
    );
    let packed = Zstd(Yaml(settings)).encode().unwrap();
    let Zstd(Yaml(restored)) = Zstd::<Yaml<Settings>>::decode(packed).unwrap();
    assert_eq!(restored.name, "example");
    assert_eq!(restored.retries, 3);
}

#[test]
fn concatenated_frames_are_decoded_before_the_inner_format() {
    let mut packed = zstd::stream::encode_all(b"[1,".as_slice(), 1).unwrap();
    packed.extend(zstd::stream::encode_all(b"2]".as_slice(), 9).unwrap());
    assert_eq!(Zstd::<Json<Vec<u32>>>::decode(packed).unwrap().0.0, [1, 2]);
}

#[test]
fn invalid_truncated_and_trailing_data_keep_the_decompression_error() {
    let valid = Zstd(b"hello".to_vec()).encode().unwrap();
    let mut trailing = valid.clone();
    trailing.extend_from_slice(b"invalid tail");
    let inputs = [b"not zstd".to_vec(), trailing]
        .into_iter()
        .chain((0..valid.len()).map(|end| valid[..end].to_vec()));

    for input in inputs {
        let error = Zstd::<Vec<u8>>::decode(input).unwrap_err();
        assert!(matches!(
            error,
            Error::Decode {
                format: "zstd",
                line: None,
                column: None,
                ..
            }
        ));
        assert!(error.source().unwrap().is::<io::Error>());
    }
}

#[test]
fn inner_decoding_errors_keep_their_format_position_and_source() {
    let packed = Zstd(b"[1,\nno]".to_vec()).encode().unwrap();
    let error = Zstd::<Json<Vec<u32>>>::decode(packed).unwrap_err();
    assert!(matches!(
        error,
        Error::Decode {
            format: "zstd",
            line: None,
            column: None,
            ..
        }
    ));
    let inner = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert!(matches!(
        inner,
        Error::Decode {
            format: "json",
            line: Some(2),
            column: Some(_),
            ..
        }
    ));
    assert!(inner.source().unwrap().is::<serde_json::Error>());
}

#[test]
fn inner_encoding_errors_keep_their_format_and_source() {
    let error = Zstd(Toml(42)).encode().unwrap_err();
    assert!(matches!(error, Error::Encode { format: "zstd", .. }));
    let inner = error.source().unwrap().downcast_ref::<Error>().unwrap();
    assert!(matches!(inner, Error::Encode { format: "toml", .. }));
    assert!(inner.source().unwrap().is::<toml::ser::Error>());
}

#[test]
fn custom_codecs_need_only_one_direction_and_boxable_errors() {
    struct EncodeOnly;

    impl Encode for EncodeOnly {
        type Error = anyhow::Error;

        fn encode(self) -> Result<Vec<u8>, Self::Error> {
            anyhow::bail!("custom encoding failed")
        }
    }

    #[derive(Debug)]
    struct DecodeOnly;

    impl Decode for DecodeOnly {
        type Error = io::Error;

        fn decode(_: Vec<u8>) -> Result<Self, Self::Error> {
            Err(io::Error::other("custom decoding failed"))
        }
    }

    let error = Zstd(EncodeOnly).encode().unwrap_err();
    assert!(matches!(error, Error::Encode { format: "zstd", .. }));
    assert_eq!(
        error.source().unwrap().to_string(),
        "custom encoding failed"
    );

    // Arrays implement Encode but not Decode, and their error is Infallible.
    let packed = Zstd(*b"hello").encode().unwrap();
    let error = Zstd::<DecodeOnly>::decode(packed).unwrap_err();
    assert!(matches!(error, Error::Decode { format: "zstd", .. }));
    let inner = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(inner.to_string(), "custom decoding failed");
}
