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
                pixel[3] = (f32::from(color[3]) + f32::from(pixel[3]) * (1.0 - alpha)).round() as u8;
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
        ops: &[PixelOp],
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

/// Applies `ops` (normalized, see [`PixelOp::normalized`]) to one frame through the same selection
/// as the single-op calls: one GPU upload and readback when `renderAcceleration` is `gpu`, otherwise
/// the CPU batch. Any GPU failure leaves the frame untouched and runs the batch on the CPU.
pub fn paint_batch(pixels: &mut [u8], width: u32, height: u32, ops: &[PixelOp]) -> Result<(), String> {
    let mut op = |backend: &mut dyn PixelBackend| backend.paint_batch(pixels, width, height, ops);
    if try_gpu(&mut op) {
        return Ok(());
    }
    CPU.with(|cpu| op(&mut *cpu.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let rect = RoundedRect { x: 2, y: 2, width: 12, height: 12, radius: 4 }.normalized();
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
        let rect = RoundedRect { x: 0, y: 0, width: 16, height: 16, radius: 0 };
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
            RoundedRect { x: 0, y: 0, width: 2, height: 2, radius: 0 },
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
        assert!(FailingGpu.blur_rgba(&mut pixels, width, height, 9.0).is_err());
        assert_eq!(pixels, source, "a failed GPU blur must not touch the frame");
        let rect = RoundedRect { x: 4, y: 4, width: 40, height: 30, radius: 8 };
        assert!(FailingGpu
            .fill_rounded_rect(&mut pixels, width, height, rect, None, [9, 9, 9, 255])
            .is_err());
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
    fn batch_ops(with_blur: bool) -> Vec<PixelOp> {
        let outer = RoundedRect { x: 10, y: 8, width: 120, height: 90, radius: 20 };
        let inner = RoundedRect { x: 40, y: 30, width: 70, height: 40, radius: 8 };
        let mut ops = vec![
            PixelOp::FillRoundedRect { outer, hole: None, color: [200, 60, 30, 170] },
            PixelOp::FillRoundedRect {
                outer,
                hole: Some(outer.inset(3)),
                color: [240, 240, 250, 255],
            },
            PixelOp::FillRoundedRect { outer: inner, hole: None, color: [10, 90, 200, 128] },
        ];
        if with_blur {
            ops.push(PixelOp::Blur { radius: 12.0 });
        }
        ops.into_iter().map(PixelOp::normalized).collect()
    }

    fn apply_single(backend: &mut dyn PixelBackend, pixels: &mut [u8], width: u32, height: u32, op: PixelOp) {
        match op {
            PixelOp::FillRoundedRect { outer, hole, color } => {
                backend.fill_rounded_rect(pixels, width, height, outer, hole, color)
            }
            PixelOp::Blur { radius } => backend.blur_rgba(pixels, width, height, radius),
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
        assert!(FailsOnBlur
            .paint_batch(&mut pixels, width, height, &batch_ops(true))
            .is_err());
        assert_eq!(pixels, source, "a failed batch must not keep its earlier ops");
    }

    /// The largest number of pixels that differ by more than 2 in any channel, and the outline
    /// budget that the corner arcs may use.
    fn fill_disagreements(cpu: &[u8], gpu: &[u8], ops: &[PixelOp]) -> (usize, usize) {
        let differing = cpu
            .chunks_exact(4)
            .zip(gpu.chunks_exact(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 2))
            .count();
        let budget = ops
            .iter()
            .map(|op| match op {
                PixelOp::FillRoundedRect { outer, .. } => 2 * (outer.width + outer.height) as usize,
                PixelOp::Blur { .. } => 0,
            })
            .sum();
        (differing, budget)
    }

    #[test]
    fn gpu_batch_matches_gpu_single_ops_and_the_cpu_batch() {
        let Ok(mut gpu) = gles::GpuBlur::new() else {
            eprintln!("skipping GPU batch check: no GPU");
            return;
        };
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 9);

        let fills = batch_ops(false);
        let mut cpu = source.clone();
        CpuPixels::default().paint_batch(&mut cpu, width, height, &fills).expect("cpu fills");
        let mut batch = source.clone();
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &fills).expect("gpu batch");
        let mut single = source.clone();
        for op in &fills {
            apply_single(&mut gpu, &mut single, width, height, *op);
        }
        assert_eq!(batch, single, "one batch must equal the per-op GPU results exactly");
        let (differing, budget) = fill_disagreements(&cpu, &batch, &fills);
        assert!(differing <= budget, "{differing} GPU fill pixels differ from CPU, budget {budget}");

        let blurred = batch_ops(true);
        let mut cpu = source.clone();
        CpuPixels::default().paint_batch(&mut cpu, width, height, &blurred).expect("cpu batch");
        let mut batch = source.clone();
        PixelBackend::paint_batch(&mut gpu, &mut batch, width, height, &blurred).expect("gpu batch");
        let mut single = source;
        for op in &blurred {
            apply_single(&mut gpu, &mut single, width, height, *op);
        }
        assert_eq!(batch, single, "blur batch must equal the per-op GPU results exactly");
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

    #[test]
    fn gpu_rounded_fill_and_stroke_match_the_cpu_when_a_gpu_is_present() {
        let Ok(mut context) = gles::GpuBlur::new() else {
            eprintln!("skipping GPU fill comparison: no GPU");
            return;
        };
        let (width, height) = (160_u32, 120_u32);
        let source = frame(width, height, 7);
        let outer = RoundedRect { x: 10, y: 8, width: 120, height: 90, radius: 20 }.normalized();
        let cases = [
            (outer, None),
            (outer, Some(outer.inset(6))),
        ];
        for (outer, hole) in cases {
            let mut cpu = source.clone();
            let mut gpu = source.clone();
            CpuPixels::default()
                .fill_rounded_rect(&mut cpu, width, height, outer, hole, [200, 60, 30, 170])
                .expect("cpu fill");
            PixelBackend::fill_rounded_rect(
                &mut context,
                &mut gpu,
                width,
                height,
                outer,
                hole,
                [200, 60, 30, 170],
            )
            .expect("gpu fill");
            // The CPU floors each row inset and the shader samples pixel centres, so corner arcs
            // may disagree by a pixel. Every other pixel must match within rounding.
            let differing = cpu
                .chunks_exact(4)
                .zip(gpu.chunks_exact(4))
                .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 2))
                .count();
            let outline = 2 * (outer.width + outer.height) as usize;
            assert!(
                differing <= outline,
                "{differing} pixels differ by over 2, outline budget {outline}"
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
        let rect = RoundedRect { x: 0, y: 0, width, height: 1, radius: 0 };
        assert!(PixelBackend::fill_rounded_rect(
            &mut context,
            &mut fill,
            width,
            1,
            rect,
            None,
            [1, 2, 3, 255],
        )
        .is_err());
        assert_eq!(fill, source);
    }
}
