//! Volume and brightness OSD: one retained overlay surface driven by the system snapshot.
//!
//! The surface is created lazily on the first visible change, never shown for a
//! kind's baseline snapshot, and hidden at the single deadline. No timer is owned
//! here; the shell loop must wake at [`OsdController::deadline`].

use std::time::{Duration, Instant};

use flamewm_api::settings::SettingsSnapshot;
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
    stage: Stage,
    deadline: Option<Instant>,
}

impl OsdState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            last: [None, None],
            stage: Stage::Hold,
            deadline: None,
        }
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
            Stage::Hold => {
                self.stage = Stage::Fade;
                self.deadline = Some(now + FADE);
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
        let surface = if let Some(surface) = self.surface {
            surface
        } else {
            let surface = create(runtime)?;
            self.surface = Some(surface);
            surface
        };
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

    /// Hides the card once the single deadline has fully elapsed.
    fn expire(&mut self, runtime: &mut SurfaceRuntime, now: Instant) -> Result<(), UiBackendError> {
        match self.surface {
            Some(surface) if self.state.expire(now) => runtime.hide(surface),
            _ => Ok(()),
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

/// Visible state of the three OSD icons: exactly one of the volume pair, or the
/// brightness glyph.
fn icons(kind: OsdKind, muted: bool) -> (bool, bool, bool) {
    match kind {
        OsdKind::Volume => (!muted, muted, false),
        OsdKind::Brightness => (false, false, true),
    }
}

fn paint(document: &mut impl UiDocumentAccess, level: Level) -> Result<(), String> {
    let (high, muted, brightness) = icons(level.kind, level.muted);
    document.visible("osd-icon-high", high)?;
    document.visible("osd-icon-muted", muted)?;
    document.visible("osd-icon-brightness", brightness)?;
    document.size("osd-fill", level.fill_width() as f32, TRACK_H as f32)?;
    document.text("osd-text", level.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect::new(x, y, width, height)
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
