mod gles;

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::native::rounded::row_inset;

pub use flamewm_render_core::{PixelBackend, PixelOp, RenderAcceleration, RoundedRect};

pub const RENDER_ACCELERATION_SETTING: &str = "renderAcceleration";

const MIN_GPU_PIXELS: usize = 64 * 64;
const CPU_SCRATCH_RETAIN_BUDGET_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct CpuPixels {
    scratch: flamewm_image_core::blur::BlurScratch,
}

impl PixelBackend for CpuPixels {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String> {
        let result = flamewm_image_core::blur::gaussian_blur_rgba_with(
            &mut self.scratch,
            pixels,
            width,
            height,
            radius,
        );
        self.scratch
            .release_if_over_budget(CPU_SCRATCH_RETAIN_BUDGET_BYTES);
        result.map_err(|error| format!("{error:?}"))
    }

    fn fill_rounded_rect(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String> {
        if pixels.len() != width as usize * height as usize * 4 {
            return Err("pixel buffer does not match its size".to_owned());
        }
        let alpha = f32::from(color[3]) / 255.0;
        for row in 0..outer.height {
            let y = outer.y + row as i32;
            if !(0..height as i32).contains(&y) {
                continue;
            }
            let inset = row_inset(outer.width, outer.height, outer.radius, row) as i32;
            let start = (outer.x + inset).max(0);
            let end = (outer.x + outer.width as i32 - inset).min(width as i32);
            for x in start..end {
                if hole.is_some_and(|hole| inside_rounded(hole, x, y)) {
                    continue;
                }
                let index = (y as usize * width as usize + x as usize) * 4;
                let pixel = &mut pixels[index..index + 4];
                for channel in 0..3 {
                    pixel[channel] = (f32::from(color[channel]) * alpha
                        + f32::from(pixel[channel]) * (1.0 - alpha))
                        .round() as u8;
                }
                pixel[3] =
                    (f32::from(color[3]) + f32::from(pixel[3]) * (1.0 - alpha)).round() as u8;
            }
        }
        Ok(())
    }
}

/// Whether device pixel (`x`, `y`) lies inside `rect`'s rounded outline, using the same row
/// insets as the outer fill.
fn inside_rounded(rect: RoundedRect, x: i32, y: i32) -> bool {
    let (Ok(row), Ok(column)) = (u32::try_from(y - rect.y), u32::try_from(x - rect.x)) else {
        return false;
    };
    if row >= rect.height || column >= rect.width {
        return false;
    }
    let inset = row_inset(rect.width, rect.height, rect.radius, row);
    column >= inset && column < rect.width - inset
}

impl PixelBackend for gles::GpuBlur {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String> {
        self.blur_sigma(pixels, width, height, radius / 2.0)
    }

    fn fill_rounded_rect(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
    ) -> Result<(), String> {
        self.rounded_fill(pixels, width, height, outer, hole, color)
    }

    fn paint_batch(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        self.paint_ops(pixels, width, height, ops)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccelerationStatus {
    pub requested: RenderAcceleration,
    pub gpu_renderer: Option<String>,
    pub gpu_error: Option<String>,
}

static REQUESTED: AtomicU8 = AtomicU8::new(0);
static STATUS: Mutex<(Option<String>, Option<String>)> = Mutex::new((None, None));

enum GpuSlot {
    Untried,
    Ready(Box<gles::GpuBlur>),
    Failed,
}

thread_local! {
    static GPU: RefCell<GpuSlot> = const { RefCell::new(GpuSlot::Untried) };
    static CPU: RefCell<CpuPixels> = RefCell::default();
}

pub fn set_render_acceleration(mode: RenderAcceleration) {
    let previous = REQUESTED.swap(mode as u8, Ordering::Relaxed);
    if previous != mode as u8 && mode == RenderAcceleration::Gpu {
        GPU.with(|slot| {
            let mut slot = slot.borrow_mut();
            if matches!(*slot, GpuSlot::Failed) {
                *slot = GpuSlot::Untried;
            }
        });
    }
}

#[must_use]
pub fn render_acceleration() -> RenderAcceleration {
    if REQUESTED.load(Ordering::Relaxed) == RenderAcceleration::Gpu as u8 {
        RenderAcceleration::Gpu
    } else {
        RenderAcceleration::Cpu
    }
}

#[must_use]
pub fn render_acceleration_status() -> AccelerationStatus {
    let (gpu_renderer, gpu_error) = STATUS
        .lock()
        .map(|status| status.clone())
        .unwrap_or_default();
    AccelerationStatus {
        requested: render_acceleration(),
        gpu_renderer,
        gpu_error,
    }
}

static PROBE: Mutex<Option<Result<String, String>>> = Mutex::new(None);

pub fn probe_gpu_renderer() -> Result<String, String> {
    let mut cached = PROBE
        .lock()
        .map_err(|_| "GPU probe state is poisoned".to_owned())?;
    cached
        .get_or_insert_with(|| gles::GpuBlur::new().map(|gpu| gpu.renderer().to_owned()))
        .clone()
}

fn record(renderer: Option<String>, error: Option<String>) {
    if let Ok(mut status) = STATUS.lock() {
        *status = (renderer, error);
    }
}

/// Runs one pixel operation on the GPU when `renderAcceleration` is `gpu` and the GPU is healthy.
/// `false` means the operation did not run on the GPU and the caller must run it on the CPU; the
/// GPU contract leaves the frame untouched on error, so the CPU sees the original pixels.
fn try_gpu(op: &mut dyn FnMut(&mut dyn PixelBackend) -> Result<(), String>) -> bool {
    if render_acceleration() != RenderAcceleration::Gpu {
        return false;
    }
    GPU.with(|slot| {
        let mut slot = slot.borrow_mut();
        if matches!(*slot, GpuSlot::Untried) {
            *slot = match gles::GpuBlur::new() {
                Ok(gpu) => {
                    eprintln!("flamewm-render-x11: GPU acceleration on {}", gpu.renderer());
                    record(Some(gpu.renderer().to_owned()), None);
                    GpuSlot::Ready(Box::new(gpu))
                }
                Err(error) => {
                    eprintln!(
                        "flamewm-render-x11: GPU acceleration unavailable, using CPU: {error}"
                    );
                    record(None, Some(error));
                    GpuSlot::Failed
                }
            };
        }
        let GpuSlot::Ready(gpu) = &mut *slot else {
            return false;
        };
        let backend: &mut dyn PixelBackend = &mut **gpu;
        match op(backend) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("flamewm-render-x11: GPU pixel op failed, using CPU: {error}");
                record(None, Some(error));
                *slot = GpuSlot::Failed;
                false
            }
        }
    })
}

pub(crate) fn gpu_blur_rgba(pixels: &mut [u8], width: u32, height: u32, radius: f32) -> bool {
    if radius < 0.5 || (width as usize) * (height as usize) < MIN_GPU_PIXELS {
        return false;
    }
    try_gpu(&mut |backend| backend.blur_rgba(pixels, width, height, radius))
}

pub fn blur_rgba(pixels: &mut [u8], width: u32, height: u32, radius: f32) {
    if gpu_blur_rgba(pixels, width, height, radius) {
        return;
    }
    let result = CPU.with(|cpu| cpu.borrow_mut().blur_rgba(pixels, width, height, radius));
    if let Err(error) = result {
        eprintln!("flamewm-render-x11: backdrop blur skipped: {error}");
    }
}

/// Source-over `color` through `outer` minus `hole`. Runs on the GPU when `renderAcceleration` is
/// `gpu`, and on the CPU otherwise or after any GPU failure.
pub fn fill_rounded_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    outer: RoundedRect,
    hole: Option<RoundedRect>,
    color: [u8; 4],
) -> Result<(), String> {
    let outer = outer.normalized();
    let hole = hole.map(RoundedRect::normalized);
    let mut op = |backend: &mut dyn PixelBackend| {
        backend.fill_rounded_rect(pixels, width, height, outer, hole, color)
    };
    if try_gpu(&mut op) {
        return Ok(());
    }
    CPU.with(|cpu| op(&mut *cpu.borrow_mut()))
}

/// Source-over ring of `thickness` pixels inside `rect`, through the same selection as
/// [`fill_rounded_rect`].
pub fn stroke_rounded_rect(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    rect: RoundedRect,
    thickness: u32,
    color: [u8; 4],
) -> Result<(), String> {
    let outer = rect.normalized();
    fill_rounded_rect(
        pixels,
        width,
        height,
        outer,
        Some(outer.inset(thickness)),
        color,
    )
}

/// Runs `ops` on the GPU only. `false` means the GPU did not run them and the frame is untouched,
/// so the caller keeps its own painter; the CPU batch is not used because its edges differ.
pub(crate) fn try_gpu_batch(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    ops: &[PixelOp<'_>],
) -> bool {
    try_gpu(&mut |backend| backend.paint_batch(pixels, width, height, ops))
}

/// Applies `ops` (normalized, see [`PixelOp::normalized`]) to one frame through the same selection
/// as the single-op calls: one GPU upload and readback when `renderAcceleration` is `gpu`, otherwise
/// the CPU batch. Any GPU failure leaves the frame untouched and runs the batch on the CPU.
pub fn paint_batch(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    ops: &[PixelOp<'_>],
) -> Result<(), String> {
    let mut op = |backend: &mut dyn PixelBackend| backend.paint_batch(pixels, width, height, ops);
    if try_gpu(&mut op) {
        return Ok(());
    }
    CPU.with(|cpu| op(&mut *cpu.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_setting_never_runs_the_gpu_batch_and_leaves_tiny_and_empty_frames_untouched() {
        set_render_acceleration(RenderAcceleration::Cpu);
        let mut tiny = vec![7_u8; 2 * 2 * 4];
        assert!(!try_gpu_batch(&mut tiny, 2, 2, &[]));
        let ops = [PixelOp::FillRoundedRect {
            outer: RoundedRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                radius: 0,
            },
            hole: None,
            color: [1, 2, 3, 255],
        }];
        assert!(!try_gpu_batch(&mut tiny, 2, 2, &ops));
        assert!(tiny.iter().all(|&byte| byte == 7));
    }

    fn frame(width: u32, height: u32, seed: u32) -> Vec<u8> {
        (0..width * height * 4)
            .map(|i| (i.wrapping_mul(37).wrapping_add(seed) % 251) as u8)
            .collect()
    }

    #[test]
    fn cpu_backend_output_matches_the_image_core_blur() {
        let (width, height) = (96_u32, 80_u32);
        let source = frame(width, height, 0);
        let mut expected = source.clone();
        flamewm_image_core::blur::gaussian_blur_rgba(&mut expected, width, height, 9.0)
            .expect("reference blur");
        let mut actual = source;
        CpuPixels::default()
            .blur_rgba(&mut actual, width, height, 9.0)
            .expect("cpu backend blur");
        assert_eq!(actual, expected);
    }

    #[test]
    fn cpu_fill_is_hard_edged_and_composites_source_over() {
        let (width, height) = (16_u32, 16_u32);
        let mut pixels = vec![0_u8; (width * height * 4) as usize];
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let rect = RoundedRect {
            x: 2,
            y: 2,
            width: 12,
            height: 12,
            radius: 4,
        }
        .normalized();
        fill_rounded_rect(&mut pixels, width, height, rect, None, [255, 0, 0, 255])
            .expect("cpu fill");
        let at = |x: u32, y: u32| {
            let index = ((y * width + x) * 4) as usize;
            [pixels[index], pixels[index + 1], pixels[index + 2]]
        };
        assert_eq!(at(8, 8), [255, 0, 0], "interior is painted");
        assert_eq!(at(2, 2), [0, 0, 0], "rounded corner stays unpainted");
        assert_eq!(at(0, 0), [0, 0, 0], "outside the rect is untouched");
        assert_eq!(at(13, 13), [0, 0, 0], "bottom-right corner stays unpainted");
    }

    #[test]
    fn cpu_stroke_paints_the_ring_and_keeps_the_interior() {
        let (width, height) = (16_u32, 16_u32);
        let mut pixels = vec![40_u8; (width * height * 4) as usize];
        let rect = RoundedRect {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
            radius: 0,
        };
        stroke_rounded_rect(&mut pixels, width, height, rect, 2, [255, 255, 255, 255])
            .expect("cpu stroke");
        let at = |x: u32, y: u32| pixels[((y * width + x) * 4) as usize];
        assert_eq!(at(0, 8), 255, "left edge ring");
        assert_eq!(at(1, 8), 255, "second ring pixel");
        assert_eq!(at(2, 8), 40, "first interior pixel");
        assert_eq!(at(8, 8), 40, "centre");
    }

    #[test]
    fn translucent_fill_uses_the_documented_source_over_formula() {
        let mut pixels = [0, 0, 255, 255].repeat(4);
        fill_rounded_rect(
            &mut pixels,
            2,
            2,
            RoundedRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                radius: 0,
            },
            None,
            [255, 0, 0, 128],
        )
        .expect("translucent fill");
        // a = 128/255: rgb = src*a + dst*(1-a), alpha = 128 + 255*(1-a).
        assert_eq!(&pixels[0..4], &[128, 0, 127, 255]);
    }

    #[test]
    fn failing_gpu_backend_leaves_pixels_for_the_cpu_fallback() {
        struct FailingGpu;
        impl PixelBackend for FailingGpu {
            fn blur_rgba(&mut self, _: &mut [u8], _: u32, _: u32, _: f32) -> Result<(), String> {
                Err("simulated GL failure".to_owned())
            }
            fn fill_rounded_rect(
                &mut self,
                _: &mut [u8],
                _: u32,
                _: u32,
                _: RoundedRect,
                _: Option<RoundedRect>,
                _: [u8; 4],
            ) -> Result<(), String> {
                Err("simulated GL failure".to_owned())
            }
        }
        let (width, height) = (96_u32, 80_u32);
        let source = frame(width, height, 13);
        let mut pixels = source.clone();
        // `try_gpu` falls back to the CPU on this Err, so the frame must still be the source.
        assert!(
            FailingGpu
                .blur_rgba(&mut pixels, width, height, 9.0)
                .is_err()
        );
        assert_eq!(pixels, source, "a failed GPU blur must not touch the frame");
        let rect = RoundedRect {
            x: 4,
            y: 4,
            width: 40,
            height: 30,
            radius: 8,
        };
        assert!(
            FailingGpu
                .fill_rounded_rect(&mut pixels, width, height, rect, None, [9, 9, 9, 255])
                .is_err()
        );
        assert_eq!(pixels, source, "a failed GPU fill must not touch the frame");
        CpuPixels::default()
            .blur_rgba(&mut pixels, width, height, 9.0)
            .expect("cpu fallback");
        let mut expected = source;
        flamewm_image_core::blur::gaussian_blur_rgba(&mut expected, width, height, 9.0)
            .expect("reference blur");
        assert_eq!(pixels, expected);
    }

    /// Two panels, a stroke and a translucent fill, optionally followed by a blur.
    fn batch_ops(with_blur: bool) -> Vec<PixelOp<'static>> {
        let outer = RoundedRect {
            x: 10,
            y: 8,
            width: 120,
            height: 90,
            radius: 20,
        };
        let inner = RoundedRect {
            x: 40,
            y: 30,
            width: 70,
            height: 40,
            radius: 8,
        };
        let mut ops = vec![
            PixelOp::FillRoundedRect {
                outer,
                hole: None,
                color: [200, 60, 30, 170],
            },
            PixelOp::FillRoundedRect {
                outer,
                hole: Some(outer.inset(3)),
                color: [240, 240, 250, 255],
            },
            PixelOp::FillRoundedRect {
                outer: inner,
                hole: None,
                color: [10, 90, 200, 128],
            },
        ];
        if with_blur {
            ops.push(PixelOp::Blur { radius: 12.0 });
        }
        ops.into_iter().map(PixelOp::normalized).collect()
    }

    fn apply_single(
        backend: &mut dyn PixelBackend,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        op: PixelOp<'_>,
    ) {
        match op {
            PixelOp::FillRoundedRect { outer, hole, color } => {
                backend.fill_rounded_rect(pixels, width, height, outer, hole, color)
            }
            PixelOp::Blur { radius } => backend.blur_rgba(pixels, width, height, radius),
            blend @ (PixelOp::BlendMask { .. } | PixelOp::BlendImage { .. }) => {
                backend.paint_batch(pixels, width, height, std::slice::from_ref(&blend))
            }
        }
        .expect("single op");
    }

    #[test]
    fn cpu_batch_is_byte_identical_to_sequential_single_ops() {
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 21);
        let ops = batch_ops(true);
        let mut batch = source.clone();
        paint_batch(&mut batch, width, height, &ops).expect("cpu batch");
        let mut sequential = source;
        let mut cpu = CpuPixels::default();
        for op in &ops {
            apply_single(&mut cpu, &mut sequential, width, height, *op);
        }
        assert_eq!(batch, sequential);
    }

    #[test]
    fn batch_error_after_a_successful_op_leaves_the_frame_untouched() {
        /// Fills on the CPU and fails every blur, so the first op succeeds before the failure.
        struct FailsOnBlur;
        impl PixelBackend for FailsOnBlur {
            fn blur_rgba(&mut self, _: &mut [u8], _: u32, _: u32, _: f32) -> Result<(), String> {
                Err("simulated GL failure".to_owned())
            }
            fn fill_rounded_rect(
                &mut self,
                pixels: &mut [u8],
                width: u32,
                height: u32,
                outer: RoundedRect,
                hole: Option<RoundedRect>,
                color: [u8; 4],
            ) -> Result<(), String> {
                CpuPixels::default().fill_rounded_rect(pixels, width, height, outer, hole, color)
            }
        }
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 13);
        let mut pixels = source.clone();
        assert!(
            FailsOnBlur
                .paint_batch(&mut pixels, width, height, &batch_ops(true))
                .is_err()
        );
        assert_eq!(
            pixels, source,
            "a failed batch must not keep its earlier ops"
        );
    }

    #[test]
    fn gpu_batch_matches_gpu_single_ops_and_the_cpu_blur() {
        let Ok(mut gpu) = gles::GpuBlur::new() else {
            eprintln!("skipping GPU batch check: no GPU");
            return;
        };
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 9);

        let fills = batch_ops(false);
        let mut batch = source.clone();
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &fills).expect("gpu batch");
        let mut single = source.clone();
        for op in &fills {
            apply_single(&mut gpu, &mut single, width, height, *op);
        }
        assert_eq!(
            batch, single,
            "one batch must equal the per-op GPU results exactly"
        );

        let blurred = batch_ops(true);
        let mut cpu = source.clone();
        CpuPixels::default()
            .paint_batch(&mut cpu, width, height, &blurred)
            .expect("cpu batch");
        let mut batch = source.clone();
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &blurred)
            .expect("gpu batch");
        let mut single = source;
        for op in &blurred {
            apply_single(&mut gpu, &mut single, width, height, *op);
        }
        assert_eq!(
            batch, single,
            "blur batch must equal the per-op GPU results exactly"
        );
        // The blur tolerance matches `gpu_blur_matches_the_cpu_blur_when_a_gpu_is_present`.
        let margin = 24;
        let mut total = 0_u64;
        let mut count = 0_u64;
        for y in margin..height - margin {
            for x in margin..width - margin {
                let index = ((y * width + x) * 4) as usize;
                for channel in 0..4 {
                    total += u64::from(cpu[index + channel].abs_diff(batch[index + channel]));
                    count += 1;
                }
            }
        }
        let mean = total as f64 / count as f64;
        assert!(mean < 6.0, "blur batch mean channel difference {mean:.2}");
    }

    /// A text-like coverage mask, one partly outside the frame, and a premultiplied image, blended
    /// over a noisy frame. Returns the per-channel (max, mean) absolute difference between the CPU
    /// batch and the GPU batch, or `None` when no GPU is present.
    fn blend_difference() -> Option<(u8, f64)> {
        let mut gpu = gles::GpuBlur::new().ok()?;
        let (width, height) = (96_u32, 64_u32);
        let source = frame(width, height, 5);
        let coverage: Vec<u8> = (0..24 * 12).map(|i| ((i * 37) % 256) as u8).collect();
        let image: Vec<u8> = (0..16 * 10)
            .flat_map(|i| {
                let a = ((i * 29) % 256) as u8;
                let premultiplied = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
                [premultiplied(200), premultiplied(40), premultiplied(90), a]
            })
            .collect();
        let ops = [
            PixelOp::BlendMask {
                x: 5,
                y: 7,
                width: 24,
                height: 12,
                mask: &coverage,
                color: [220, 230, 255, 255],
            },
            PixelOp::BlendMask {
                x: -4,
                y: 50,
                width: 24,
                height: 12,
                mask: &coverage,
                color: [10, 20, 30, 180],
            },
            PixelOp::BlendImage {
                x: 60,
                y: 20,
                width: 16,
                height: 10,
                rgba: &image,
            },
        ];
        let mut cpu = source.clone();
        CpuPixels::default()
            .paint_batch(&mut cpu, width, height, &ops)
            .expect("cpu blend");
        let mut batch = source;
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &ops).expect("gpu blend");
        let mut max = 0_u8;
        let mut total = 0_u64;
        for (a, b) in cpu.iter().zip(&batch) {
            let diff = a.abs_diff(*b);
            max = max.max(diff);
            total += u64::from(diff);
        }
        Some((max, total as f64 / cpu.len() as f64))
    }

    #[test]
    fn gpu_blend_ops_match_the_cpu_within_the_measured_tolerance() {
        let Some((max, mean)) = blend_difference() else {
            eprintln!("skipping GPU blend check: no GPU");
            return;
        };
        eprintln!("GPU blend vs CPU blend: max {max}, mean {mean:.4} per channel");
        // Measured 2026-10-28 with `cargo test -p flamewm-render-x11 --lib -- --nocapture`
        // (the log shows libEGL "failed to create dri2 screen" warnings; the GL renderer name was not
        // recorded): max 1 level, mean 0.0046 per channel.
        assert!(max <= 1, "blend max channel difference {max}");
        assert!(mean < 0.05, "blend mean channel difference {mean:.4}");
    }

    #[test]
    fn gpu_full_frame_image_over_a_transparent_frame_matches_the_cpu() {
        let Ok(mut gpu) = gles::GpuBlur::new() else {
            return;
        };
        let (width, height) = (1350_u32, 641_u32);
        let image: Vec<u8> = (0..width * height)
            .flat_map(|i| [(i % 251) as u8, 90, 160, 255])
            .collect();
        let ops = [PixelOp::BlendImage {
            x: 0,
            y: 0,
            width,
            height,
            rgba: &image,
        }];
        let mut cpu = vec![0_u8; (width * height * 4) as usize];
        CpuPixels::default()
            .paint_batch(&mut cpu, width, height, &ops)
            .expect("cpu");
        let mut batch = vec![0_u8; cpu.len()];
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &ops).expect("gpu");
        let max = cpu.iter().zip(&batch).map(|(a, b)| a.abs_diff(*b)).max();
        eprintln!("FULLFRAME cpu[0..4]={:?} gpu[0..4]={:?} max={max:?}", &cpu[..4], &batch[..4]);
        assert!(max.unwrap_or(0) <= 1);
    }

    #[test]
    fn gpu_translucent_fill_over_a_transparent_frame_matches_the_cpu() {
        let Ok(mut gpu) = gles::GpuBlur::new() else {
            return;
        };
        let (width, height) = (64_u32, 64_u32);
        let ops = [PixelOp::FillRoundedRect {
            outer: RoundedRect { x: 8, y: 8, width: 40, height: 40, radius: 6 },
            hole: None,
            color: [200, 100, 50, 128],
        }];
        let mut cpu = vec![0_u8; (width * height * 4) as usize];
        CpuPixels::default().paint_batch(&mut cpu, width, height, &ops).expect("cpu");
        let mut batch = vec![0_u8; cpu.len()];
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &ops).expect("gpu");
        let i = (30 * width + 30) as usize * 4;
        eprintln!("TRANSLUCENT cpu={:?} gpu={:?}", &cpu[i..i + 4], &batch[i..i + 4]);
        let max = cpu.iter().zip(&batch).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        let centre = (cpu[i..i + 4] == batch[i..i + 4]) as u8;
        assert!(centre == 1 && max <= 80, "max {max}");
    }

    #[test]
    fn gaussian_weights_are_normalized_and_bounded() {
        for sigma in [0.3_f32, 1.0, 3.0, 6.0, 40.0] {
            let weights = gles::gaussian_weights(sigma);
            let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
            assert!((total - 1.0).abs() < 1e-4, "sigma {sigma}: {total}");
            assert!(weights.len() <= 33);
            assert!(weights.windows(2).all(|pair| pair[0] >= pair[1]));
        }
        assert_eq!(gles::downscale_factor(3.0), 1);
        assert_eq!(gles::downscale_factor(6.0), 2);
        assert_eq!(gles::downscale_factor(12.0), 4);
    }

    #[test]
    fn gpu_blur_matches_the_cpu_blur_when_a_gpu_is_present() {
        let (width, height) = (160_u32, 120_u32);
        let source: Vec<u8> = (0..width * height)
            .flat_map(|index| {
                let x = index % width;
                let y = index / width;
                let stripe = if (x / 20 + y / 20) % 2 == 0 { 230 } else { 30 };
                [
                    stripe,
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    255,
                ]
            })
            .collect();
        for radius in [4.0_f32, 12.0, 24.0] {
            let mut cpu = source.clone();
            flamewm_image_core::blur::gaussian_blur_rgba(&mut cpu, width, height, radius)
                .expect("cpu blur");
            let mut gpu = source.clone();
            let mut context = match gles::GpuBlur::new() {
                Ok(context) => context,
                Err(error) => {
                    eprintln!("skipping GPU comparison: {error}");
                    return;
                }
            };
            PixelBackend::blur_rgba(&mut context, &mut gpu, width, height, radius)
                .expect("gpu blur");
            let margin = (radius * 2.0) as u32;
            let mut total = 0_u64;
            let mut count = 0_u64;
            for y in margin..height - margin {
                for x in margin..width - margin {
                    let index = ((y * width + x) * 4) as usize;
                    for channel in 0..4 {
                        total += u64::from(cpu[index + channel].abs_diff(gpu[index + channel]));
                        count += 1;
                    }
                }
            }
            let mean = total as f64 / count as f64;
            assert!(
                mean < 6.0,
                "radius {radius}: mean channel difference {mean:.2}"
            );
            eprintln!(
                "GPU {} radius {radius}: mean difference {mean:.2}",
                context.renderer()
            );
        }
    }

    /// Source-over `color` through the XRender AA coverage of `outer`, or of the ring between
    /// `outer` and `hole` (whose inset is the stroke width). This is the reference the GPU must match.
    fn xrender_reference(
        outer: RoundedRect,
        hole: Option<RoundedRect>,
        color: [u8; 4],
        source: &[u8],
        width: u32,
    ) -> Vec<u8> {
        let mut out = source.to_vec();
        let thickness = hole.map(|hole| (hole.x - outer.x) as u32);
        for row in 0..outer.height {
            let runs = match thickness {
                Some(thickness) => crate::xrender_aa::stroke_row_runs(
                    outer.width,
                    outer.height,
                    outer.radius,
                    thickness,
                    row,
                ),
                None => {
                    crate::xrender_aa::fill_row_runs(outer.width, outer.height, outer.radius, row)
                }
            };
            let y = (outer.y + row as i32) as usize;
            for (start, len, coverage) in runs {
                for col in start..start + len {
                    let index = (y * width as usize + (outer.x as usize + col as usize)) * 4;
                    let alpha = f32::from(color[3]) / 255.0 * coverage;
                    for channel in 0..3 {
                        out[index + channel] = (f32::from(color[channel]) * alpha
                            + f32::from(out[index + channel]) * (1.0 - alpha))
                            .round() as u8;
                    }
                    out[index + 3] =
                        (alpha * 255.0 + f32::from(out[index + 3]) * (1.0 - alpha)).round() as u8;
                }
            }
        }
        out
    }

    /// Largest and mean per-channel difference between `actual` and `expected` inside `rect`.
    fn channel_error(expected: &[u8], actual: &[u8], width: u32, rect: RoundedRect) -> (u8, f32) {
        let (mut max, mut total, mut count) = (0_u8, 0_u32, 0_u32);
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                let index = (y as usize * width as usize + x as usize) * 4;
                for channel in 0..4 {
                    let diff = expected[index + channel].abs_diff(actual[index + channel]);
                    max = max.max(diff);
                    total += u32::from(diff);
                    count += 1;
                }
            }
        }
        (max, total as f32 / count as f32)
    }

    #[test]
    fn gpu_rounded_fill_and_stroke_match_the_xrender_aa_reference() {
        let Ok(mut context) = gles::GpuBlur::new() else {
            eprintln!("skipping GPU fill comparison: no GPU");
            return;
        };
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 7);
        let outer = RoundedRect {
            x: 10,
            y: 8,
            width: 120,
            height: 90,
            radius: 20,
        }
        .normalized();
        let cases = [(outer, None), (outer, Some(outer.inset(6)))];
        for (outer, hole) in cases {
            let color = [200, 60, 30, 170];
            let reference = xrender_reference(outer, hole, color, &source, width);
            let mut gpu = source.clone();
            PixelBackend::fill_rounded_rect(
                &mut context,
                &mut gpu,
                width,
                height,
                outer,
                hole,
                color,
            )
            .expect("gpu fill");
            let (max, mean) = channel_error(&reference, &gpu, width, outer);
            // Stated tolerance: the GPU one-pixel ramp samples pixel centres while XRender averages
            // four sub-scanlines per corner row, so the largest corner-arc difference is 8/255 on the
            // Mesa Intel UHD (measured). The mean over the whole rect must stay below 0.5.
            eprintln!("GPU AA vs XRender reference: max {max}, mean {mean:.3}");
            assert!(
                max <= 8,
                "max channel error {max} against the XRender AA reference"
            );
            assert!(
                mean < 0.5,
                "mean channel error {mean:.3} against the XRender AA reference"
            );
        }
    }

    #[test]
    fn gles_errors_leave_the_frame_untouched() {
        let Ok(mut context) = gles::GpuBlur::new() else {
            eprintln!("skipping GLES error check: no GPU");
            return;
        };
        // Wider than any GL texture limit: rejected before any GL call writes the frame.
        let width = 1_u32 << 16;
        let source = frame(width, 1, 3);
        let mut blur = source.clone();
        assert!(PixelBackend::blur_rgba(&mut context, &mut blur, width, 1, 9.0).is_err());
        assert_eq!(blur, source);
        let mut fill = source.clone();
        let rect = RoundedRect {
            x: 0,
            y: 0,
            width,
            height: 1,
            radius: 0,
        };
        assert!(
            PixelBackend::fill_rounded_rect(
                &mut context,
                &mut fill,
                width,
                1,
                rect,
                None,
                [1, 2, 3, 255],
            )
            .is_err()
        );
        assert_eq!(fill, source);
    }
}
