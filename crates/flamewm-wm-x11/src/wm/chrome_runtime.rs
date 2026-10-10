//! Chrome runtime: cached metadata/invalidation/paint bridge.
//!
//! Owns title/class/icon refresh decisions, paint invalidation, and the
//! bridge from a cached [`ChromeScene`]/[`ChromePaintPlan`] to the external
//! renderer. Manage/property paths own all X round-trips, catalog, and
//! decode work; paint consumes cached rasters only via `J08` `paint_plan`
//! and the `with_icon`/`render` path. No icon I/O during paint.

use crate::chrome::IconImage;
use crate::frame::chrome::{
    ChromePaintPlan, ChromeScene, paint_plan, plan_scene, render, with_icon,
};
use crate::frame::model::FrameControl;

/// Cached identity inputs owned outside paint (manage/property paths).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChromeIdentity {
    pub title: String,
    pub wm_instance: String,
    pub wm_class: String,
}

/// State inputs that invalidate the cached scene when they change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeState {
    pub frame_w: u32,
    pub frame_h: u32,
    pub active: bool,
    pub hover: Option<FrameControl>,
    pub pressed: Option<FrameControl>,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// What changed after a manage/property refresh (already performed I/O).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChromeRefresh {
    pub title_changed: bool,
    pub class_changed: bool,
    pub icon_changed: bool,
}

impl ChromeRefresh {
    /// True when any refresh facet requires a repaint.
    #[must_use]
    pub fn needs_repaint(self) -> bool {
        self.title_changed || self.class_changed || self.icon_changed
    }
}

/// Decide whether already-fetched title/class values require a refresh.
/// Pure string comparison; performs no X or catalog work.
#[must_use]
pub fn decide_refresh(
    old: &ChromeIdentity,
    title: &str,
    wm_instance: &str,
    wm_class: &str,
    icon_changed: bool,
) -> ChromeRefresh {
    ChromeRefresh {
        title_changed: old.title != title,
        class_changed: old.wm_instance != wm_instance || old.wm_class != wm_class,
        icon_changed,
    }
}

/// Decide whether state inputs differ enough to invalidate the scene.
#[must_use]
pub fn state_changed(old: Option<ChromeState>, next: ChromeState) -> bool {
    old.map_or(true, |old| old != next)
}

/// Per-client chrome cache: last identity/state plus the planned scene.
/// Paint clones out of the cache; invalidation only flips `dirty`.
#[derive(Debug, Clone, Default)]
pub struct ChromeRuntime {
    identity: Option<ChromeIdentity>,
    state: Option<ChromeState>,
    icon_present: bool,
    scene: Option<ChromeScene>,
    dirty: bool,
}

impl ChromeRuntime {
    /// Empty cache; first `scene_for` call always (re)builds.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True when no scene is cached or an invalidation is pending.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty || self.scene.is_none()
    }

    /// Record a metadata refresh decision. Returns true when repaint is needed.
    pub fn note_refresh(&mut self, refresh: ChromeRefresh) -> bool {
        if refresh.needs_repaint() {
            self.dirty = true;
        }
        refresh.needs_repaint()
    }

    /// Record a state transition. Returns true when repaint is needed.
    pub fn note_state(&mut self, next: ChromeState) -> bool {
        if state_changed(self.state, next) {
            self.state = Some(next);
            self.dirty = true;
            return true;
        }
        false
    }

    /// Rebuild (when dirty) and return the cached scene for these inputs.
    /// `icon` is the already-cached raster; no lookup or decode happens here.
    pub fn scene_for(
        &mut self,
        identity: &ChromeIdentity,
        state: ChromeState,
        icon: Option<IconImage>,
    ) -> &ChromeScene {
        let identity_changed = self.identity.as_ref() != Some(identity);
        let state_dirty = state_changed(self.state, state);
        let icon_dirty = self.icon_present != icon.is_some();
        if identity_changed || state_dirty || icon_dirty || self.scene.is_none() {
            self.dirty = true;
        }
        if self.dirty || self.scene.is_none() {
            let planned = plan_scene(
                state.frame_w.max(1),
                state.frame_h.max(1),
                &identity.title,
                state.active,
                state.hover,
                state.pressed,
                state.maximized,
                state.fullscreen,
            );
            self.scene = Some(with_icon(planned, icon));
            self.identity = Some(identity.clone());
            self.state = Some(state);
            self.icon_present = self.scene.as_ref().is_some_and(|s| s.icon_present);
            self.dirty = false;
        }
        self.scene.as_ref().expect("scene rebuilt when missing")
    }

    /// Derive the production paint plan from the cached scene.
    /// Returns `None` when no scene has been planned yet.
    #[must_use]
    pub fn cached_plan(&self) -> Option<ChromePaintPlan> {
        self.scene.as_ref().map(paint_plan)
    }

    /// Paint bridge: execute the cached scene against the external renderer.
    /// Cached rasters only; no X round-trips, catalog, or decode work.
    pub fn paint_cached(
        &self,
        renderer: &mut flamewm_render_x11::ExternalDecorationRenderer,
        frame_xid: u64,
    ) -> Result<(), String> {
        match self.scene.as_ref() {
            Some(scene) => render(renderer, frame_xid, scene),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ChromeIdentity {
        ChromeIdentity {
            title: "xterm".to_owned(),
            wm_instance: "xterm".to_owned(),
            wm_class: "XTerm".to_owned(),
        }
    }

    fn state() -> ChromeState {
        ChromeState {
            frame_w: 320,
            frame_h: 240,
            active: true,
            hover: None,
            pressed: None,
            maximized: false,
            fullscreen: false,
        }
    }

    fn catalog_icon() -> IconImage {
        IconImage {
            width: 2,
            height: 2,
            argb: vec![0xff11_2233, 0xff44_5566, 0x8077_8899, 0x0000_0000],
        }
    }

    #[test]
    fn catalog_icon_is_used_for_titlebar_slot_and_retained_across_rebuild() {
        let mut runtime = ChromeRuntime::new();
        let scene = runtime.scene_for(&identity(), state(), Some(catalog_icon()));
        assert!(scene.icon_present, "catalog raster must reach the scene");
        assert_eq!(scene.icon.as_ref(), Some(&catalog_icon()));

        // Unchanged inputs reuse the cached scene and keep the raster.
        let scene = runtime.scene_for(&identity(), state(), Some(catalog_icon()));
        assert!(scene.icon_present, "cached rebuild must keep the raster");
        assert_eq!(scene.icon.as_ref(), Some(&catalog_icon()));

        // A state change (e.g. hover) rebuilds the scene; the icon must
        // still be carried from the cached raster the caller passes in.
        let mut hovered = state();
        hovered.hover = Some(FrameControl::Close);
        let scene = runtime.scene_for(&identity(), hovered, Some(catalog_icon()));
        assert!(scene.icon_present, "state rebuild must keep the raster");
        assert_eq!(scene.icon.as_ref(), Some(&catalog_icon()));
    }

    #[test]
    fn missing_icon_clears_slot_only_when_caller_passes_none() {
        let mut runtime = ChromeRuntime::new();
        runtime.scene_for(&identity(), state(), Some(catalog_icon()));
        let scene = runtime.scene_for(&identity(), state(), None);
        assert!(!scene.icon_present);
        assert!(scene.icon.is_none());
    }
}
