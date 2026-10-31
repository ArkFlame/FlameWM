//! Passive X11 startup-notification observer and cursor-adjacent icon overlay.
//! No global cursor changes, pointer grabs or idle polling. One retained overlay is
//! created only on demand and its `XShape` input region is empty.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use flamewm_api::display::{DisplaySnapshot, OutputSnapshot};
use flamewm_api::settings::{SettingValue, SettingsSnapshot};
use flamewm_applications::launch_feedback::{Counters, Fragments, Sequences};
use flamewm_shell::icon_loader::{IconLoader, IconTarget};
use flamewm_ui_x11::{
    decode_document, RuntimeImage, StartupX11Event, StartupX11Observer, SurfaceConfig,
    SurfaceHandle, SurfaceInputMode, SurfaceRole, SurfaceRuntime, UiBackendError, UiDocumentAccess,
    UiTemplate,
};

const ARTIFACT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flamewm-launch-feedback.rwr"));
/// Pointer query cadence while the pointer moved recently (the badge must follow it).
const POLL_MOVING: Duration = Duration::from_millis(50);
/// Pointer query cadence once the pointer has been still for [`MOTION_WINDOW`].
const POLL_STILL: Duration = Duration::from_millis(250);
const MOTION_WINDOW: Duration = Duration::from_secs(1);
const BASE_ICON: i32 = 16;
const BASE_OFFSET: i32 = 13;

pub struct LaunchFeedback {
    observer: StartupX11Observer,
    fragments: Fragments,
    sequences: Sequences,
    surface: Option<SurfaceHandle>,
    visible: bool,
    requested: Option<(String, String, u32)>,
    painted: Option<(String, String, u32)>,
    loader: Option<IconLoader>,
    loader_root: PathBuf,
    icon_theme: String,
    last_poll: Option<Instant>,
    last_pointer: Option<(i32, i32)>,
    last_motion: Option<Instant>,
    indicator_frame: Option<usize>,
}

/// Pure cadence rule: fast while the pointer moved within [`MOTION_WINDOW`], slow once still.
#[must_use]
fn poll_interval(since_motion: Duration) -> Duration {
    if since_motion < MOTION_WINDOW {
        POLL_MOVING
    } else {
        POLL_STILL
    }
}

impl LaunchFeedback {
    pub fn connect(loader_root: PathBuf) -> Result<Self, String> {
        Ok(Self {
            observer: StartupX11Observer::connect()?,
            fragments: Fragments::default(),
            sequences: Sequences::default(),
            surface: None,
            visible: false,
            requested: None,
            painted: None,
            loader: None,
            loader_root,
            icon_theme: String::new(),
            last_poll: None,
            last_pointer: None,
            last_motion: None,
            indicator_frame: None,
        })
    }

    pub fn fd(&self) -> RawFd {
        self.observer.fd()
    }

    pub fn pointer_position(&self) -> Option<(i32, i32)> {
        self.observer.pointer_position()
    }

    /// Called only on readiness. The platform backend owns all X11 protocol I/O.
    pub fn on_ready(&mut self) -> Result<(), String> {
        let previous = self.sequences.counters;
        for event in self.observer.on_ready(self.sequences.active())? {
            match event {
                StartupX11Event::Fragment {
                    sender,
                    begin,
                    data,
                } => {
                    if let Some(message) = self.fragments.push(sender, begin, data) {
                        self.sequences.accept(message, Instant::now());
                    }
                }
                StartupX11Event::WindowIdentity {
                    startup_id,
                    classes,
                } => {
                    if self
                        .sequences
                        .complete_for_window(startup_id.as_deref(), None)
                        .is_none()
                    {
                        for class in classes {
                            if self
                                .sequences
                                .complete_for_window(None, Some(&class))
                                .is_some()
                            {
                                break;
                            }
                        }
                    }
                }
            }
        }
        self.report_transition(previous);
        Ok(())
    }

    fn report_transition(&self, previous: Counters) {
        let current = self.sequences.counters;
        if current == previous {
            return;
        }
        flamewm_debug::emit(
            flamewm_debug::DebugEventId("shell.launch-feedback.lifecycle"),
            Duration::ZERO,
            || {
                format!(
                    "started={} completed={} expired={}",
                    current.started, current.completed, current.expired
                )
            },
        );
    }

    /// No motion events are assumed to originate from `XFixes` cursor-image subscriptions.
    /// One active-only query per 50ms follows the pointer above *all* client windows.
    pub fn tick(
        &mut self,
        runtime: &mut SurfaceRuntime,
        displays: &DisplaySnapshot,
        settings: Option<&SettingsSnapshot>,
        now: Instant,
    ) -> Result<Option<Instant>, UiBackendError> {
        let previous = self.sequences.counters;
        self.sequences.expire(now);
        self.report_transition(previous);
        if self.sequences.visible().is_none() {
            self.hide(runtime)?;
            // A SILENT launch still needs its expiry deadline, without pointer polling.
            return Ok(self.sequences.next_deadline());
        }
        let since_motion = self
            .last_motion
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        let interval = poll_interval(since_motion);
        let should_poll = self
            .last_poll
            .is_none_or(|last| now.saturating_duration_since(last) >= interval);
        let icon_theme = settings
            .and_then(|s| s.values.get("iconTheme"))
            .and_then(|v| {
                if let SettingValue::Text(v) = v {
                    Some(v.as_str())
                } else {
                    None
                }
            })
            .unwrap_or("breeze");
        if self.icon_theme != icon_theme {
            icon_theme.clone_into(&mut self.icon_theme);
            if let Some(loader) = &mut self.loader {
                loader.set_theme(icon_theme);
            }
            self.requested = None;
            self.painted = None;
        }
        // Bounded active-only pointer query; XFixes cursor notifications do not report motion.
        if !should_poll && self.requested.is_some() && self.painted.is_some() {
            return Ok(Some(now + interval));
        }
        let Some((px, py)) = self.observer.pointer_position() else {
            self.hide(runtime)?;
            return Ok(self.sequences.next_deadline());
        };
        let moved = self.last_pointer != Some((px, py));
        if moved || self.last_motion.is_none() {
            self.last_motion = Some(now);
        }
        self.last_pointer = Some((px, py));
        let output = output_at(displays, px, py);
        let scale = output
            .map_or(100, |o| u32::from(o.shell_scale_percent))
            .clamp(50, 400);
        let side = ((BASE_ICON as u32 * scale + 50) / 100).clamp(8, 64);
        // Compare borrowed sequence fields before allocating a new request key.
        // Pointer updates must not clone app IDs/icons on every active poll.
        let (started, replacement) = {
            let sequence = self.sequences.visible().expect("checked above");
            (
                sequence.started,
                updated_launch_key(&sequence.id, &sequence.icon, side, self.requested.as_ref()),
            )
        };
        if let Some(key) = replacement {
            self.requested = Some(key);
            self.painted = None;
            self.indicator_frame = None;
            if let Some(surface) = self.surface {
                runtime.set_surface_shape_mask(surface, None)?;
                runtime.with_document(surface, |doc| {
                    for index in 0..4 {
                        doc.visible(&format!("launch-dot-{index}"), true)?;
                    }
                    doc.visible("launch-icon", false)
                })?;
                runtime.redraw(surface)?;
            }
            if self.loader.is_none() {
                self.loader = Some(IconLoader::spawn(
                    self.loader_root.clone(),
                    self.icon_theme.clone(),
                ));
            }
            if let (Some(loader), Some((_, icon, _))) = (&mut self.loader, &self.requested) {
                loader.request(IconTarget::TaskSlot(0), icon, side, side);
            }
        }
        // Maintain a bounded query interval while decoding is still pending.
        self.last_poll = Some(now);
        let results = self
            .loader
            .as_mut()
            .map(IconLoader::drain_ready)
            .unwrap_or_default();
        let mut icon_ready = false;
        for result in results {
            // The loader deduplicates by icon identity, not by startup ID. If
            // another launch of the same app begins during a pending decode,
            // the first result is still the correct raster for the new badge.
            if !self
                .requested
                .as_ref()
                .is_some_and(|(_, icon, requested_side)| {
                    matches_icon_result(
                        result.target,
                        &result.name,
                        result.width,
                        result.height,
                        icon,
                        *requested_side,
                    )
                })
            {
                continue;
            }
            let Some(raster) = result.raster else {
                continue;
            };
            let surface = self.surface(runtime)?;
            let image = RuntimeImage {
                source: raster.source,
                width: raster.width,
                height: raster.height,
                pixels: raster.pixels,
            };
            // XShape clips the 24-bit overlay at real PNG/SVG alpha boundaries, so
            // pixels outside the app glyph reveal the desktop instead of opaque black.
            let Some(shape) = alpha_shape_spans(&image.pixels, image.width, image.height) else {
                // A missing/fully transparent glyph must not create an opaque black overlay.
                // Keep the already-visible four-dot animation until the launch completes.
                continue;
            };
            runtime.set_surface_shape_mask(surface, Some(shape))?;
            runtime.with_document(surface, |doc| {
                doc.image_rgba8("launch-icon", image)?;
                doc.visible("launch-icon", true)?;
                for index in 0..4 {
                    doc.visible(&format!("launch-dot-{index}"), false)?;
                }
                Ok(())
            })?;
            self.painted = self.requested.clone();
            icon_ready = true;
        }
        let surface = self.surface(runtime)?;
        // No global cursor replacement and no pointer-event interception.
        let (x, mut y) = badge_position(px, py, side as i32, scale, output);
        // Opt-in only; global animation/reduced-motion policy always wins.
        let animate = settings.is_some_and(|s| {
            s.values.get("launchFeedbackAnimation") == Some(&SettingValue::Boolean(true))
                && s.values.get("animationsEnabled") != Some(&SettingValue::Boolean(false))
                && s.values.get("reducedMotion") != Some(&SettingValue::Boolean(true))
        });
        let animate_indicator = settings.is_none_or(|s| {
            s.values.get("animationsEnabled") != Some(&SettingValue::Boolean(false))
                && s.values.get("reducedMotion") != Some(&SettingValue::Boolean(true))
        });
        if self.painted.is_none() {
            let frame = if animate_indicator {
                ((now.saturating_duration_since(started).as_millis() / 120) % 4) as usize
            } else {
                0
            };
            if self.indicator_frame != Some(frame) {
                runtime.with_document(surface, |doc| {
                    for index in 0..4 {
                        let color = if index == frame {
                            flamewm_ui_x11::UiColor {
                                r: 101,
                                g: 168,
                                b: 255,
                                a: 255,
                            }
                        } else {
                            flamewm_ui_x11::UiColor {
                                r: 70,
                                g: 80,
                                b: 90,
                                a: 160,
                            }
                        };
                        doc.background(&format!("launch-dot-{index}"), color)?;
                    }
                    Ok(())
                })?;
                self.indicator_frame = Some(frame);
            }
        }
        if animate {
            let frame = ((now.saturating_duration_since(started).as_millis() / 140) % 4) as usize;
            y = y.saturating_sub([0, 1, 2, 1][frame]);
        }
        runtime.apply_output_scale(surface, scale as f32 / 100.0)?;
        runtime.move_resize(surface, x, y, side, side)?;
        let was_visible = self.visible;
        if !was_visible {
            runtime.show(surface)?;
            self.visible = true;
        }
        if moved || icon_ready || !was_visible {
            runtime.redraw(surface)?;
        }
        if self.painted != self.requested || should_poll {
            self.last_poll = Some(now);
            return Ok(Some(now + interval));
        }
        Ok(self.sequences.next_deadline())
    }

    fn surface(&mut self, runtime: &mut SurfaceRuntime) -> Result<SurfaceHandle, UiBackendError> {
        if let Some(surface) = self.surface {
            return Ok(surface);
        }
        let template = decode_document(ARTIFACT).map_err(UiBackendError::Document)?;
        let surface = runtime.create_surface(
            UiTemplate::new(template),
            SurfaceConfig {
                width: 16,
                height: 16,
                title: "FlameWM launch feedback".into(),
                role: SurfaceRole::Overlay,
                input: SurfaceInputMode::PassThrough,
                initially_visible: false,
                x: 0,
                y: 0,
            },
        )?;
        self.surface = Some(surface);
        Ok(surface)
    }
    fn hide(&mut self, runtime: &mut SurfaceRuntime) -> Result<(), UiBackendError> {
        if self.visible {
            if let Some(surface) = self.surface {
                runtime.hide(surface)?;
            }
            self.visible = false;
        }
        if !self.sequences.active() {
            self.requested = None;
            self.painted = None;
            self.last_poll = None;
            self.last_pointer = None;
            self.last_motion = None;
            // The existing icon resolver runs on a dedicated worker. Dropping
            // it when the last launch ends closes the queue and terminates
            // that thread instead of retaining a needless idle worker.
            self.loader = None;
        }
        Ok(())
    }
}

/// Convert RGBA8 icon alpha into per-row runs for `XShape`'s 1-bit bounding region.
/// A small alpha cutoff preserves antialiased edges without expanding transparent padding.
fn alpha_shape_spans(pixels: &[u8], width: u32, height: u32) -> Option<Vec<(u32, u32, u32)>> {
    let len = (width as usize)
        .checked_mul(height as usize)?
        .checked_mul(4)?;
    if width == 0 || height == 0 || width > 64 || height > 64 || pixels.len() != len {
        return None;
    }
    let mut spans = Vec::new();
    for y in 0..height {
        let mut start = None;
        for x in 0..=width {
            let covered = x < width && pixels[((y * width + x) * 4 + 3) as usize] >= 8;
            if covered {
                if start.is_none() {
                    start = Some(x);
                }
            } else if let Some(left) = start.take() {
                spans.push((y, left, x));
            }
        }
    }
    (!spans.is_empty()).then_some(spans)
}

fn updated_launch_key(
    id: &str,
    icon: &str,
    side: u32,
    existing: Option<&(String, String, u32)>,
) -> Option<(String, String, u32)> {
    let icon = if icon.trim().is_empty() {
        "application-x-executable"
    } else {
        icon
    };
    if existing.is_some_and(|(old_id, old_icon, old_side)| {
        old_id.as_str() == id && old_icon.as_str() == icon && *old_side == side
    }) {
        None
    } else {
        Some((id.to_owned(), icon.to_owned(), side))
    }
}

fn matches_icon_result(
    target: IconTarget,
    name: &str,
    width: u32,
    height: u32,
    requested: &str,
    side: u32,
) -> bool {
    target == IconTarget::TaskSlot(0) && name == requested && width == side && height == side
}

fn output_at(displays: &DisplaySnapshot, x: i32, y: i32) -> Option<&OutputSnapshot> {
    displays
        .outputs
        .iter()
        .filter(|o| o.connected)
        .find(|o| {
            let r = o.geometry;
            x >= r.x
                && y >= r.y
                && x < r.x.saturating_add(r.width)
                && y < r.y.saturating_add(r.height)
        })
        .or_else(|| displays.outputs.iter().find(|o| o.primary && o.connected))
}

fn badge_position(
    px: i32,
    py: i32,
    side: i32,
    scale: u32,
    output: Option<&OutputSnapshot>,
) -> (i32, i32) {
    let offset = ((i64::from(BASE_OFFSET) * i64::from(scale) + 50) / 100) as i32;
    let (mut x, mut y) = (px.saturating_add(offset), py.saturating_add(offset));
    if let Some(out) = output {
        let r = out.geometry;
        let right = r.x.saturating_add(r.width);
        let bottom = r.y.saturating_add(r.height);
        // Flip rather than clamp across the pointer hot spot at screen edges.
        if x.saturating_add(side) > right {
            x = px.saturating_sub(offset).saturating_sub(side);
        }
        if y.saturating_add(side) > bottom {
            y = py.saturating_sub(offset).saturating_sub(side);
        }
        x = x.clamp(r.x, right.saturating_sub(side).max(r.x));
        y = y.clamp(r.y, bottom.saturating_sub(side).max(r.y));
    }
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamewm_api::{ModeId, OutputId, Rect};
    fn snapshot() -> DisplaySnapshot {
        DisplaySnapshot {
            generation: 0,
            pending: None,
            outputs: vec![OutputSnapshot {
                id: OutputId::new("left"),
                connector: "DP-1".into(),
                edid_identity: String::new(),
                connected: true,
                primary: true,
                geometry: Rect::new(-1920, -100, 1920, 1080),
                current_mode: ModeId(1),
                modes: vec![],
                shell_scale_percent: 150,
            }],
        }
    }
    #[test]
    fn same_app_overlapping_launch_uses_inflight_icon_result() {
        // A first launch's result is reusable by a second launch with the
        // same icon. The loader does not know startup IDs and coalesces keys.
        assert!(matches_icon_result(
            IconTarget::TaskSlot(0),
            "org.kde.dolphin",
            24,
            24,
            "org.kde.dolphin",
            24
        ));
        assert!(!matches_icon_result(
            IconTarget::TaskSlot(0),
            "org.kde.dolphin",
            24,
            24,
            "org.kde.kate",
            24
        ));
        assert!(!matches_icon_result(
            IconTarget::TaskSlot(0),
            "org.kde.dolphin",
            16,
            16,
            "org.kde.dolphin",
            24
        ));
    }

    #[test]
    fn stable_launch_key_does_not_allocate_another_owned_key() {
        let current = ("launch-1".to_owned(), "org.kde.kate".to_owned(), 24);
        assert_eq!(
            updated_launch_key("launch-1", "org.kde.kate", 24, Some(&current)),
            None
        );
        assert_eq!(
            updated_launch_key("launch-2", "org.kde.kate", 24, Some(&current)),
            Some(("launch-2".to_owned(), "org.kde.kate".to_owned(), 24))
        );
        assert_eq!(
            updated_launch_key("launch-1", "org.kde.kate", 32, Some(&current)),
            Some(("launch-1".to_owned(), "org.kde.kate".to_owned(), 32))
        );
        assert_eq!(
            updated_launch_key("launch-1", "  ", 24, None),
            Some((
                "launch-1".to_owned(),
                "application-x-executable".to_owned(),
                24
            ))
        );
    }

    #[test]
    fn alpha_shape_excludes_transparent_padding_and_holes() {
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        for (x, y) in [(1, 1), (2, 1), (2, 2)] {
            rgba[(y * 4 + x) * 4 + 3] = 255;
        }
        assert_eq!(
            alpha_shape_spans(&rgba, 4, 4),
            Some(vec![(1, 1, 3), (2, 2, 3)])
        );
        assert_eq!(alpha_shape_spans(&[], 4, 4), None);
        assert_eq!(alpha_shape_spans(&[0; 16 * 4], 4, 4), None);
    }

    #[test]
    fn placement_handles_negative_origin_and_output_corner() {
        let snap = snapshot();
        let output = output_at(&snap, -1910, -90).expect("left output");
        assert_eq!(
            badge_position(-1910, -90, 24, 150, Some(output)),
            (-1890, -70)
        );
        assert_eq!(badge_position(-5, 970, 24, 150, Some(output)), (-49, 926));
    }

    #[test]
    fn pointer_cadence_is_fast_only_while_moving() {
        assert_eq!(poll_interval(Duration::ZERO), POLL_MOVING);
        assert_eq!(poll_interval(MOTION_WINDOW - Duration::from_millis(1)), POLL_MOVING);
        assert_eq!(poll_interval(MOTION_WINDOW), POLL_STILL);
        assert_eq!(poll_interval(Duration::from_secs(14)), POLL_STILL);
    }
}
