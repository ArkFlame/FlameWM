//! Capacity-reuse proof for the typed `SharedBuffers` facade.
//!
//! Mirrors `text_ref_alloc.rs`: a counting global allocator gated by a
//! thread-local flag, so only allocations on this test's own thread between
//! arm and disarm are counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use flamewm_render_core::{SharedBuffers, glyphs};

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

fn note_alloc() {
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Runs `body` with counting armed on this thread and returns the allocations it made.
fn allocations_during<T>(body: impl FnOnce() -> T) -> (T, usize) {
    ALLOCS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    let result = body();
    ARMED.with(|a| a.set(false));
    (result, ALLOCS.with(|n| n.get()))
}

fn any_font() -> bool {
    glyphs::text_mask("A", 24.0, 0.0, 24.0).is_some()
}

#[test]
fn a_reclaimed_coverage_buffer_is_reused_without_reallocating() {
    if !any_font() {
        return;
    }
    let mut buffers = SharedBuffers::new();

    // A wide run grows the shared buffer and records its capacity.
    let first = buffers
        .text_mask("a wide sample label", 24.0, 0.0, 24.0)
        .expect("first mask");
    let first_capacity = first.coverage.capacity();
    assert!(
        first_capacity > 0,
        "first rasterization must retain coverage capacity"
    );
    buffers.reclaim(first);

    // Prime the "Hi" glyphs so the measured call only exercises buffer reuse.
    let warm = buffers.text_mask("Hi", 24.0, 0.0, 24.0).expect("warm mask");
    buffers.reclaim(warm);

    // A smaller run on the same facade must reuse the larger capacity and
    // allocate nothing on this thread: no fresh coverage Vec, no realloc.
    let (second, allocs) = allocations_during(|| {
        buffers
            .text_mask("Hi", 24.0, 0.0, 24.0)
            .expect("second mask")
    });
    assert_eq!(
        allocs, 0,
        "a reclaimed coverage buffer must be reused, not reallocated"
    );
    assert_eq!(
        second.coverage.capacity(),
        first_capacity,
        "the smaller run keeps the larger retained capacity"
    );
    buffers.reclaim(second);

    // The reused path emits exactly the same geometry and bytes as the plain
    // entry point, so visuals stay pixel-identical.
    let reused = buffers
        .text_mask("Hi", 24.0, 0.0, 24.0)
        .expect("reused mask");
    let plain = glyphs::text_mask("Hi", 24.0, 0.0, 24.0).expect("plain mask");
    assert_eq!(reused.x, plain.x);
    assert_eq!(reused.y, plain.y);
    assert_eq!(reused.width, plain.width);
    assert_eq!(reused.height, plain.height);
    assert_eq!(reused.coverage, plain.coverage);
    buffers.reclaim(reused);
    assert_eq!(
        buffers.coverage_capacity(),
        first_capacity,
        "reclaim must restore the retained capacity"
    );
}

#[test]
fn empty_and_zero_size_rasterizations_do_not_dirty_the_shared_buffer() {
    if !any_font() {
        return;
    }
    let mut buffers = SharedBuffers::new();
    assert!(buffers.text_mask("", 24.0, 0.0, 24.0).is_none());
    assert!(buffers.text_mask("A", 0.0, 0.0, 24.0).is_none());
    assert_eq!(buffers.coverage_capacity(), 0);
}
