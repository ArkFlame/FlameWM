// Shared by `build.rs` (which emits the node pool into the compiled UI) and the runtime (which addresses it).
// Each screen row owns `POOL_RUNS` absolutely positioned text nodes; a run is a span of cells with one colour pair.
pub const POOL_ROWS: usize = 96;
// Dense full-screen TUIs (OpenCode, htop) alternate colour and block glyphs across a row.
// With only 64 runs, ui::fit merges the remaining block rectangles into a text node,
// discarding their geometry. 128 covers fully distinct 120-column rows without
// affecting the per-frame dirty-row path. Bounded at build time: no runtime node growth.
pub const POOL_RUNS: usize = 128;

/// Node id of run `slot` on row `row`. Generated ids are `r{row}s{slot}` and must match `build.rs` output.
#[must_use]
pub fn run_id(row: usize, slot: usize) -> String {
    format!("r{row}s{slot}")
}
