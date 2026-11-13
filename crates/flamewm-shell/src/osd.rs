//! Volume and brightness OSD: one retained overlay surface driven by the system snapshot.
//!
//! The surface is created lazily on the first visible change, never shown for a
//! kind's baseline snapshot, and hidden at the single deadline. No timer is owned
//! here; the shell loop must wake at [`OsdController::deadline`].

use std::time::{Duration, Instant};

use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_api::Rect;
use flamewm_ui_x11::{
    decode_document, SurfaceConfig, SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime,
    UiBackendError, UiDocumentAccess, UiTemplate,
};

use crate::settings_projection::{self, SurfaceKind};

const COMPILED_UI: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-osd.rwr"));

pub const OSD_WIDTH: i32 = 215;
pub const OSD_HEIGHT: i32 = 37;
pub const OSD_RADIUS: i32 = 18;
pub const TRACK_W: i32 = 129;
pub const TRACK_H: i32 = 5;
pub const HOLD: Duration = Duration::from_millis(1200);
pub const FADE: Duration = Duration::from_millis(180);
/// Workspace card hold. With [`FADE`] the card is gone 800 ms after the switch.
pub const WORKSPACE_HOLD: Duration = Duration::from_millis(620);
/// Settings key that turns the workspace switch card on. Default off.
pub const WORKSPACE_OSD_KEY: &str = "workspaceOsd";

/// Which system value the card shows. Each kind keeps its own baseline, so a
/// volume change never re-shows a stale brightness level and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
}

impl OsdKind {
    fn slot(self) -> usize {
        match self {
            Self::Volume => 0,
            Self::Brightness => 1,
        }
    }
}

/// One level as the shell observes it in the snapshot, clamped to the live cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    pub kind: OsdKind,
    /// Percent as shown and scaled: volume against `max`, brightness against 100.
    pub volume: u8,
    pub muted: bool,
    /// Full-track percent: the live volume cap, or 100 for brightness.
    pub max: u8,
}

impl Level {
    /// `volume` is the sink's real level, never clamped: the card must show what the sink holds.
    /// `max` is the live volume cap (`AudioSettings::max_percent`): 150 with raise maximum, else
    /// 100. It only sets the full-track scale, so lowering the cap moves no observed level.
    #[must_use]
    pub fn new(volume: u8, muted: bool, max: u8) -> Self {
        Self {
            kind: OsdKind::Volume,
            volume,
            muted,
            max,
        }
    }

    /// Panel backlight percentage (`0..=100`); never muted.
    #[must_use]
    pub fn brightness(percent: u8) -> Self {
        Self {
            kind: OsdKind::Brightness,
            volume: percent.min(100),
            muted: false,
            max: 100,
        }
    }

    /// Filled track width in logical px, full at `max`. Muted shows an empty track.
    #[must_use]
    pub fn fill_width(self) -> i32 {
        if self.muted || self.max == 0 {
            return 0;
        }
        (i32::from(self.volume) * TRACK_W / i32::from(self.max)).min(TRACK_W)
    }

    #[must_use]
    pub fn label(self) -> String {
        if self.muted {
            "Muted".to_owned()
        } else {
            format!("{}%", self.volume)
        }
    }
}

/// One output as the OSD sees it: geometry, shell scale, and identity flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OsdOutput {
    pub geometry: Rect,
    pub scale_percent: u16,
    pub primary: bool,
    pub connected: bool,
}

/// Output the card centres on: the shown panel's output, else the snapshot
/// primary, else the first connected output, else the first output. `None`
/// only when there are no outputs at all.
#[must_use]
pub fn target_output(panel: Option<OsdOutput>, outputs: &[OsdOutput]) -> Option<OsdOutput> {
    panel
        .or_else(|| outputs.iter().copied().find(|output| output.primary))
        .or_else(|| outputs.iter().copied().find(|output| output.connected))
        .or_else(|| outputs.first().copied())
}

/// Device-px size of the card at one output scale (rounded per axis).
#[must_use]
pub fn device_size(scale: f32) -> (u32, u32) {
    (
        (OSD_WIDTH as f32 * scale).round() as u32,
        (OSD_HEIGHT as f32 * scale).round() as u32,
    )
}

/// Card rect centred on the full `output` geometry (not the strut-reduced work
/// area), clamped so it never leaves the output. Signed origins are preserved.
#[must_use]
pub fn placement(output: Rect, scale: f32) -> Rect {
    let (width, height) = device_size(scale);
    let width = width as i32;
    let height = height as i32;
    let x = output.x + (output.width - width).div_euclid(2);
    let y = output.y + (output.height - height).div_euclid(2);
    let max_x = (output.x + output.width - width).max(output.x);
    let max_y = (output.y + output.height - height).max(output.y);
    Rect::new(
        x.clamp(output.x, max_x),
        y.clamp(output.y, max_y),
        width,
        height,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Hold,
    Fade,
}

/// Coalescing state: one deadline shared by every change.
#[derive(Debug)]
pub struct OsdState {
    /// Last observed level per `OsdKind` slot; the first observation of each kind is a baseline.
    last: [Option<Level>; 2],
    /// Last observed active workspace index; the first observation is the baseline.
    last_workspace: Option<usize>,
    stage: Stage,
    deadline: Option<Instant>,
    /// Fade after the hold; `ZERO` under reducedMotion, so the card hides at the end of its hold.
    fade: Duration,
}

impl OsdState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: [None, None],
            last_workspace: None,
            stage: Stage::Hold,
            deadline: None,
            fade: FADE,
        }
    }

    /// Applies `flamewm_api::motion::motion_scale` (hundredths; 0 = reducedMotion, no fade).
    pub fn set_motion(&mut self, scale: u16) {
        self.fade = if scale == 0 {
            Duration::ZERO
        } else {
            FADE * 100 / u32::from(scale)
        };
    }

    /// Records one observed level for its kind. The first observation of a kind is
    /// the baseline and never shows. Returns `true` when the card must be painted
    /// and shown. Each change replaces the single deadline with a fresh hold.
    pub fn observe(&mut self, level: Level, now: Instant) -> bool {
        let Some(previous) = self.last[level.kind.slot()].replace(level) else {
            return false;
        };
        // Only the sink reading moves the card. `max` is display scale, so a cap change is not a
        // change in what the sink holds.
        if previous.kind == level.kind
            && previous.volume == level.volume
            && previous.muted == level.muted
        {
            return false;
        }
        self.stage = Stage::Hold;
        self.deadline = Some(now + HOLD);
        true
    }

    /// Records the active workspace index. The first observation is the baseline and never
    /// shows. A change starts the workspace hold when `show` is set. It is recorded even when
    /// the setting is off, so enabling it later never replays a stale switch.
    pub fn observe_workspace(&mut self, index: usize, show: bool, now: Instant) -> bool {
        let Some(previous) = self.last_workspace.replace(index) else {
            return false;
        };
        if previous == index || !show {
            return false;
        }
        self.stage = Stage::Hold;
        self.deadline = Some(now + WORKSPACE_HOLD);
        true
    }

    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Advances the single deadline. Returns `true` when the card must hide.
    pub fn expire(&mut self, now: Instant) -> bool {
        let Some(at) = self.deadline else {
            return false;
        };
        if now < at {
            return false;
        }
        match self.stage {
            Stage::Hold if self.fade.is_zero() => {
                self.stage = Stage::Hold;
                self.deadline = None;
                true
            }
            Stage::Hold => {
                self.stage = Stage::Fade;
                self.deadline = Some(now + self.fade);
                false
            }
            Stage::Fade => {
                self.stage = Stage::Hold;
                self.deadline = None;
                true
            }
        }
    }
}

impl Default for OsdState {
    fn default() -> Self {
        Self::new()
    }
}

/// Owns the single retained overlay surface and its coalescing state. The shell
/// loop publishes [`OsdController::deadline`] to the reactor wake slot, so it
/// wakes exactly then; with no deadline the loop idles.
#[derive(Default)]
pub struct OsdController {
    state: OsdState,
    surface: Option<SurfaceHandle>,
    /// True while the surface shows the workspace card; that surface is destroyed at hide.
    workspace_card: bool,
}

impl OsdController {
    /// The pending hide deadline, if a card is showing.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.state.deadline()
    }

    /// Feeds one snapshot level per kind, hides at the deadline, then re-arms the timer.
    /// `None` means the kind has no state to show (no default endpoint, no backlight).
    /// `settings` styles the card (accent, surface fill, backdrop) on every show; `None`
    /// keeps the stylesheet defaults. When both kinds changed in one tick the card is
    /// painted once, brightness last.
    pub fn update(
        &mut self,
        runtime: &mut SurfaceRuntime,
        volume: Option<Level>,
        brightness: Option<Level>,
        target: impl FnOnce() -> (Rect, f32),
        settings: Option<&SettingsSnapshot>,
        now: Instant,
    ) -> Result<(), UiBackendError> {
        let result = self.observe(runtime, volume, brightness, target, settings, now);
        result.and_then(|()| self.expire(runtime, now))
    }

    fn observe(
        &mut self,
        runtime: &mut SurfaceRuntime,
        volume: Option<Level>,
        brightness: Option<Level>,
        target: impl FnOnce() -> (Rect, f32),
        settings: Option<&SettingsSnapshot>,
        now: Instant,
    ) -> Result<(), UiBackendError> {
        if let Some(settings) = settings {
            self.state
                .set_motion(flamewm_api::motion::motion_scale(settings));
        }
        let volume_changed = volume.is_some_and(|level| self.state.observe(level, now));
        let brightness_changed = brightness.is_some_and(|level| self.state.observe(level, now));
        let Some(level) = brightness
            .filter(|_| brightness_changed)
            .or_else(|| volume.filter(|_| volume_changed))
        else {
            return Ok(());
        };
        // The output lookup runs only when the card shows, so idle ticks allocate nothing.
        let (work, scale) = target();
        let surface = self.ensure_surface(runtime)?;
        self.workspace_card = false;
        let scale = runtime.apply_output_scale(surface, scale)?;
        let card = placement(work, scale);
        if let Some(settings) = settings {
            runtime.with_document(surface, |document| {
                settings_projection::project(document, settings, SurfaceKind::Osd)
            })?;
            let (opacity, blur) = settings_projection::start_backdrop_percents(settings);
            runtime.set_popup_backdrop(surface, Some((opacity, blur)))?;
        }
        runtime.with_document(surface, |document| paint(document, level))?;
        runtime.move_resize(
            surface,
            card.x,
            card.y,
            card.width as u32,
            card.height as u32,
        )?;
        runtime.show(surface)?;
        runtime.redraw(surface)
    }

    /// Feeds the active workspace index (`None` when the snapshot has no workspaces). A change
    /// shows the workspace card for [`WORKSPACE_HOLD`] + [`FADE`] when `workspaceOsd` is on. The
    /// card is flat: no backdrop, so no blur. Call before [`OsdController::update`], which owns
    /// the shared hide deadline.
    pub fn update_workspace(
        &mut self,
        runtime: &mut SurfaceRuntime,
        active: Option<usize>,
        names: &[String],
        target: impl FnOnce() -> (Rect, f32),
        settings: Option<&SettingsSnapshot>,
        now: Instant,
    ) -> Result<(), UiBackendError> {
        let Some(index) = active else {
            return Ok(());
        };
        if !self
            .state
            .observe_workspace(index, workspace_osd_enabled(settings), now)
        {
            return Ok(());
        }
        let label = workspace_label(index, names);
        let (work, scale) = target();
        let surface = self.ensure_surface(runtime)?;
        let scale = runtime.apply_output_scale(surface, scale)?;
        let card = placement(work, scale);
        if let Some(settings) = settings {
            runtime.with_document(surface, |document| {
                settings_projection::project(document, settings, SurfaceKind::Osd)
            })?;
        }
        runtime.set_popup_backdrop(surface, None)?;
        runtime.with_document(surface, |document| paint_workspace(document, &label))?;
        runtime.move_resize(
            surface,
            card.x,
            card.y,
            card.width as u32,
            card.height as u32,
        )?;
        runtime.show(surface)?;
        self.workspace_card = true;
        runtime.redraw(surface)
    }

    fn ensure_surface(
        &mut self,
        runtime: &mut SurfaceRuntime,
    ) -> Result<SurfaceHandle, UiBackendError> {
        if let Some(surface) = self.surface {
            return Ok(surface);
        }
        let surface = create(runtime)?;
        self.surface = Some(surface);
        Ok(surface)
    }

    /// Hides the card once the single deadline has fully elapsed. A workspace card destroys its
    /// surface, so an idle shell keeps no OSD surface after a switch card.
    fn expire(&mut self, runtime: &mut SurfaceRuntime, now: Instant) -> Result<(), UiBackendError> {
        let Some(surface) = self.surface else {
            return Ok(());
        };
        if !self.state.expire(now) {
            return Ok(());
        }
        if self.workspace_card {
            self.workspace_card = false;
            self.surface = None;
            runtime.destroy(surface)
        } else {
            runtime.hide(surface)
        }
    }
}

fn create(runtime: &mut SurfaceRuntime) -> Result<SurfaceHandle, UiBackendError> {
    let document = decode_document(COMPILED_UI).map_err(UiBackendError::Document)?;
    runtime.create_surface(
        UiTemplate::new(document),
        SurfaceConfig {
            width: OSD_WIDTH as u32,
            height: OSD_HEIGHT as u32,
            title: "FlameWM OSD".to_owned(),
            role: SurfaceRole::Overlay,
            input: SurfaceInputMode::PassThrough,
            initially_visible: false,
            x: 0,
            y: 0,
        },
    )
}

/// Whether `workspaceOsd` is on. Default off: only an explicit `true` shows the card.
#[must_use]
pub fn workspace_osd_enabled(settings: Option<&SettingsSnapshot>) -> bool {
    matches!(
        settings.and_then(|settings| settings.values.get(WORKSPACE_OSD_KEY)),
        Some(SettingValue::Boolean(true))
    )
}

/// Card text for the 0-based workspace `index`: "N  Name" when named, else "Workspace N".
#[must_use]
pub fn workspace_label(index: usize, names: &[String]) -> String {
    let number = index + 1;
    match names
        .get(index)
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
    {
        Some(name) => format!("{number}  {name}"),
        None => format!("Workspace {number}"),
    }
}

/// Visible state of the three OSD icons: exactly one of the volume pair, or the
/// brightness glyph.
fn icons(kind: OsdKind, muted: bool) -> (bool, bool, bool) {
    match kind {
        OsdKind::Volume => (!muted, muted, false),
        OsdKind::Brightness => (false, false, true),
    }
}

/// Workspace card: the number and name only. The volume icons, track and fill stay hidden.
fn paint_workspace(document: &mut impl UiDocumentAccess, label: &str) -> Result<(), String> {
    document.visible("osd-icon-high", false)?;
    document.visible("osd-icon-muted", false)?;
    document.visible("osd-icon-brightness", false)?;
    document.visible("osd-track", false)?;
    document.visible("osd-fill", false)?;
    document.visible("osd-text", false)?;
    document.visible("osd-workspace", true)?;
    document.text("osd-workspace", label.to_owned())
}

fn paint(document: &mut impl UiDocumentAccess, level: Level) -> Result<(), String> {
    let (high, muted, brightness) = icons(level.kind, level.muted);
    document.visible("osd-workspace", false)?;
    document.visible("osd-track", true)?;
    document.visible("osd-fill", true)?;
    document.visible("osd-text", true)?;
    document.visible("osd-icon-high", high)?;
    document.visible("osd-icon-muted", muted)?;
    document.visible("osd-icon-brightness", brightness)?;
    document.size("osd-fill", level.fill_width() as f32, TRACK_H as f32)?;
    document.text("osd-text", level.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn settings_with_workspace_osd(value: Option<bool>) -> SettingsSnapshot {
        let mut values = BTreeMap::new();
        if let Some(value) = value {
            values.insert(WORKSPACE_OSD_KEY.to_owned(), SettingValue::Boolean(value));
        }
        SettingsSnapshot {
            revision: 1,
            values,
        }
    }

    fn work(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect::new(x, y, width, height)
    }

    #[test]
    fn workspace_osd_is_off_unless_explicitly_on() {
        assert!(!workspace_osd_enabled(None));
        assert!(!workspace_osd_enabled(Some(&settings_with_workspace_osd(
            None
        ))));
        assert!(!workspace_osd_enabled(Some(&settings_with_workspace_osd(
            Some(false)
        ))));
        assert!(workspace_osd_enabled(Some(&settings_with_workspace_osd(
            Some(true)
        ))));
    }

    #[test]
    fn workspace_card_hides_at_800_ms_total() {
        assert_eq!(WORKSPACE_HOLD + FADE, Duration::from_millis(800));
    }

    #[test]
    fn workspace_label_uses_number_and_name_with_fallback() {
        assert_eq!(workspace_label(0, &[]), "Workspace 1");
        let names = [String::new(), "  Dev ".to_owned()];
        assert_eq!(workspace_label(0, &names), "Workspace 1");
        assert_eq!(workspace_label(1, &names), "2  Dev");
        assert_eq!(workspace_label(4, &names), "Workspace 5");
    }

    #[test]
    fn workspace_first_observation_is_baseline_and_change_shows() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        assert!(!state.observe_workspace(0, true, t0));
        assert_eq!(state.deadline(), None);
        assert!(state.observe_workspace(1, true, t0));
        assert_eq!(state.deadline(), Some(t0 + WORKSPACE_HOLD));
        assert!(!state.observe_workspace(1, true, t0 + Duration::from_millis(100)));
        assert_eq!(state.deadline(), Some(t0 + WORKSPACE_HOLD));
    }

    #[test]
    fn workspace_change_with_setting_off_never_shows_or_replays() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        assert!(!state.observe_workspace(0, false, t0));
        assert!(!state.observe_workspace(2, false, t0));
        assert_eq!(state.deadline(), None);
        assert!(
            !state.observe_workspace(2, true, t0),
            "re-enabling must not replay a switch"
        );
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn workspace_card_fades_then_hides_at_800_ms() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe_workspace(0, true, t0);
        state.observe_workspace(1, true, t0);
        assert!(!state.expire(t0 + (WORKSPACE_HOLD - Duration::from_millis(1))));
        assert!(!state.expire(t0 + WORKSPACE_HOLD));
        assert!(state.expire(t0 + Duration::from_millis(800)));
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn card_centres_on_full_output_at_100_percent() {
        assert_eq!(
            placement(work(0, 0, 1350, 641), 1.0),
            Rect::new(567, 302, 215, 37)
        );
    }

    fn out(x: i32, primary: bool, connected: bool) -> OsdOutput {
        OsdOutput {
            geometry: Rect::new(x, 0, 1280, 800),
            scale_percent: 100,
            primary,
            connected,
        }
    }

    #[test]
    fn target_prefers_panel_output_then_primary_then_connected() {
        let a = out(0, true, true);
        let b = out(1280, false, true);
        assert_eq!(target_output(Some(b), &[a, b]), Some(b));
        assert_eq!(target_output(None, &[a, b]), Some(a));
        assert_eq!(target_output(None, &[out(0, false, false), b]), Some(b));
        assert_eq!(target_output(None, &[]), None);
    }

    #[test]
    fn no_primary_flagged_single_output_centres_on_that_output() {
        let only = out(0, false, true);
        let target = target_output(None, &[only]).expect("one output");
        assert_eq!(
            placement(target.geometry, 1.0),
            Rect::new(532, 381, 215, 37)
        );
    }

    #[test]
    fn two_outputs_with_panel_on_second_centres_on_second() {
        let first = out(0, true, true);
        let second = out(1280, false, true);
        let target = target_output(Some(second), &[first, second]).expect("target");
        assert_eq!(target.geometry.x, 1280);
        assert_eq!(
            placement(target.geometry, 1.0),
            Rect::new(1812, 381, 215, 37)
        );
    }

    #[test]
    fn card_clamps_inside_an_output_smaller_than_the_card() {
        assert_eq!(
            placement(work(0, 0, 150, 20), 1.0),
            Rect::new(0, 0, 215, 37)
        );
    }

    #[test]
    fn card_keeps_negative_origin_for_left_output() {
        assert_eq!(
            placement(work(-1920, -40, 1920, 1080), 1.0),
            Rect::new(-1068, 481, 215, 37)
        );
    }

    #[test]
    fn scale_125_and_150_round_device_size() {
        assert_eq!(device_size(1.25), (269, 46));
        assert_eq!(device_size(1.5), (323, 56));
        assert_eq!(
            placement(work(0, 0, 1920, 1080), 1.5),
            Rect::new(798, 512, 323, 56)
        );
        assert_eq!(
            placement(work(-1920, 0, 1920, 1080), 1.25),
            Rect::new(-1095, 517, 269, 46)
        );
    }

    #[test]
    fn fill_width_at_zero_quarter_half_full_and_max() {
        let fill = |volume| Level::new(volume, false, 150).fill_width();
        assert_eq!(fill(0), 0);
        assert_eq!(fill(25), 21);
        assert_eq!(fill(50), 43);
        assert_eq!(fill(100), 86);
        assert_eq!(fill(150), TRACK_W);
    }

    #[test]
    fn volume_above_max_fills_the_track_and_shows_the_real_level() {
        assert_eq!(Level::new(200, false, 150).fill_width(), TRACK_W);
        assert_eq!(Level::new(200, false, 150).label(), "200%");
    }

    #[test]
    fn without_raise_maximum_the_card_shows_the_real_level_over_a_full_track() {
        let level = Level::new(120, false, 100);
        assert_eq!(level.label(), "120%");
        assert_eq!(level.fill_width(), TRACK_W);
        assert_eq!(Level::new(60, false, 100).fill_width(), 77);
        assert_eq!(Level::new(60, false, 100).label(), "60%");
    }

    #[test]
    fn raise_maximum_shows_up_to_150_percent() {
        assert_eq!(Level::new(120, false, 150).label(), "120%");
        assert_eq!(Level::new(120, false, 150).fill_width(), 103);
    }

    #[test]
    fn brightness_level_scales_against_100_and_is_never_muted() {
        let level = Level::brightness(140);
        assert_eq!(level.kind, OsdKind::Brightness);
        assert_eq!(level.label(), "100%");
        assert_eq!(level.fill_width(), TRACK_W);
        assert_eq!(Level::brightness(50).fill_width(), 64);
        assert_eq!(Level::brightness(0).fill_width(), 0);
        assert_eq!(Level::brightness(70).label(), "70%");
    }

    #[test]
    fn lowering_the_cap_alone_never_shows_a_card() {
        // Regression: the cap used to clamp the observed level, so a cap change with an unchanged
        // sink showed a card at the clamped value. The sink did not move, so no card may show.
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe(Level::new(120, false, 150), t0);
        assert!(!state.observe(Level::new(120, false, 100), t0));
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn mute_shows_empty_track_and_muted_label() {
        let level = Level::new(80, true, 150);
        assert_eq!(level.fill_width(), 0);
        assert_eq!(level.label(), "Muted");
    }

    #[test]
    fn device_width_scales_once_not_squared() {
        for (scale, expected) in [(1.0_f32, 215), (1.25, 269), (1.5, 323), (2.0, 430)] {
            let card = placement(work(0, 0, 3840, 2160), scale);
            assert_eq!(card.width, expected, "scale {scale}");
            assert_eq!(card.height, (37.0 * scale).round() as i32, "scale {scale}");
        }
        assert_ne!(placement(work(0, 0, 3840, 2160), 1.5).width, 484);
    }

    #[test]
    fn baseline_observation_does_not_show() {
        let mut state = OsdState::new();
        assert!(!state.observe(Level::new(40, false, 150), Instant::now()));
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn rejected_key_leaves_the_snapshot_level_and_never_shows() {
        // A media key only reaches the OSD through the snapshot. A key whose write the backend
        // rejects leaves the sink level unchanged, so the observed level repeats and no card shows.
        let mut state = OsdState::new();
        let t0 = Instant::now();
        assert!(!state.observe(Level::new(40, false, 150), t0));
        assert!(!state.observe(Level::new(40, false, 150), t0));
        assert_eq!(state.deadline(), None);
        assert!(state.observe(Level::new(45, false, 150), t0));
    }

    #[test]
    fn first_observation_of_each_kind_is_its_own_baseline() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        assert!(!state.observe(Level::new(40, false, 150), t0));
        assert!(!state.observe(Level::brightness(70), t0));
        assert_eq!(state.deadline(), None);
        assert!(state.observe(Level::brightness(75), t0));
    }

    #[test]
    fn switching_kind_shows_the_new_card_and_repeating_it_does_not() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        // Both kinds take their baseline first; only later changes paint.
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::brightness(65), t0);
        assert!(state.observe(Level::new(45, false, 150), t0));
        assert!(state.observe(Level::brightness(70), t0 + Duration::from_millis(100)));
        assert_eq!(
            state.deadline(),
            Some(t0 + Duration::from_millis(100) + HOLD)
        );
        assert!(!state.observe(Level::brightness(70), t0 + Duration::from_millis(400)));
        assert_eq!(
            state.deadline(),
            Some(t0 + Duration::from_millis(100) + HOLD)
        );
        assert!(state.observe(Level::new(50, false, 150), t0 + Duration::from_millis(500)));
    }

    #[test]
    fn exactly_one_icon_is_visible_per_kind() {
        assert_eq!(icons(OsdKind::Volume, false), (true, false, false));
        assert_eq!(icons(OsdKind::Volume, true), (false, true, false));
        assert_eq!(icons(OsdKind::Brightness, false), (false, false, true));
    }

    /// The card resolves its brightness glyph at build time, so a renamed asset
    /// fails the shell build rather than the first key press.
    #[test]
    fn osd_document_declares_the_brightness_icon() {
        let source = include_str!("../../../ui/shell/osd.html");
        assert!(source.contains(r#"id="osd-workspace""#));
        assert!(source.contains(r#"id="osd-icon-brightness""#));
        assert!(source.contains("assets/breeze/video-display-brightness.svg"));
        assert!(source.contains(r"<title>FlameWM OSD</title>"));
    }

    #[test]
    fn unchanged_level_does_not_reset_deadline() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        assert!(!state.observe(Level::new(40, false, 150), t0 + Duration::from_millis(500)));
        assert!(state.observe(Level::new(45, false, 150), t0 + Duration::from_millis(500)));
        assert_eq!(
            state.deadline(),
            Some(t0 + Duration::from_millis(500) + HOLD)
        );
    }

    #[test]
    fn coalesced_changes_keep_one_deadline_that_moves_forward() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::new(45, false, 150), t0 + Duration::from_millis(100));
        state.observe(Level::new(50, false, 150), t0 + Duration::from_millis(900));
        assert_eq!(
            state.deadline(),
            Some(t0 + Duration::from_millis(900) + HOLD)
        );
        assert!(!state.expire(t0 + HOLD));
        assert_eq!(
            state.deadline(),
            Some(t0 + Duration::from_millis(900) + HOLD)
        );
    }

    #[test]
    fn hold_then_fade_then_hide_leaves_no_deadline() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::new(41, false, 150), t0);
        assert!(!state.expire(t0 + (HOLD - Duration::from_millis(1))));
        assert!(!state.expire(t0 + HOLD));
        assert_eq!(state.deadline(), Some(t0 + HOLD + FADE));
        assert!(state.expire(t0 + HOLD + FADE));
        assert_eq!(state.deadline(), None);
        assert!(!state.expire(t0 + HOLD * 10));
    }

    #[test]
    fn reduced_motion_hides_at_the_end_of_hold_without_a_fade() {
        let mut state = OsdState::new();
        state.set_motion(0);
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::new(41, false, 150), t0);
        assert!(!state.expire(t0 + (HOLD - Duration::from_millis(1))));
        assert!(state.expire(t0 + HOLD));
        assert_eq!(state.deadline(), None);
    }

    #[test]
    fn fade_duration_follows_motion_scale() {
        let mut state = OsdState::new();
        state.set_motion(50);
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::new(41, false, 150), t0);
        assert!(!state.expire(t0 + HOLD));
        assert_eq!(state.deadline(), Some(t0 + HOLD + FADE * 2));
    }

    #[test]
    fn change_during_fade_restarts_hold() {
        let mut state = OsdState::new();
        let t0 = Instant::now();
        state.observe(Level::new(40, false, 150), t0);
        state.observe(Level::new(41, false, 150), t0);
        assert!(!state.expire(t0 + HOLD));
        let t1 = t0 + HOLD + Duration::from_millis(50);
        assert!(state.observe(Level::new(41, true, 150), t1));
        assert_eq!(state.deadline(), Some(t1 + HOLD));
        assert!(!state.expire(t1 + (HOLD - Duration::from_millis(1))));
    }

    /// The backdrop/XShape mask radius is read from the document ROOT (`body`) only,
    /// so the card radius must be declared on the root rule, not just on `.osd-card`.
    #[test]
    fn osd_root_declares_card_corner_radius() {
        let source = include_str!("../../../ui/shell/osd.html");
        let body_rule = source
            .split("body {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("osd.html declares a body rule");
        assert!(
            body_rule.contains(&format!("border-radius:{OSD_RADIUS}px")),
            "body rule must carry border-radius:{OSD_RADIUS}px, got: {body_rule}"
        );
    }
}
