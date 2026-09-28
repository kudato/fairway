//! Checks that the owned codecs decode and encode a large input without
//! copying it.
//!
//! The file holds a single test: the counting allocator sees every allocation
//! in the process, so concurrent tests would distort the measurements.
#![allow(unsafe_code)] // A global allocator can only be implemented with `unsafe`.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    error::Error as _,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use fairway_codec::{Decode, Encode, Markdown};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// Counts live bytes and their peak on top of the system allocator.
struct Counted;

/// Records an allocation of `size` bytes and raises the peak if needed.
fn allocated(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    PEAK.fetch_max(live, Relaxed);
}

// SAFETY: every method forwards its arguments unchanged to `System`, so the
// allocator keeps the `GlobalAlloc` contract. The counters are atomics and
// never allocate.
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds the contract of `alloc` for `layout`.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds the contract of `alloc_zeroed` for `layout`.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` came from `System` through this allocator with
        // `layout`, as the caller guarantees.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: `pointer` came from `System` through this allocator with
        // `layout`, and the caller upholds the contract of `realloc` for `size`.
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

/// Resets the peak to the current live size and returns that size.
fn mark() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

/// Returns how far the peak has risen above `before` since the last `mark`.
fn peak_since(before: usize) -> usize {
    PEAK.load(Relaxed).saturating_sub(before)
}

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
