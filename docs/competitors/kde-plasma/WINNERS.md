# KDE Plasma winners for FlameWM

Snapshot: 2026-11-08. Ranked. Each entry lists files, a spec, an acceptance test, PSS cost estimate, and dependency group.
Excluded (in flight): dock, weather tint, icon-theme and taskbar alignment, accent token, notification history bounds, DND settings toggle, RenderBackend enum.
Rejected and the reasons are in `LEARNINGS.md`. Only three candidates pass the bar; a fourth is conditional.

PSS estimates are reasoning estimates, not measurements. No FlameWM or KDE build was run in this pass, so every PSS cell is `unmeasured` until a runtime check.

## Rank 1: Bounded clipboard history

Why: Klipper's history (default 20, cap 2048) is the most-used KDE clipboard feature. FlameWM keeps one text item only (`crates/flamewm-clipboard/src/keeper.rs:21`). Zero idle cost when history is empty.

Files (owner: `flamewm-clipboard`):
- new `crates/flamewm-clipboard/src/history.rs`
- edit `crates/flamewm-clipboard/src/keeper.rs` (hook push on owner change; reuse `PASSWORD_HINT` check at `:24`)
- edit `crates/flamewm-clipboard/src/lib.rs` (`mod history;` and export)

Spec:

```rust
// crates/flamewm-clipboard/src/history.rs
use std::collections::VecDeque;

pub const MAX_ITEMS: usize = 20;          // Klipper default
pub const MAX_BYTES: usize = 256 * 1024;  // total cap, FlameWM choice

pub struct ClipHistory {
    items: VecDeque<String>, // newest at front
    bytes: usize,
}

impl ClipHistory {
    pub const fn new() -> Self { Self { items: VecDeque::new(), bytes: 0 } }
    /// Returns false when the text is empty, too large for MAX_BYTES, or a duplicate of the newest item.
    pub fn push(&mut self, text: String) -> bool { /* dedupe newest; evict back until items<=MAX_ITEMS && bytes<=MAX_BYTES */ }
    pub fn iter(&self) -> impl Iterator<Item = &str> { self.items.iter().map(String::as_str) }
    pub fn clear(&mut self) { self.items.clear(); self.bytes = 0; }
}
```

Rules:
- Skip owners that advertise `x-kde-passwordManagerHint` (same check as `keeper.rs:24`).
- Items larger than `MAX_BYTES` are not stored. Return `false`.
- An explicit clear (existing `SetSelectionOwner` with no owner) calls `clear()`.
- No thread, no timer, no disk write. Memory is only the strings held.

Acceptance (unit tests in `history.rs`, run by the lead):
- `push` evicts oldest when `MAX_ITEMS` is exceeded.
- `push` evicts until `bytes <= MAX_BYTES`.
- Duplicate of the newest item is not added.
- Item over `MAX_BYTES` is rejected.
- `clear` empties the history.
- Command to run: `cargo test -p flamewm-clipboard history` (not run here).

PSS estimate: about 0 when empty; up to 256 KiB heap when full (`unmeasured`).

Dependency group: serialize with other edits to `flamewm-clipboard`. UI (popup) is a separate follow-up and is not part of this winner.

## Rank 2: Sticky keys (latch modifiers)

Why: KWin ships Sticky Keys (`kwin/src/plugins/stickykeys/`). FlameWM has no sticky or slow keys (0 `.rs` hits). Accessibility gap with zero idle cost.

Files (owner: `flamewm-platform`, then per-backend input wiring):
- new `crates/flamewm-platform/src/sticky_keys.rs` (pure state machine, no I/O)
- edit `crates/flamewm-wm-wayland/src/input/shortcuts.rs` or `mod.rs` (one call site)
- edit `crates/flamewm-wm-x11/src/wm/events.rs` or the key path (one call site)
- settings bool in `crates/flamewm-platform/src/settings.rs` (add field and default only)

Warning: `crates/flamewm-wm-wayland/src/input/mod.rs` and `shortcuts.rs` are in the working tree with uncommitted changes in the parent repo. Serialize with that owner.

Spec:

```rust
// crates/flamewm-platform/src/sticky_keys.rs
pub enum Latch { Idle, Armed(Modifier), Locked(Modifier) }

pub struct StickyKeys { state: Latch, enabled: bool }

impl StickyKeys {
    /// A modifier press arms it. A second press of the same modifier locks it.
    /// A non-modifier key releases an armed (not locked) modifier after the key.
    pub fn on_key(&mut self, key: KeyEvent) -> Option<KeyEvent> { /* returns the event with latched mods applied */ }
}
```

Rules:
- Off by default. `enabled` comes from the typed setting.
- Only Shift, Ctrl, Alt, Super latch. Other keys pass through.
- An armed modifier is released after the next non-modifier key.
- Two taps lock until tapped again.

Acceptance:
- Unit test: press Shift, then `a` gives `A`; Shift is released.
- Unit test: double-tap Ctrl locks; `c` then `v` both have Ctrl; a third Ctrl tap releases.
- Unit test: disabled state is a no-op (returns input unchanged).
- Command: `cargo test -p flamewm-platform sticky_keys` (not run here).

PSS estimate: about 0 (a few bytes of state; `unmeasured`).

Dependency group: serialized with `flamewm-wm-wayland/src/input/*` and `flamewm-wm-x11` key path, not independent.

## Rank 3: Shake pointer to locate

Why: KWin `shakecursor/` enlarges the pointer after shaking. FlameWM has no equivalent (0 `.rs` hits). Accessibility and low-vision gain. Event-driven, no timer.

Files (owner: `flamewm-platform` detector, then input wiring):
- new `crates/flamewm-platform/src/shake.rs` (pure detector)
- one call site in `crates/flamewm-wm-wayland/src/input/mod.rs` (pointer motion) and the X11 motion path

Spec:

```rust
// crates/flamewm-platform/src/shake.rs
pub struct ShakeDetector { ring: [(u32, i32); 8], len: usize, head: usize }

impl ShakeDetector {
    /// Feeds one motion sample (timestamp ms, dx). Returns true on a shake:
    /// at least 4 direction reversals with |dx| >= 40 px within 500 ms.
    pub fn feed(&mut self, t_ms: u32, dx: i32) -> bool { /* ring update, reversal count */ }
}
```

Rules:
- Only a size or theme boost of the pointer for 1.5 s. The boost ends on the next motion after the timeout (no timer thread; check the time on the next event).
- Off by default. One typed setting.

Acceptance:
- Unit test: four reversals of 60 px within 300 ms returns `true` once.
- Unit test: slow back-and-forth (one reversal per 800 ms) returns `false`.
- Unit test: small jitter (|dx| < 40) returns `false`.
- Command: `cargo test -p flamewm-platform shake` (not run here).

PSS estimate: about 0 (eight entries of fixed storage).

Dependency group: serialized with rank 2 (same input files).

## Rank 4 (conditional): Workspace switch OSD

Why: KWin `desktopchangeosd/` shows the new desktop for a short time. FlameWM has no workspace OSD (0 hits). Useful for a two-row grid.

Status: owner not mapped. Do not dispatch until the owner is confirmed. Candidate owners to check first: `crates/flamewm-desktop/src/` (surface host) and the workspace switch path in `crates/flamewm-wm-wayland/src/`. `UI never becomes WM authority` (`AGENTS.md`), so the WM emits the switch event and the UI draws.

Spec sketch: transient surface created on switch, destroyed after 1 s. Zero idle memory when not shown.

Acceptance: a unit test for the timeout state, plus a runtime check that the surface is gone after 1 s (`unverified` until run).

PSS estimate: `unmeasured` (one surface only while shown).

## Dependency summary

| Group | Winners | Files | Parallel? |
|---|---|---|---|
| A | 1 (clipboard history) | `crates/flamewm-clipboard/**` | Independent of B and C |
| B | 2, 3 (sticky keys, shake) | `crates/flamewm-platform/src/{sticky_keys,shake}.rs`, input call sites | Serialize with each other |
| C | 4 (workspace OSD, conditional) | owner TBD | After owner check |

Groups A and B do not overlap, so A and B can be dispatched in parallel. Group B shares input files with uncommitted parent-repo changes, so it waits for that owner.
