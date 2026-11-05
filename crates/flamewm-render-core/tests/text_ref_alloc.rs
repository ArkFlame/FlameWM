//! Allocation proof for `RuntimeDocument::set_text_ref`.
//!
//! This test binary installs a counting global allocator. Counting is gated by a
//! thread-local flag, so only allocations made on the test's own thread between the
//! arm and disarm points are recorded; other test threads are not counted.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use flamewm_render_core::{CompiledDocument, CompiledNode, NodeKind, RuntimeDocument, Style};

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
fn allocations_during(body: impl FnOnce()) -> usize {
    ALLOCS.with(|n| n.set(0));
    ARMED.with(|a| a.set(true));
    body();
    ARMED.with(|a| a.set(false));
    ALLOCS.with(|n| n.get())
}

fn node(kind: NodeKind, id: &str, text: &str, parent: Option<u32>, first_child: Option<u32>) -> CompiledNode {
    CompiledNode {
        kind,
        parent,
        first_child,
        next_sibling: None,
        id: id.to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: text.to_string().into(),
        image: None,
        style: Arc::new(Style::default()),
        hover_style: None,
        active_style: None,
        focus_style: None,
    }
}

/// root(0) -> label(1) -> text(2).
fn label_document() -> RuntimeDocument {
    RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            node(NodeKind::Element, "root", "", None, Some(1)),
            node(NodeKind::Element, "label", "", Some(0), Some(2)),
            node(NodeKind::Text, "", "steady", Some(1), None),
        ],
    })
    .expect("document validates")
}

#[test]
fn unchanged_set_text_ref_allocates_nothing() {
    let mut doc = label_document();
    doc.set_text_ref("label", "steady").expect("prime");
    let caller_text = String::from("steady");
    let revision = doc.revision();

    let allocs = allocations_during(|| {
        for _ in 0..1_000 {
            doc.set_text_ref("label", &caller_text).expect("unchanged");
        }
    });

    assert_eq!(allocs, 0, "unchanged set_text_ref must not allocate");
    assert_eq!(doc.revision(), revision, "unchanged set_text_ref must not dirty");
}

#[test]
fn changed_set_text_ref_allocates_and_control_counts_owned_path() {
    let mut doc = label_document();
    doc.set_text_ref("label", "steady").expect("prime");
    let changed = String::from("moved");
    let allocs = allocations_during(|| doc.set_text_ref("label", &changed).expect("changed"));
    assert!(allocs > 0, "a real change must store its text");

    // Control: the owned `set_text` path allocates on an unchanged call, so the counter
    // is observing real allocations and the zero above is not vacuous.
    let owned = allocations_during(|| doc.set_text("label", String::from("moved")).expect("owned"));
    assert!(owned > 0, "owned set_text with an unchanged value must allocate its argument");
}
