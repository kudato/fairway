//! Zstd interoperability, nested codecs, and errors from both conversion stages.

use std::{error::Error as _, io};

use fairway_codec::{Decode, Encode, Error, Json, Toml, Zstd};

#[test]
fn raw_buffers_round_trip_and_interoperate_with_standard_zstd_streams() {
    for contents in [Vec::new(), (0..=255).collect(), b"lorem\n".repeat(4096)] {
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

    let packed = Zstd("lorem\n".to_owned()).encode().unwrap();
    assert_eq!(Zstd::<String>::decode(packed).unwrap().as_ref(), "lorem\n");
}

#[test]
fn concatenated_frames_are_decoded_before_the_inner_format() {
    let mut packed = zstd::stream::encode_all(b"[1,".as_slice(), 1).unwrap();
    packed.extend(zstd::stream::encode_all(b"2]".as_slice(), 9).unwrap());
    assert_eq!(Zstd::<Json<Vec<u32>>>::decode(packed).unwrap().0.0, [1, 2]);
}

#[test]
fn invalid_truncated_and_trailing_data_keep_the_decompression_error() {
    let valid = Zstd(b"lorem".to_vec()).encode().unwrap();
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
    // The inner codec's error stays typed, so callers can downcast it.
    let error = Zstd(Toml(1)).encode().unwrap_err();
    assert!(matches!(
        error.source().unwrap().downcast_ref::<Error>(),
        Some(Error::Encode { format: "toml", .. })
    ));

    // Arrays implement Encode but not Decode, and their error is Infallible.
    let packed = Zstd(*b"lorem").encode().unwrap();
    let error = Zstd::<DecodeOnly>::decode(packed).unwrap_err();
    assert!(matches!(error, Error::Decode { format: "zstd", .. }));
    let inner = error.source().unwrap().downcast_ref::<io::Error>().unwrap();
    assert_eq!(inner.to_string(), "custom decoding failed");
}
