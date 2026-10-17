//! Checks that a Markdown document keeps only its source text in memory.
//!
//! The file holds a single test: the counting allocator sees every allocation
//! in the process, so concurrent tests would distort the measurements.
#![allow(unsafe_code)] // A global allocator can only be implemented with `unsafe`.

use std::sync::atomic::Ordering::Relaxed;

use fairway_codec::{Encode, Markdown};

#[path = "support/counting.rs"]
mod counting;

use counting::{LIVE, mark, peak_since};

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
