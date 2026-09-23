//! Owned inputs keep their allocation across whole-document and stream adapters.

use fairway_codec::{
    self as codec, Decode, Decoder, Encode, Encoder, Json, Jsonl, Markdown, StreamDecode,
    StreamEncode,
};

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

#[tokio::test]
async fn async_round_trip_reuses_bytes_through_the_compute_pool() {
    let bytes = input();
    let expected = bytes.clone();
    let original = allocation(&bytes);
    let bytes = codec::encode(codec::decode::<Vec<u8>>(bytes).await.unwrap())
        .await
        .unwrap();
    assert_eq!(allocation(&bytes), original);
    let bytes = codec::encode(codec::decode::<String>(bytes).await.unwrap())
        .await
        .unwrap();
    assert_eq!(allocation(&bytes), original);
    let bytes = codec::encode(codec::decode::<Markdown>(bytes).await.unwrap())
        .await
        .unwrap();
    assert_eq!(allocation(&bytes), original);
    assert_eq!(bytes, expected);
}

#[test]
fn stream_adapters_keep_the_first_chunk_and_handoff_the_output() {
    // The first chunk ends inside a UTF-8 character. Reserved capacity is enough
    // to append the rest, so the allocation can survive the whole pipeline.
    let mut first = Vec::with_capacity(4096);
    first.extend_from_slice(&[b'#', b' ', 0xd0]);
    let original = allocation(&first);
    let mut decoder = Decoder::<Markdown>::new();
    decoder.push(first).unwrap();
    assert!(decoder.next().unwrap().is_none());
    decoder.push(Vec::with_capacity(8192)).unwrap();
    decoder.push(vec![0x9a, b'\n']).unwrap();
    decoder.finish().unwrap();
    let document = decoder.next().unwrap().unwrap();
    assert_eq!(document.as_ref(), "# К\n");
    let mut output = Vec::with_capacity(8192);
    let mut encoder = Encoder::<Markdown>::new();
    encoder.encode(document, &mut output).unwrap();
    encoder.finish(&mut output).unwrap();
    assert_eq!(allocation(&output), original);
    assert_eq!(output, "# К\n".as_bytes());
}

#[test]
fn stream_assembly_can_grow_and_preserves_existing_output() {
    let first = b"small".to_vec();
    let tail = " and much more text".repeat(1024).into_bytes();
    let mut expected = first.clone();
    expected.extend_from_slice(&tail);
    let mut decoder = Decoder::<Vec<u8>>::new();
    decoder.push(first).unwrap();
    decoder.push(tail).unwrap();
    decoder.finish().unwrap();
    let bytes = decoder.next().unwrap().unwrap();
    assert_eq!(bytes, expected);
    let mut output = b"prefix:".to_vec();
    Encoder::<Vec<u8>>::new()
        .encode(bytes, &mut output)
        .unwrap();
    assert_eq!(&output[..7], b"prefix:");
    assert_eq!(&output[7..], expected);
}

#[test]
fn invalid_utf8_errors_keep_their_position_and_incomplete_length() {
    for bytes in [vec![b'a', 0xff], vec![b'a', 0xe2, 0x82]] {
        let expected = std::str::from_utf8(&bytes).unwrap_err();
        assert_eq!(String::decode(bytes.clone()).unwrap_err(), expected);
        assert_eq!(Markdown::decode(bytes).unwrap_err(), expected);
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

#[test]
fn jsonl_handles_repeated_adoption_compaction_and_empty_chunks() {
    let input = "1\r\n22\n333\n4444\n55555".as_bytes();
    for width in 1..=input.len() {
        let mut decoder = Jsonl::<u32>::new();
        let mut output = Vec::new();
        for chunk in input.chunks(width) {
            decoder.push(chunk.to_vec()).unwrap();
            while let Some(value) = decoder.next().unwrap() {
                output.push(value);
            }
            decoder.push(Vec::new()).unwrap();
        }
        decoder.finish();
        while let Some(value) = decoder.next().unwrap() {
            output.push(value);
        }
        assert_eq!(output, [1, 22, 333, 4444, 55555], "chunk width {width}");
    }
}
