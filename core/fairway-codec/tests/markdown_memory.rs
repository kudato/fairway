//! Checks that a Markdown document keeps only its source text in memory.
//!
//! The file holds a single test: the counting allocator sees every allocation
//! in the process, so concurrent tests would distort the measurements.
#![allow(unsafe_code)] // A global allocator can only be implemented with `unsafe`.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use fairway_codec::{Encode, Markdown};

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

#[test]
fn markdown_retains_only_source_and_does_not_collect_during_traversal() {
    let input = "# h\n\n*x* **y** `z`\n".repeat(4_000);
    // Room for allocator and test harness overhead, but far less than a
    // collected list of events for this input would take.
    let allowance = 2 * input.len() + 64 * 1024;
    let before_document = mark();
    let document = Markdown::parse(input.clone());
    let creation_peak = peak_since(before_document);
    assert!(creation_peak <= allowance, "creation peak: {creation_peak}");

    // Measure what the parser alone needs for this input instead of
    // hard-coding sizes that differ between platforms.
    let before_parser = mark();
    let expected = pulldown_cmark::Parser::new(&input).count();
    let parser_peak = peak_since(before_parser);
    for _ in 0..2 {
        let before_walk = mark();
        assert_eq!(document.events().count(), expected);
        let walk_peak = peak_since(before_walk);
        assert!(
            walk_peak <= parser_peak + allowance,
            "traversal peak: {walk_peak}, parser alone: {parser_peak}"
        );
        let retained = LIVE.load(Relaxed).saturating_sub(before_document);
        assert!(
            retained <= allowance,
            "retained after traversal: {retained}"
        );
    }
    // An iterator dropped before the end must release its working state too.
    {
        let mut events = document.events();
        assert!(events.next().is_some());
    }
    assert!(LIVE.load(Relaxed).saturating_sub(before_document) <= allowance);
    let bytes = document.encode().unwrap();
    assert_eq!(bytes, input.as_bytes());
    drop(bytes);
    assert!(LIVE.load(Relaxed).saturating_sub(before_document) <= 64 * 1024);
}
