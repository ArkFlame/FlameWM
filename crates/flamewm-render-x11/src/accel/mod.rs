mod gles;

use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

pub use flamewm_render_core::{PixelBackend, RenderAcceleration};

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

pub(crate) fn gpu_blur_rgba(pixels: &mut [u8], width: u32, height: u32, radius: f32) -> bool {
    if render_acceleration() != RenderAcceleration::Gpu
        || radius < 0.5
        || (width as usize) * (height as usize) < MIN_GPU_PIXELS
    {
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
        match PixelBackend::blur_rgba(gpu.as_mut(), pixels, width, height, radius) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("flamewm-render-x11: GPU blur failed, using CPU: {error}");
                record(None, Some(error));
                *slot = GpuSlot::Failed;
                false
            }
        }
    })
}

pub(crate) fn blur_rgba(pixels: &mut [u8], width: u32, height: u32, radius: f32) {
    if gpu_blur_rgba(pixels, width, height, radius) {
        return;
    }
    let result = CPU.with(|cpu| cpu.borrow_mut().blur_rgba(pixels, width, height, radius));
    if let Err(error) = result {
        eprintln!("flamewm-render-x11: backdrop blur skipped: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_backend_output_matches_the_image_core_blur() {
        let (width, height) = (96_u32, 80_u32);
        let source: Vec<u8> = (0..width * height * 4).map(|i| (i * 37 % 251) as u8).collect();
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
    fn failing_gpu_backend_leaves_pixels_for_the_cpu_fallback() {
        struct FailingGpu;
        impl PixelBackend for FailingGpu {
            fn blur_rgba(&mut self, _: &mut [u8], _: u32, _: u32, _: f32) -> Result<(), String> {
                Err("simulated GL failure".to_owned())
            }
        }
        let (width, height) = (96_u32, 80_u32);
        let source: Vec<u8> = (0..width * height * 4).map(|i| (i * 13 % 255) as u8).collect();
        let mut pixels = source.clone();
        // `gpu_blur_rgba` returns false on this Err, so the CPU path below runs on the same frame.
        assert!(FailingGpu.blur_rgba(&mut pixels, width, height, 9.0).is_err());
        assert_eq!(pixels, source, "a failed GPU pass must not touch the frame");
        CpuPixels::default()
            .blur_rgba(&mut pixels, width, height, 9.0)
            .expect("cpu fallback");
        let mut expected = source;
        flamewm_image_core::blur::gaussian_blur_rgba(&mut expected, width, height, 9.0)
            .expect("reference blur");
        assert_eq!(pixels, expected);
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
}
