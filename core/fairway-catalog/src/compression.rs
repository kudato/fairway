use std::io;

pub(crate) fn compress(content: &[u8]) -> io::Result<Vec<u8>> {
    zstd::bulk::compress(content, 3)
}

pub(crate) fn decompress(compressed: &[u8]) -> io::Result<Vec<u8>> {
    zstd::stream::decode_all(compressed)
}
