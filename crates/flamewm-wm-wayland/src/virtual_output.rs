//! Headless/virtual outputs for multi-head testing in the nested (winit) backend.
//!
//! The nested backend drives a single winit window, so with one `Output` there is no way to prove
//! multi-head behavior (`docs/backlog/displays.md`: "the nested winit backend exposes one output").
//! Setting `FLAMEWM_VIRTUAL_OUTPUTS=<N>` adds `N` extra smithay [`Output`] objects with a fixed mode,
//! placed side by side after the primary output through [`crate::output_layout::origin_after`]. Each
//! one gets its own wl_output global, and because the compositor installs
//! `OutputManagerState::new_with_xdg_output`, each wl_output automatically advertises an xdg-output
//! too (`.vendor/smithay-master/src/wayland/output/xdg.rs:58`). No client changes are needed: a
//! multi-head client sees the extra outputs with distinct names, modes and logical positions.
//!
//! The extra outputs are *offscreen*: the single winit framebuffer keeps painting the primary
//! output; the virtual ones exist so layout, per-output work area, surface enter/leave and
//! wl_output/xdg-output advertisement can be exercised. This is the same shape as mango's
//! `create_virtual_output` / headless output creation
//! (`.vendor/mangowm/src/dispatch/bind.c:114`, `.vendor/mangowm/src/manage/monitor.c:1071`: a headless
//! 1920x1080 output added to the multi backend), and smithay's anvil example (`Output::new` +
//! `change_current_state` + `create_global` per output). Nothing runs unless the env var is set and
//! the winit backend is active, so default single-output behavior is unchanged.

use std::cell::RefCell;

use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::Transform;

use crate::model::Rect;
use crate::state::State;

/// Number of additional outputs to create. `FLAMEWM_VIRTUAL_OUTPUTS=2` adds two outputs next to the
/// primary (three in total).
pub(crate) const VIRTUAL_OUTPUTS_ENV: &str = "FLAMEWM_VIRTUAL_OUTPUTS";

/// Physical mode of every virtual output. Offscreen outputs have no host window to size them, so a
/// fixed 1280x800 (matching the nested default content size) is used. Mango's headless precedent is
/// 1920x1080; the exact number is immaterial to multi-head advertisement and row placement.
const VIRTUAL_MODE: (i32, i32) = (1280, 800);

/// Upper bound so a typo cannot allocate an unbounded number of globals in the test compositor.
const MAX_VIRTUAL_OUTPUTS: usize = 16;

// The wl_output/xdg-output globals must outlive this function. The compositor is single-threaded
// (its state uses `Rc`/`RefCell`), so a thread-local keeps them alive without adding a field to
// `State`; the module owns them, and they are released when the loop thread ends.
thread_local! {
    static VIRTUAL: RefCell<Vec<(Output, GlobalId)>> = RefCell::new(Vec::new());
}

/// Parses the env value as a count of extra outputs. Empty/whitespace means none; a negative,
/// non-numeric or absurdly large value is an error rather than a silently ignored setting.
pub(crate) fn parse_count(text: &str) -> Result<usize, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(0);
    }
    let count: usize = text
        .parse()
        .map_err(|_| format!("expected a non-negative integer, got {text:?}"))?;
    if count > MAX_VIRTUAL_OUTPUTS {
        return Err(format!(
            "{text:?} exceeds the maximum of {MAX_VIRTUAL_OUTPUTS} virtual outputs"
        ));
    }
    Ok(count)
}

/// Installs the virtual outputs when `FLAMEWM_VIRTUAL_OUTPUTS` is set and the winit backend is
/// active. Called once, after the backend has sized the primary output. Existing single-output
/// behavior is untouched when the variable is absent or the native backend is selected.
pub(crate) fn install_from_env(state: &mut State) {
    // The winit backend is the only one with a single-output problem to work around.
    if state.backend.is_none() {
        return;
    }
    let Ok(text) = std::env::var(VIRTUAL_OUTPUTS_ENV) else {
        return;
    };
    let count = match parse_count(&text) {
        Ok(count) => count,
        Err(error) => {
            eprintln!("flamewm-wayland: {VIRTUAL_OUTPUTS_ENV} ignored: {error}");
            return;
        }
    };
    if count == 0 {
        return;
    }
    if VIRTUAL.with(|v| !v.borrow().is_empty()) {
        return; // already installed
    }

    let mut placed: Vec<(i32, i32)> = if state.pointer_layout.is_empty() {
        vec![(state.output_size.w, state.output_size.h)]
    } else {
        state
            .pointer_layout
            .iter()
            .map(|rect| (rect.w, rect.h))
            .collect()
    };
    // Keep names distinct from the `FLAMEWM_DEBUG_VIRTUAL_OUTPUTS` seam if both env vars are set.
    let first = state.layout_outputs.len().saturating_sub(1) + 1;
    let mut added = Vec::with_capacity(count);
    for ordinal in first..first + count {
        let output = Output::new(
            format!("virtual-{ordinal}"),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "FlameWM".to_owned(),
                model: "Virtual".to_owned(),
            },
        );
        let mode = Mode {
            size: VIRTUAL_MODE.into(),
            refresh: 60_000,
        };
        // Virtual outputs are 100% scale so physical mode == logical size; origin_after places the
        // next output at the end of the row, the same arithmetic DRM uses for a second connector.
        let origin = crate::output_layout::origin_after(&placed, VIRTUAL_MODE);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some(origin.into()),
        );
        output.set_preferred(mode);
        let global = output.create_global::<State>(&state.display);
        placed.push(VIRTUAL_MODE);
        added.push((output, global));
    }

    let outputs: Vec<Output> = added.iter().map(|(out, _)| out.clone()).collect();
    // Move the globals into the thread-local so they live until the loop thread exits.
    VIRTUAL.with(|v| *v.borrow_mut() = added);
    sync_layout(state, &outputs);
    eprintln!(
        "flamewm-wayland: virtual outputs enabled ({VIRTUAL_OUTPUTS_ENV}={count}): {}",
        state.layout_outputs.join(",")
    );
}

/// Makes the WM layout state include the newly advertised outputs, the way
/// `debug_outputs::sync_layout` / `drm::outputs::sync_pointer_layout` do for their outputs: names,
/// pointer rects (for hit testing and clamping), scales and surface homes all stay in step.
fn sync_layout(state: &mut State, added: &[Output]) {
    let primary = state.output.name();
    if state.layout_outputs.is_empty() {
        state.layout_outputs.push(primary.clone());
        state.pointer_layout.push(state.output_rect());
        state
            .output_scales
            .push((primary.clone(), state.output_scale()));
    }
    for out in added {
        let location = out.current_location();
        let size = out
            .current_mode()
            .map_or((0, 0), |mode| (mode.size.w, mode.size.h));
        let scale = out.current_scale().fractional_scale();
        state.output_handles.push(out.clone());
        state.layout_outputs.push(out.name());
        state
            .pointer_layout
            .push(Rect::new(location.x, location.y, size.0, size.1));
        state.output_scales.push((out.name(), scale));
    }
    let mut live = state.layout_outputs.clone();
    live.push(primary);
    state.surface_outputs.retain_outputs(&live);
    state.refresh_window_outputs();
    state.request_full_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_count_accepts_zero_and_positive_counts() {
        assert_eq!(parse_count("2"), Ok(2));
        assert_eq!(parse_count(" 1 "), Ok(1));
        assert_eq!(parse_count(""), Ok(0));
        assert_eq!(parse_count("16"), Ok(16));
    }

    #[test]
    fn parse_count_rejects_bad_values() {
        assert!(parse_count("two").is_err());
        assert!(parse_count("-1").is_err());
        assert!(parse_count("17").is_err());
        assert!(parse_count("1.5").is_err());
    }
}
