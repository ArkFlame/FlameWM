//! Debug-only virtual outputs for the nested backend.
//!
//! The nested backend drives one winit window, so no runtime path reaches the multi-output code
//! (per-output work area, window migration on output removal). This seam adds extra logical
//! outputs on top of the primary one, the way smithay's anvil example fakes outputs for testing:
//! `FLAMEWM_DEBUG_VIRTUAL_OUTPUTS="WxH@X,Y;..."` is read once at startup, and writing the flag file
//! named by `FLAMEWM_DEBUG_REMOVE_OUTPUT_FLAG` removes the most recently added virtual output on the
//! next loop turn. Nothing here runs unless one of those variables is set.
//!
//! Precedent: smithay `anvil` (`Output::new` + `change_current_state` + `create_global` per output)
//! and `drm/outputs.rs` `sync_pointer_layout` for the retire-then-relayout order.

use std::path::PathBuf;

use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::Transform;

use crate::model::Rect;
use crate::state::State;

pub(crate) const VIRTUAL_OUTPUTS_ENV: &str = "FLAMEWM_DEBUG_VIRTUAL_OUTPUTS";
pub(crate) const REMOVE_FLAG_ENV: &str = "FLAMEWM_DEBUG_REMOVE_OUTPUT_FLAG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Spec {
    pub w: i32,
    pub h: i32,
    pub x: i32,
    pub y: i32,
}

/// Parses `WxH@X,Y;WxH@X,Y`. Empty input means no virtual outputs; any malformed entry is an error.
pub(crate) fn parse(text: &str) -> Result<Vec<Spec>, String> {
    text.split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (size, origin) = entry
                .split_once('@')
                .ok_or_else(|| format!("missing '@' in {entry:?}"))?;
            let (w, h) = size
                .split_once('x')
                .ok_or_else(|| format!("missing 'x' in {entry:?}"))?;
            let (x, y) = origin
                .split_once(',')
                .ok_or_else(|| format!("missing ',' in {entry:?}"))?;
            let num = |v: &str| {
                v.trim()
                    .parse::<i32>()
                    .map_err(|_| format!("bad number {v:?} in {entry:?}"))
            };
            let spec = Spec {
                w: num(w)?,
                h: num(h)?,
                x: num(x)?,
                y: num(y)?,
            };
            if spec.w <= 0 || spec.h <= 0 {
                return Err(format!("non-positive size in {entry:?}"));
            }
            Ok(spec)
        })
        .collect()
}

/// Virtual outputs in creation order, with the global each one advertises.
#[derive(Default)]
pub(crate) struct VirtualOutputs {
    entries: Vec<(Output, GlobalId)>,
    flag: Option<PathBuf>,
}

impl VirtualOutputs {
    /// Origin and name of the newest virtual output, where new toplevels are placed under the seam.
    pub(crate) fn newest(&self) -> Option<(String, Rect)> {
        self.entries.last().map(|(out, _)| {
            let loc = out.current_location();
            let size = out.current_mode().map_or((0, 0), |m| (m.size.w, m.size.h));
            (out.name(), Rect::new(loc.x, loc.y, size.0, size.1))
        })
    }
}

/// Reads both env variables and installs the outputs. Called once the backend has its primary size.
pub(crate) fn install_from_env(state: &mut State) {
    state.debug_outputs.flag = std::env::var_os(REMOVE_FLAG_ENV).map(PathBuf::from);
    let Ok(text) = std::env::var(VIRTUAL_OUTPUTS_ENV) else {
        return;
    };
    match parse(&text) {
        Ok(specs) => {
            for (index, spec) in specs.into_iter().enumerate() {
                add(state, index + 1, spec);
            }
            sync_layout(state);
            eprintln!(
                "flamewm-wayland: debug virtual outputs {}",
                state
                    .layout_outputs
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            );
        }
        Err(error) => eprintln!("flamewm-wayland: {VIRTUAL_OUTPUTS_ENV} ignored: {error}"),
    }
}

fn add(state: &mut State, ordinal: usize, spec: Spec) {
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
        size: (spec.w, spec.h).into(),
        refresh: 60_000,
    };
    output.change_current_state(
        Some(mode),
        Some(Transform::Normal),
        Some(smithay::output::Scale::Integer(1)),
        Some((spec.x, spec.y).into()),
    );
    output.set_preferred(mode);
    let global = output.create_global::<State>(&state.display);
    state.debug_outputs.entries.push((output, global));
}

/// Removes the newest virtual output when the flag file exists; the file is consumed. Runs every
/// loop turn, but costs nothing unless `FLAMEWM_DEBUG_REMOVE_OUTPUT_FLAG` was set at startup.
pub(crate) fn poll_removal(state: &mut State) {
    let Some(flag) = state.debug_outputs.flag.clone() else {
        return;
    };
    if !flag.exists() {
        return;
    }
    let _ = std::fs::remove_file(&flag);
    let Some((out, global)) = state.debug_outputs.entries.pop() else {
        eprintln!("flamewm-wayland: debug remove requested with no virtual output");
        return;
    };
    let name = out.name();
    state.display.remove_global::<State>(global);
    state.forget_lock_output(&name);
    // `output_handles` still holds the removed handle, so `sync_layout` retires it.
    sync_layout(state);
    eprintln!("flamewm-wayland: debug virtual output {name} removed");
}

/// Makes the layout state match the live virtual outputs. Removed outputs are retired first, while
/// `layout_outputs`/`pointer_layout` still describe where their windows sit, then the layout is rebuilt
/// with the primary output as row zero, as `drm::outputs::sync_pointer_layout` does for connectors.
fn sync_layout(state: &mut State) {
    let primary = state.output.name();
    let live: Vec<Output> = state
        .debug_outputs
        .entries
        .iter()
        .map(|(out, _)| out.clone())
        .collect();
    let gone: Vec<Output> = state
        .output_handles
        .iter()
        .filter(|old| {
            let name = old.name();
            name != primary && !live.iter().any(|new| new.name() == name)
        })
        .cloned()
        .collect();
    state.output_handles.clone_from(&live);
    for out in &gone {
        state.retire_output(out);
    }
    let mut names = vec![primary.clone()];
    let mut rects = vec![state.output_rect()];
    let mut scales = vec![(primary.clone(), 1.0)];
    for out in &live {
        let mode = out.current_mode().map_or((0, 0), |m| (m.size.w, m.size.h));
        let loc = out.current_location();
        names.push(out.name());
        rects.push(Rect::new(loc.x, loc.y, mode.0, mode.1));
        scales.push((out.name(), 1.0));
    }
    state.layout_outputs = names;
    state.pointer_layout = rects;
    state.output_scales = scales;
    let mut live_names = state.layout_outputs.clone();
    live_names.push(primary);
    state.surface_outputs.retain_outputs(&live_names);
    state.refresh_window_outputs();
    // A layout change moves windows between outputs; nothing else damages the nested frame for it.
    state.request_full_redraw();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_size_and_origin_pairs() {
        assert_eq!(
            parse("800x600@1024,0; 640x480@0,640"),
            Ok(vec![
                Spec {
                    w: 800,
                    h: 600,
                    x: 1024,
                    y: 0
                },
                Spec {
                    w: 640,
                    h: 480,
                    x: 0,
                    y: 640
                },
            ])
        );
        assert_eq!(parse(""), Ok(vec![]));
    }

    #[test]
    fn rejects_malformed_or_empty_size() {
        assert!(parse("800x600").is_err());
        assert!(parse("800@0,0").is_err());
        assert!(parse("800x600@0").is_err());
        assert!(parse("0x600@0,0").is_err());
        assert!(parse("abcx600@0,0").is_err());
    }
}
