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
    (
        size.0.div_ceil(DESKTOP_SCALE).max(1),
        size.1.div_ceil(DESKTOP_SCALE).max(1),
    )
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
        Self {
            last: now,
            start: now,
            phases: Vec::new(),
        }
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
        let mut line = format!(
            "BACKDROP_PHASE total_us={} rows={rows}",
            start.elapsed().as_micros()
        );
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

/// Samples a reduced capture of a `src` physical-pixel region into a `w` x `h` logical buffer. The
/// logical `origin` and size map to physical pixels at `scale`, clamped to the region's edges.
/// Bilinear in stored-pixel space, clamped to the edges. The column indices and weights depend only
/// on `x`, so they are computed once instead of once per row, and each row's two source rows and
/// weight are hoisted out of the pixel loop, and the vertical blend is done once per row, so a pixel
/// costs one horizontal lerp per channel. Fixed-point (10-bit weights). Matches the per-pixel reference to within 1 per byte
/// (float rounding order).
pub(super) fn crop_capture(
    pixels: &[u8],
    src: (usize, usize),
    origin: (i32, i32),
    w: usize,
    h: usize,
    scale: f64,
) -> Vec<u8> {
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let (sw, sh) = stored_size(src);
    let stored_scale = DESKTOP_SCALE as f32;
    let scale = scale as f32;
    // Per column: byte offsets of the two source pixels and the horizontal weight.
    let columns: Vec<(usize, usize, u32)> = (0..w)
        .map(|x| {
            let ox = ((origin.0 + x as i32) as f32 * scale).clamp(0.0, (src.0 as i32 - 1) as f32);
            let fx = ((ox + 0.5) / stored_scale - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx as usize;
            (
                x0 * 4,
                (x0 + 1).min(sw - 1) * 4,
                ((fx - x0 as f32) * 1024.0 + 0.5) as u32,
            )
        })
        .collect();
    let mut out = vec![0_u8; w * h * 4];
    // The two source rows of an output row blended vertically, one value per stored sample.
    let mut blended = vec![0_u32; sw * 4];
    for (y, out_row) in out.chunks_exact_mut(w * 4).enumerate() {
        // Output pixel centre mapped into stored space, clamped to the output edges.
        let oy = ((origin.1 + y as i32) as f32 * scale).clamp(0.0, (src.1 as i32 - 1) as f32);
        let fy = ((oy + 0.5) / stored_scale - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = ((fy - y0 as f32) * 1024.0 + 0.5) as u32;
        let top_row = &pixels[y0 * sw * 4..(y0 + 1) * sw * 4];
        let bottom_row = &pixels[y1 * sw * 4..(y1 + 1) * sw * 4];
        // 10-bit fixed point: a blended sample is at most 255 << 10, so the horizontal blend
        // (<= 255 << 20) fits u32 and one shift rounds it.
        for ((value, &top), &bottom) in blended.iter_mut().zip(top_row).zip(bottom_row) {
            *value = u32::from(top) * (1024 - ty) + u32::from(bottom) * ty;
        }
        let (pixels4, _) = out_row.as_chunks_mut::<4>();
        for (pixel, &(i0, i1, tx)) in pixels4.iter_mut().zip(&columns) {
            for (c, value) in pixel.iter_mut().enumerate() {
                let sample = blended[i0 + c] * (1024 - tx) + blended[i1 + c] * tx;
                *value = ((sample + (1 << 19)) >> 20) as u8;
            }
        }
    }
    out
}

/// The original per-pixel formulation, kept as the reference `crop_capture` must match exactly.
#[cfg(test)]
fn crop_capture_reference(
    pixels: &[u8],
    src: (usize, usize),
    origin: (i32, i32),
    w: usize,
    h: usize,
    scale: f64,
) -> Vec<u8> {
    let (sw, sh) = stored_size(src);
    let stored_scale = DESKTOP_SCALE as f32;
    let scale = scale as f32;
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let oy = ((origin.1 + y as i32) as f32 * scale).clamp(0.0, (src.1 as i32 - 1) as f32);
        let fy = ((oy + 0.5) / stored_scale - 0.5).clamp(0.0, (sh - 1) as f32);
        for x in 0..w {
            let ox = ((origin.0 + x as i32) as f32 * scale).clamp(0.0, (src.0 as i32 - 1) as f32);
            let fx = ((ox + 0.5) / stored_scale - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let at = |x: usize, y: usize, c: usize| f32::from(pixels[(y * sw + x) * 4 + c]);
            for c in 0..4 {
                let top = at(x0, y0, c) + (at(x1, y0, c) - at(x0, y0, c)) * tx;
                let bottom = at(x0, y1, c) + (at(x1, y1, c) - at(x0, y1, c)) * tx;
                out.push((top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod crop_tests {
    use super::*;

    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_u32;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn crop_capture_matches_the_per_pixel_reference() {
        for (src, origin, w, h, scale) in [
            ((203, 117), (0, 0), 203, 117, 1.0),
            ((203, 117), (-5, -3), 190, 110, 1.0),
            ((200, 120), (7, 9), 61, 44, 1.5),
            ((64, 64), (0, 0), 1, 1, 1.0),
            ((1, 1), (0, 0), 7, 5, 2.0),
            ((257, 31), (250, 20), 40, 30, 1.25),
        ] {
            let (sw, sh) = stored_size(src);
            let pixels = noise(sw * sh * 4);
            let fast = crop_capture(&pixels, src, origin, w, h, scale);
            let slow = crop_capture_reference(&pixels, src, origin, w, h, scale);
            assert_eq!(fast.len(), slow.len(), "{src:?} {w}x{h}");
            let worst = fast
                .iter()
                .zip(&slow)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            assert!(
                worst <= 1,
                "src {src:?} origin {origin:?} {w}x{h} scale {scale}: off by {worst}"
            );
        }
    }

    #[test]
    fn empty_output_is_empty() {
        let pixels = noise(4 * 4 * 4);
        assert_eq!(
            crop_capture(&pixels, (16, 16), (0, 0), 0, 5, 1.0),
            Vec::<u8>::new()
        );
        assert_eq!(
            crop_capture(&pixels, (16, 16), (0, 0), 5, 0, 1.0),
            Vec::<u8>::new()
        );
    }
}

/// Reads the `dims` region of `texture` back in [`DESKTOP_STRIP_ROWS`] strips and box-averages each
/// into the reduced image, so only one strip of full-size pixels is ever resident.
pub(super) fn read_reduced(
    renderer: &mut GlowRenderer,
    texture: &mut GlesTexture,
    dims: (usize, usize),
    clock: &mut PhaseClock,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let (sw, sh) = stored_size(dims);
    let mut stored = vec![0u8; sw * sh * 4];
    let target = renderer.bind(texture)?;
    let mut y0 = 0;
    while y0 < dims.1 {
        let rows = DESKTOP_STRIP_ROWS.min(dims.1 - y0);
        let region = Rectangle::<i32, Buffer>::new(
            Point::from((0, y0 as i32)),
            Size::from((dims.0 as i32, rows as i32)),
        );
        // The readback is requested as `Argb8888` (BGRA_EXT). The `Abgr8888` request returned
        // blue-first bytes on this stack (desktop top-left read as [0,0,255] for a red wallpaper),
        // which tinted every translucent titlebar with R and B swapped. `downscale_strip` and the
        // RGBA consumers below expect straight RGBA, so keep this format in step with them.
        let mapping = renderer.copy_framebuffer(&target, region, Fourcc::Argb8888)?;
        clock.split("copy");
        let strip = renderer.map_texture(&mapping)?;
        clock.split("map");
        downscale_strip(strip, (dims.0, y0, rows), &mut stored, (sw, sh));
        clock.split("downscale");
        y0 += rows;
    }
    drop(target);
    Ok(stored)
}

thread_local! {
    /// Float work buffers reused by every capture blur on the render thread, so a burst of captures
    /// (a window drag) does not reallocate them each time.
    static BLUR_SCRATCH: std::cell::RefCell<BlurScratch> = std::cell::RefCell::new(BlurScratch::new());
}

/// Blurs a reduced capture by `blur_percent` of the surface blur radius. 100% is
/// [`MAX_BLUR_RADIUS_PX`] at 1x, scaled to the reduced image.
pub(super) fn blur_reduced(stored: Vec<u8>, (sw, sh): (usize, usize), blur_percent: u8) -> Vec<u8> {
    let radius = MAX_BLUR_RADIUS_PX * f32::from(blur_percent) / 100.0 / DESKTOP_SCALE as f32;
    let source = RgbaImage {
        width: sw as u32,
        height: sh as u32,
        pixels: stored,
    };
    BLUR_SCRATCH
        .with_borrow_mut(|scratch| {
            frosted_backdrop_with(
                scratch,
                &source,
                ImageRect {
                    x: 0,
                    y: 0,
                    width: sw as u32,
                    height: sh as u32,
                },
                radius,
                None,
            )
        })
        .pixels
}

/// Box-averages `rows` full-width readback rows starting at output row `y0` into the stored image.
/// `y0` and the strip height are multiples of `DESKTOP_SCALE` except at the output bottom.
pub(super) fn downscale_strip(
    strip: &[u8],
    (width, y0, rows): (usize, usize, usize),
    stored: &mut [u8],
    (sw, sh): (usize, usize),
) {
    let end = y0 + rows;
    // One accumulator row per stored row: each source row is read once, front to back, and the
    // division happens once per stored pixel instead of once per channel of a nested block walk.
    let mut sums = vec![0u32; sw * 4];
    for sy in (y0 / DESKTOP_SCALE)..end.div_ceil(DESKTOP_SCALE).min(sh) {
        sums.fill(0);
        let source_rows = (sy * DESKTOP_SCALE).max(y0)..((sy + 1) * DESKTOP_SCALE).min(end);
        let row_count = source_rows.len() as u32;
        for y in source_rows {
            let row = &strip[(y - y0) * width * 4..(y - y0 + 1) * width * 4];
            let blocks = row.as_chunks::<4>().0.chunks(DESKTOP_SCALE);
            for (pixels, sum) in blocks.zip(sums.as_chunks_mut::<4>().0) {
                for pixel in pixels {
                    for (total, byte) in sum.iter_mut().zip(pixel) {
                        *total += u32::from(*byte);
                    }
                }
            }
        }
        for (sx, sum) in sums.as_chunks::<4>().0.iter().enumerate() {
            let columns = width.saturating_sub(sx * DESKTOP_SCALE).min(DESKTOP_SCALE) as u32;
            let count = (row_count * columns).max(1);
            let at = (sy * sw + sx) * 4;
            for (out, total) in stored[at..at + 4].iter_mut().zip(sum) {
                *out = ((total + count / 2) / count) as u8;
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
                    surface_elements(
                        renderer,
                        &surface,
                        geometry.loc.x,
                        geometry.loc.y,
                        state.output_scale(),
                    )
                })
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// Takes the cached capture texture when it matches `dims`. A mismatched slot is cleared, so a
/// resize frees the old texture here instead of keeping it alongside the new one.
pub(super) fn take_scratch<T>(
    slot: &mut Option<((usize, usize), T)>,
    dims: (usize, usize),
) -> Option<T> {
    match slot.take() {
        Some((size, texture)) if size == dims => Some(texture),
        _ => None,
    }
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
    // Flat (`reduceTransparency`, the default) no material is translucent: nothing reads the blur,
    // so the capture, its readback and the blur are skipped entirely.
    if !consumer || glass_flat() {
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
    let reused = take_scratch(&mut state.decorations.borrow_mut().scratch, dims);
    let mut texture: GlesTexture = match reused {
        Some(texture) => texture,
        None => renderer.create_buffer(Fourcc::Abgr8888, buffer_size)?,
    };
    clock.split("texture");
    let mut tracker = OutputDamageTracker::from_output(&state.output);
    {
        let mut target = renderer.bind(&mut texture)?;
        tracker.render_output(renderer, &mut target, 0, &elements, CLEAR)?;
    }
    clock.split("render");
    let stored = read_reduced(renderer, &mut texture, dims, &mut clock)?;
    state.decorations.borrow_mut().scratch = Some((dims, texture));
    let blurred = blur_reduced(
        stored,
        stored_size(dims),
        DESKTOP_BLUR_PERCENT.load(std::sync::atomic::Ordering::Relaxed),
    );
    clock.split("blur");
    state
        .decorations
        .borrow_mut()
        .set_desktop(dims, commits, blurred);
    clock.split("store");
    clock.report(dims.1);
    Ok(())
}

#[cfg(test)]
mod scratch_tests {
    use super::take_scratch;

    #[test]
    fn scratch_is_reused_only_for_the_same_output_size() {
        let mut slot = Some(((4, 3), 7u32));
        assert_eq!(take_scratch(&mut slot, (4, 3)), Some(7));
        assert!(
            slot.is_none(),
            "a taken texture leaves the slot empty until it is put back"
        );

        let mut slot = Some(((4, 3), 7u32));
        assert_eq!(take_scratch(&mut slot, (8, 3)), None);
        assert!(
            slot.is_none(),
            "a resize drops the old texture instead of keeping it"
        );

        let mut slot: Option<((usize, usize), u32)> = None;
        assert_eq!(take_scratch(&mut slot, (4, 3)), None);
    }
}
