// Shared by `build.rs` (which emits one node pool per tier into its compiled UI) and the runtime (which addresses it).
// Each screen row owns `runs` absolutely positioned text nodes; a run is a span of cells with one colour pair.
// Dense full-screen TUIs (OpenCode, htop) alternate colour and block glyphs across a row, so runs per row must cover
// the columns; with fewer slots, ui::fit merges the remaining block rectangles into a text node and discards their
// geometry. Every slot is two document nodes, so tiers trade memory for capacity. The last tier is the maximum grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pool {
    /// Screen rows, including the tab bar.
    pub rows: usize,
    /// Run slots per row.
    pub runs: usize,
}

/// Smallest tier first. A window uses the smallest tier that fits its grid (see `app::pool_tier_for`).
pub const TIERS: [Pool; 3] = [
    Pool { rows: 24, runs: 80 },
    Pool {
        rows: 48,
        runs: 128,
    },
    Pool {
        rows: 96,
        runs: 192,
    },
];

/// Node id of run `slot` on row `row`. Generated ids are `r{row}s{slot}` and must match `build.rs` output.
#[must_use]
pub fn run_id(row: usize, slot: usize) -> String {
    format!("r{row}s{slot}")
}
