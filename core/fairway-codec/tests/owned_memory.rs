//! Checks that the owned codecs decode and encode a large input without
//! copying it.
//!
//! The file holds a single test: the counting allocator sees every allocation
//! in the process, so concurrent tests would distort the measurements.
#![allow(unsafe_code)] // A global allocator can only be implemented with `unsafe`.

use std::{error::Error as _, sync::atomic::Ordering::Relaxed};

use fairway_codec::{Decode, Encode, Markdown};

#[path = "support/counting.rs"]
mod counting;

use counting::{LIVE, mark, peak_since};

// A copy of the input would add 64 MiB; the allowance only covers test harness
// bookkeeping.
const SIZE: usize = 64 * 1024 * 1024;
const ALLOWANCE: usize = 64 * 1024;

fn without_input_copy<T>(label: &str, input: Vec<u8>, run: impl FnOnce(Vec<u8>) -> T) -> T {
    let before = mark();
    let result = run(input);
    let extra = peak_since(before);
    assert!(extra <= ALLOWANCE, "{label}: extra allocation peak {extra}");
    eprintln!("{label}: input={SIZE}, additional_peak={extra}");
    result
}

#[test]
fn owned_documents_do_not_allocate_another_input_buffer() {
    let bytes = vec![b'x'; SIZE];
    let bytes = without_input_copy("bytes round trip", bytes, |bytes| {
        Vec::<u8>::decode(bytes).unwrap().encode().unwrap()
    });
    let bytes = without_input_copy("text round trip", bytes, |bytes| {
        String::decode(bytes).unwrap().encode().unwrap()
    });
    let bytes = without_input_copy("Markdown round trip", bytes, |bytes| {
        Markdown::decode(bytes).unwrap().encode().unwrap()
    });
    assert_eq!(bytes.len(), SIZE);
    assert!(bytes.iter().all(|&byte| byte == b'x'));

    drop(bytes);

    let before = LIVE.load(Relaxed);
    let mut invalid = vec![b'x'; SIZE];
    invalid[SIZE - 1] = 0xff;
    let error = without_input_copy("invalid UTF-8", invalid, |bytes| {
        String::decode(bytes).unwrap_err()
    });
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<std::str::Utf8Error>()
        .unwrap();
    assert_eq!(source.valid_up_to(), SIZE - 1);
    assert!(LIVE.load(Relaxed).saturating_sub(before) <= ALLOWANCE);
}
