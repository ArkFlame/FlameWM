//! Split from `lib.rs` (`pager`); see crate docs.

use super::*;

/// Page-step direction for the output and mode pagers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PageStep {
    Prev,
    Next,
}

/// Page cursors for the output list (shared by the Displays and Taskbar pagers) and
/// the mode list. `generation` is the display snapshot generation the cursors were
/// last clamped against; a slot action carrying an older cursor is rejected.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DisplayPages {
    /// `None` until the first successful refresh or action stamps the cursors.
    pub(crate) generation: Option<u64>,
    pub(crate) output_page: usize,
    pub(crate) mode_page: usize,
}

/// Node ids for one pager: its two step buttons, their disabled `-off` twins, and the label.
pub(crate) struct PagerNodes {
    pub(crate) prev: &'static str,
    pub(crate) prev_off: &'static str,
    pub(crate) next: &'static str,
    pub(crate) next_off: &'static str,
    pub(crate) label: &'static str,
}

pub(crate) const OUTPUT_PAGER: PagerNodes = PagerNodes {
    prev: "display-output-prev",
    prev_off: "display-output-prev-off",
    next: "display-output-next",
    next_off: "display-output-next-off",
    label: "display-output-page-label",
};

pub(crate) const TASKBAR_OUTPUT_PAGER: PagerNodes = PagerNodes {
    prev: "taskbar-output-prev",
    prev_off: "taskbar-output-prev-off",
    next: "taskbar-output-next",
    next_off: "taskbar-output-next-off",
    label: "taskbar-output-page-label",
};

pub(crate) const MODE_PAGER: PagerNodes = PagerNodes {
    prev: "display-mode-prev",
    prev_off: "display-mode-prev-off",
    next: "display-mode-next",
    next_off: "display-mode-next-off",
    label: "display-mode-page-label",
};

pub(crate) fn page_count(len: usize, page_size: usize) -> usize {
    len.div_ceil(page_size)
}

/// Moves one page in `step`, clamped to `0..pages`. A step past either end is a no-op.
pub(crate) fn step_page(page: usize, step: PageStep, pages: usize) -> usize {
    let last = pages.saturating_sub(1);
    let page = page.min(last);
    match step {
        PageStep::Prev => page.saturating_sub(1),
        PageStep::Next => (page + 1).min(last),
    }
}

/// Absolute index of `slot` on `page`, i.e. `page * page_size + slot`. A cursor
/// stamped for another display generation is stale and rejected; a slot outside the
/// page, or an index past `len`, is a legible error. Nothing is applied on error.
pub(crate) fn page_slot_index(
    cursor_generation: u64,
    current_generation: u64,
    page: usize,
    page_size: usize,
    slot: usize,
    len: usize,
    what: &str,
) -> Result<usize, String> {
    if cursor_generation != current_generation {
        return Err(format!(
            "stale {what} page: cursor is for display generation {cursor_generation} but the authoritative generation is {current_generation}; page again"
        ));
    }
    if slot >= page_size {
        return Err(format!(
            "{what} slot {slot} is outside the page of {page_size}"
        ));
    }
    let index = page
        .checked_mul(page_size)
        .and_then(|base| base.checked_add(slot))
        .ok_or_else(|| format!("{what} page {page} overflows the index space"))?;
    if index >= len {
        return Err(format!("no {what} at index {index}; {len} available"));
    }
    Ok(index)
}

/// "1-4 of 5" for the items on `page`; "0 of 0" when there are none.
pub(crate) fn page_label(page: usize, page_size: usize, total: usize) -> String {
    if total == 0 {
        return "0 of 0".to_owned();
    }
    let start = page.saturating_mul(page_size).saturating_add(1).min(total);
    let end = page.saturating_add(1).saturating_mul(page_size).min(total);
    format!("{start}-{end} of {total}")
}

/// Projects one pager. A step button is visible only when it can move, and its
/// `-off` twin is visible exactly when the button is disabled.
pub(crate) fn project_pager(
    document: &mut impl UiDocumentAccess,
    nodes: &PagerNodes,
    page: usize,
    pages: usize,
    label: String,
) -> Result<(), String> {
    let has_prev = page > 0;
    let has_next = page + 1 < pages;
    document.visible(nodes.prev, has_prev)?;
    document.visible(nodes.prev_off, !has_prev)?;
    document.visible(nodes.next, has_next)?;
    document.visible(nodes.next_off, !has_next)?;
    document.text(nodes.label, label)
}

#[must_use]
pub fn normal_window_config(width: u32, height: u32) -> UiWindowConfig {
    UiWindowConfig {
        width,
        height,
        x: 0,
        y: 0,
        title: "System Settings".to_owned(),
        identity: Some(flamewm_ui_x11::X11WindowIdentity {
            instance: "flamewm-settings".to_owned(),
            class: "FlameWM Settings".to_owned(),
        }),
    }
}
