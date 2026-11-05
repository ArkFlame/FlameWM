//! Desktop-layer capture: the blurred backdrop that translucent chrome blends over.

use super::*;

/// and a rubber-band drag commits the desktop surface on every motion; the trailing wake
/// ([`schedule_desktop_wake`]) recaptures the final state once the burst settles.
pub(super) const DESKTOP_RECAPTURE_INTERVAL: Duration = Duration::from_millis(100);

/// The blurred desktop is kept at 1/`DESKTOP_SCALE` linear size. It only shows as a soft tint under
/// chrome, so the quarter-size copy is visually equivalent at 1/16 of the resident bytes.
pub(super) const DESKTOP_SCALE: usize = 4;
/// Output rows read back and box-averaged per step, so the full-output readback is never resident.
/// Must be a multiple of `DESKTOP_SCALE` so each strip covers whole stored rows.
pub(super) const DESKTOP_STRIP_ROWS: usize = 32;

/// Stored (downscaled) size of a `size` output capture.
pub(super) fn stored_size(size: (usize, usize)) -> (usize, usize) {
    (size.0.div_ceil(DESKTOP_SCALE).max(1), size.1.div_ceil(DESKTOP_SCALE).max(1))
}

/// Whether `FLAMEWM_BACKDROP_PROFILE` is set. Read once; every profiling path checks it first.
fn profile_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FLAMEWM_BACKDROP_PROFILE").is_some())
}

/// Per-phase wall time of one recapture, printed as one stderr line when profiling is enabled.
/// Disabled, it holds no clock and no buckets, so each `split` is a single branch.
pub(super) struct PhaseClock {
    last: Option<Instant>,
    start: Option<Instant>,
    phases: Vec<(&'static str, Duration)>,
}

impl PhaseClock {
    pub(super) fn start() -> Self {
        let now = profile_enabled().then(Instant::now);
        Self { last: now, start: now, phases: Vec::new() }
    }

    /// Adds the time since the previous split to the `name` bucket. Repeated names accumulate.
    pub(super) fn split(&mut self, name: &'static str) {
        let Some(last) = self.last else { return };
        let now = Instant::now();
        let elapsed = now - last;
        self.last = Some(now);
        match self.phases.iter_mut().find(|(phase, _)| *phase == name) {
            Some((_, total)) => *total += elapsed,
            None => self.phases.push((name, elapsed)),
        }
    }

    /// Emits the breakdown and restarts the clock, so time spent outside the split calls is not hidden.
    pub(super) fn report(&self, rows: usize) {
        let Some(start) = self.start else { return };
        let mut line = format!("BACKDROP_PHASE total_us={} rows={rows}", start.elapsed().as_micros());
        for (name, elapsed) in &self.phases {
            line.push_str(&format!(" {name}_us={}", elapsed.as_micros()));
        }
        eprintln!("{line}");
    }
}

pub(super) struct DesktopBackdrop {
    /// Output size the capture stands for; the pixels are stored at [`stored_size`] of it.
    pub(super) width: usize,
    pub(super) height: usize,
    /// Commit counter of every desktop-layer element the capture saw. A wallpaper repaint is a
    /// new commit on its surface, so it changes this and triggers a recapture.
    pub(super) commits: Vec<CommitCounter>,
    pub(super) pixels: Vec<u8>,
    pub(super) captured_at: Instant,
}

/// Bilinear sample of straight RGBA at stored-pixel coordinates (`fx`, `fy`), clamped to the edges.
pub(super) fn sample_bilinear(pixels: &[u8], (sw, sh): (usize, usize), fx: f32, fy: f32) -> [u8; 4] {
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let at = |x: usize, y: usize, c: usize| f32::from(pixels[(y * sw + x) * 4 + c]);
    let mut out = [0u8; 4];
    for (c, value) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) + (at(x1, y0, c) - at(x0, y0, c)) * tx;
        let bottom = at(x0, y1, c) + (at(x1, y1, c) - at(x0, y1, c)) * tx;
        *value = (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Box-averages `rows` full-width readback rows starting at output row `y0` into the stored image.
/// `y0` and the strip height are multiples of `DESKTOP_SCALE` except at the output bottom.
pub(super) fn downscale_strip(strip: &[u8], (width, y0, rows): (usize, usize, usize), stored: &mut [u8], (sw, sh): (usize, usize)) {
    let end = y0 + rows;
    for sy in (y0 / DESKTOP_SCALE)..end.div_ceil(DESKTOP_SCALE).min(sh) {
        for sx in 0..sw {
            let mut sum = [0u32; 4];
            let mut count = 0u32;
            for y in (sy * DESKTOP_SCALE).max(y0)..((sy + 1) * DESKTOP_SCALE).min(end) {
                for x in (sx * DESKTOP_SCALE)..((sx + 1) * DESKTOP_SCALE).min(width) {
                    let at = ((y - y0) * width + x) * 4;
                    for (total, byte) in sum.iter_mut().zip(&strip[at..at + 4]) {
                        *total += u32::from(*byte);
                    }
                    count += 1;
                }
            }
            let at = (sy * sw + sx) * 4;
            for (c, total) in sum.iter().enumerate() {
                stored[at + c] = ((total + count / 2) / count.max(1)) as u8;
            }
        }
    }
}

pub(super) fn desktop_elements(state: &State, renderer: &mut GlowRenderer) -> Vec<Elem> {
    use crate::scene::SceneItem;

    crate::scene::front_to_back(state)
        .into_iter()
        .filter_map(|item| match item {
            SceneItem::Layer { surface, .. }
                if matches!(surface.layer(), Layer::Bottom | Layer::Background) =>
            {
                Some(layer_elements(state, renderer, &surface))
            }
            SceneItem::Unmanaged(index)
                if state.unmanaged.get(index).is_some_and(|entry| entry.below) =>
            {
                let entry = &state.unmanaged[index];
                entry.surface.wl_surface().map(|surface| {
                    let geometry = entry.surface.geometry();
                    surface_elements(renderer, &surface, geometry.loc.x, geometry.loc.y)
                })
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// Arms one calloop timer that requests a redraw when the recapture throttle reopens. Idempotent
/// while armed, so a burst of throttled frames costs one timer.
pub(super) fn schedule_desktop_wake(state: &State, wait: Duration) {
    {
        let mut decorations = state.decorations.borrow_mut();
        if decorations.desktop_wake_armed {
            return;
        }
        decorations.desktop_wake_armed = true;
    }
    let armed = state.loop_handle.insert_source(
        smithay::reexports::calloop::timer::Timer::from_duration(wait),
        |_, (), state| {
            state.decorations.borrow_mut().desktop_wake_armed = false;
            state.request_redraw();
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        },
    );
    if armed.is_err() {
        state.decorations.borrow_mut().desktop_wake_armed = false;
    }
}

/// Captures and blurs the desktop layer when the cached capture is missing, the output changed
/// size, or any desktop-layer element committed (a wallpaper repaint, a surface added or removed).
/// Steady-state frames read nothing back; they only compare commit counters. Called by both the
/// winit and DRM render paths before they build the frame.
pub(crate) fn refresh_desktop_backdrop(
    state: &State,
    renderer: &mut GlowRenderer,
    size: Size<i32, Physical>,
) -> Result<(), Box<dyn Error>> {
    // The blur only shows under window chrome and override-redirect surfaces. Desktop-kind X11
    // windows are `below` entries that never read it, so an idle desktop (or one with only its own
    // menus open) must not recapture on every desktop commit: drop the capture instead.
    let consumer = !state.toplevels.is_empty()
        || !state.x11.is_empty()
        || state.unmanaged.iter().any(|entry| !entry.below);
    if !consumer {
        state.decorations.borrow_mut().drop_desktop();
        return Ok(());
    }
    let mut clock = PhaseClock::start();
    let dims = (size.w.max(1) as usize, size.h.max(1) as usize);
    let elements = desktop_elements(state, renderer);
    let commits: Vec<CommitCounter> = elements.iter().map(Element::current_commit).collect();
    clock.split("elements");
    if state.decorations.borrow().desktop_current(dims, &commits) {
        return Ok(());
    }
    // Throttled: keep the stale capture and wake once the window reopens, so the final state lands.
    let wait = state
        .decorations
        .borrow()
        .recapture_wait(dims, Instant::now());
    if let Some(wait) = wait {
        schedule_desktop_wake(state, wait);
        return Ok(());
    }
    let buffer_size: Size<i32, Buffer> = Size::from((size.w, size.h));
    let mut texture: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?;
    clock.split("texture");
    let mut tracker = OutputDamageTracker::from_output(&state.output);
    {
        let mut target = renderer.bind(&mut texture)?;
        tracker.render_output(renderer, &mut target, 0, &elements, CLEAR)?;
    }
    clock.split("render");
    // Read back and box-average one strip at a time: only the reduced image is ever resident.
    let (sw, sh) = stored_size(dims);
    let mut stored = vec![0u8; sw * sh * 4];
    let target = renderer.bind(&mut texture)?;
    let mut y0 = 0;
    while y0 < dims.1 {
        let rows = DESKTOP_STRIP_ROWS.min(dims.1 - y0);
        let region = Rectangle::<i32, Buffer>::new(
            Point::from((0, y0 as i32)),
            Size::from((dims.0 as i32, rows as i32)),
        );
        let mapping = renderer.copy_framebuffer(&target, region, Fourcc::Abgr8888)?;
        clock.split("copy");
        let strip = renderer.map_texture(&mapping)?;
        clock.split("map");
        downscale_strip(strip, (dims.0, y0, rows), &mut stored, (sw, sh));
        clock.split("downscale");
        y0 += rows;
    }
    let blur_percent = flamewm_skin::recipes::decoration::DEFAULT_SURFACE_BLUR_PERCENT;
    let radius = MAX_BLUR_RADIUS_PX * f32::from(blur_percent) / 100.0 / DESKTOP_SCALE as f32;
    let source = RgbaImage {
        width: sw as u32,
        height: sh as u32,
        pixels: stored,
    };
    let blurred = frosted_backdrop(
        &source,
        ImageRect {
            x: 0,
            y: 0,
            width: sw as u32,
            height: sh as u32,
        },
        radius,
        None,
    );
    clock.split("blur");
    state
        .decorations
        .borrow_mut()
        .set_desktop(dims, commits, blurred.pixels);
    clock.split("store");
    clock.report(dims.1);
    Ok(())
}
