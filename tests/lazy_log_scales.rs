//! Creating a histogram builds the log scale of its own width only.
//!
//! A test binary of its own: it counts every allocation the process makes, so no other test may
//! run beside it.

use std::alloc::GlobalAlloc;
use std::alloc::Layout;
use std::alloc::System;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use base2histogram::Histogram;
use base2histogram::LogScale;

/// Counts the bytes allocated through it.
struct CountingAlloc;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every method passes its arguments to `System` unchanged and returns what `System`
// returns.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller upholds the contract of `GlobalAlloc::alloc()`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller upholds the contract of `GlobalAlloc::dealloc()`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

#[test]
fn creating_a_histogram_builds_only_its_own_log_scale() {
    let before = ALLOCATED.load(Ordering::Relaxed);
    let hist = Histogram::<()>::new();
    let allocated = ALLOCATED.load(Ordering::Relaxed) - before;

    // Width 3's tables take a few kilobytes; the tables of all 16 widths take about 27 MB.
    assert!(
        allocated < 64 * 1024,
        "creating a histogram allocated {allocated} bytes"
    );
    assert_eq!(LogScale::DEFAULT_WIDTH, hist.width());
}
