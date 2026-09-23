//! Allocation regression checks run in their own single-test process.
#![allow(unsafe_code)] // Instrument System only in this test binary.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use fairway_codec::{
    self as codec, Decode, Decoder, Encode, Encoder, Jsonl, Markdown, StreamDecode, StreamEncode,
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

struct Counted;

fn allocated(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}

unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let pointer = unsafe { System.realloc(pointer, layout, size) };
        if !pointer.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            allocated(size);
        }
        pointer
    }
}

#[global_allocator]
static ALLOCATOR: Counted = Counted;

fn mark() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

fn peak_since(before: usize) -> usize {
    PEAK.load(Relaxed).saturating_sub(before)
}

// A full extra copy is 64 MiB; the allowance covers test-harness/pool bookkeeping.
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
fn owned_whole_document_and_stream_paths_do_not_allocate_another_input_buffer() {
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
    let bytes = without_input_copy("streamed document round trip", bytes, |bytes| {
        let mut decoder = Decoder::<Markdown>::new();
        decoder.push(bytes).unwrap();
        decoder.finish().unwrap();
        let document = decoder.next().unwrap().unwrap();
        let mut bytes = Vec::new();
        Encoder::<Markdown>::new()
            .encode(document, &mut bytes)
            .unwrap();
        bytes
    });
    assert_eq!(bytes.len(), SIZE);
    assert!(bytes.iter().all(|&byte| byte == b'x'));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    // Initialize the shared compute pool before measuring codec allocations.
    runtime.block_on(codec::encode(Vec::<u8>::new())).unwrap();
    let bytes = without_input_copy("async Markdown round trip", bytes, |bytes| {
        runtime.block_on(async {
            codec::encode(codec::decode::<Markdown>(bytes).await.unwrap())
                .await
                .unwrap()
        })
    });
    drop(bytes);

    let mut chunk = vec![b' '; SIZE];
    chunk[..2].copy_from_slice(b"1\n");
    let mut jsonl = without_input_copy("JSONL chunk handoff", chunk, |chunk| {
        let mut decoder = Jsonl::<u32>::new();
        decoder.push(chunk).unwrap();
        decoder
    });
    assert_eq!(jsonl.next().unwrap(), Some(1));
    drop(jsonl);

    let before = LIVE.load(Relaxed);
    let mut invalid = vec![b'x'; SIZE];
    invalid[SIZE - 1] = 0xff;
    let error = without_input_copy("invalid UTF-8", invalid, |bytes| {
        String::decode(bytes).unwrap_err()
    });
    assert_eq!(error.valid_up_to(), SIZE - 1);
    assert!(LIVE.load(Relaxed).saturating_sub(before) <= ALLOWANCE);
}
