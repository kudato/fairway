//! A global allocator that counts live bytes and their peak.
//!
//! Each memory test binary includes this module and installs the allocator,
//! so the counters see every allocation in that process.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

pub(crate) static LIVE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PEAK: AtomicUsize = AtomicUsize::new(0);

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
pub(crate) fn mark() -> usize {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    live
}

/// Returns how far the peak has risen above `before` since the last `mark`.
pub(crate) fn peak_since(before: usize) -> usize {
    PEAK.load(Relaxed).saturating_sub(before)
}
