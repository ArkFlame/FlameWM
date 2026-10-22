// Shared by `build.rs` (which emits the node pool into the compiled UI) and the runtime (which addresses it).
// Each screen row owns `POOL_RUNS` absolutely positioned text nodes; a run is a span of cells with one colour pair.
pub const POOL_ROWS: usize = 96;
pub const POOL_RUNS: usize = 28;

/// Node id of run `slot` on row `row`. Generated ids are `r{row}s{slot}` and must match `build.rs` output.
pub fn run_id(row: usize, slot: usize) -> String {
    format!("r{row}s{slot}")
}
