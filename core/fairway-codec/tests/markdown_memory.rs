//! Allocation regression checks run in their own single-test process.
#![allow(unsafe_code)] // Instrument System only in this test binary.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use fairway_codec::{Encode, Markdown};

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

#[test]
fn markdown_retains_only_source_and_does_not_collect_during_traversal() {
    let input = "# h\n\n*x* **y** `z`\n".repeat(4_000);
    // Allow allocator/test-harness noise without accepting an event collection.
    let allowance = 2 * input.len() + 64 * 1024;
    let before_document = mark();
    let document = Markdown::parse(input.clone());
    let creation_peak = peak_since(before_document);
    assert!(creation_peak <= allowance, "creation peak: {creation_peak}");

    // Use the parser's measured working set rather than platform-specific sizes.
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
    // Dropping an unfinished iterator must also release its working state.
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
