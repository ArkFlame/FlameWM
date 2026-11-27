//! Fault-injection coverage for the hardware -> software -> CPU fallback chain. No helper binary is
//! spawned and no GL driver is loaded: the injected seams drive the real tier selection, failure
//! recording and CPU hand-off instead.

use super::helper;
use super::*;

/// Serializes status assertions. Shares one lock with the accel unit tests in `mod.rs`, which use the same
/// global status.
use super::tests::TEST_SERIAL as STATUS_LOCK;

/// Helper stand-in: reports `device` on start and fails every op when `op_error` is set, as a child
/// that died or whose reply timed out would.
struct FakeHelper {
    device: &'static str,
    op_error: Option<&'static str>,
}

impl FakeHelper {
    fn result(&self) -> Result<(), String> {
        match self.op_error {
            Some(error) => Err(error.to_owned()),
            None => Ok(()),
        }
    }
}

impl PixelBackend for FakeHelper {
    fn blur_rgba(&mut self, _: &mut [u8], _: u32, _: u32, _: f32) -> Result<(), String> {
        self.result()
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
        self.result()
    }

    fn paint_batch(
        &mut self,
        _: &mut [u8],
        _: u32,
        _: u32,
        _: &[PixelOp<'_>],
    ) -> Result<(), String> {
        self.result()
    }
}

impl GpuPixels for FakeHelper {
    fn kind(&self) -> RenderBackend {
        RenderBackend::OpenGl
    }

    fn device(&self) -> &str {
        self.device
    }
}

fn reset_slot() {
    GPU.with(|cell| cell.borrow_mut().slot = GpuSlot::Untried);
}

#[test]
fn missing_helper_binary_falls_through_both_tiers_to_cpu() {
    let _guard = STATUS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    reset_slot();
    let attempted = RefCell::new(Vec::new());
    let spawn = |mode: helper::GpuMode| -> Result<Box<dyn GpuPixels>, String> {
        attempted.borrow_mut().push(mode);
        Err("flamewm-gpu-helper not found next to this binary".to_owned())
    };

    // A missing program is an Err at both stages; selection must reach the software tier and then
    // fail the slot rather than unwind.
    let slot = open_gpu_with(&spawn);
    assert_eq!(
        *attempted.borrow(),
        vec![helper::GpuMode::Hardware, helper::GpuMode::Software]
    );
    assert!(
        matches!(slot, GpuSlot::Failed),
        "a missing helper must leave no usable slot"
    );
    let status = render_backend_status();
    assert_eq!(status.active, RenderBackend::Cpu);
    assert_eq!(status.device, None);
    assert_eq!(
        status.fallback,
        Some(FallbackReason::InitFailed(
            "flamewm-gpu-helper not found next to this binary".to_owned()
        ))
    );
}

#[test]
fn helper_exiting_nonzero_at_start_fails_hardware_then_keeps_software_device() {
    let _guard = STATUS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    reset_slot();
    let attempted = RefCell::new(Vec::new());
    let spawn = |mode: helper::GpuMode| -> Result<Box<dyn GpuPixels>, String> {
        attempted.borrow_mut().push(mode);
        match mode {
            helper::GpuMode::Hardware => Err("helper exited with status 1 at start".to_owned()),
            helper::GpuMode::Software => Ok(Box::new(FakeHelper {
                device: "llvmpipe (LLVM 17.0.6, 256 bits)",
                op_error: None,
            })),
        }
    };

    let slot = open_gpu_with(&spawn);
    assert_eq!(
        *attempted.borrow(),
        vec![helper::GpuMode::Hardware, helper::GpuMode::Software]
    );
    let GpuSlot::Ready(gpu) = slot else {
        panic!("the software tier must be kept when it starts");
    };
    let device = "llvmpipe (LLVM 17.0.6, 256 bits)";
    assert_eq!(gpu.device(), device);
    let status = render_backend_status();
    assert_eq!(status.active, RenderBackend::OpenGl);
    assert_eq!(status.device.as_deref(), Some(device));
    assert_eq!(
        status.fallback,
        Some(FallbackReason::SoftwareDevice(device.to_owned()))
    );
}

#[test]
fn helper_timeout_or_death_mid_session_falls_back_to_cpu() {
    let _guard = STATUS_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    for error in ["helper reply timed out", "helper exited mid-session"] {
        reset_slot();
        let attempted = RefCell::new(Vec::new());
        let spawn = |mode: helper::GpuMode| -> Result<Box<dyn GpuPixels>, String> {
            attempted.borrow_mut().push(mode);
            Ok(Box::new(FakeHelper {
                device: "fake hardware GPU",
                op_error: Some(error),
            }))
        };
        let (width, height) = (64_u32, 64_u32);
        let source = vec![11_u8; (width * height * 4) as usize];
        let mut pixels = source.clone();
        let ops = [PixelOp::FillRoundedRect {
            outer: RoundedRect {
                x: 4,
                y: 4,
                width: 40,
                height: 40,
                radius: 4,
            },
            hole: None,
            color: [200, 10, 10, 255],
        }];

        // The child starts, then dies (or stops replying) on the first op. No software retry is
        // meaningful here: the failing op must hand the untouched frame back to the CPU tier.
        let gpu_ran = {
            let mut op = |backend: &mut dyn PixelBackend| {
                backend.paint_batch(&mut pixels, width, height, &ops)
            };
            try_gpu_with(&mut op, RenderAcceleration::Gpu, &|| open_gpu_with(&spawn))
        };
        assert!(!gpu_ran, "{error}");
        assert_eq!(
            *attempted.borrow(),
            vec![helper::GpuMode::Hardware],
            "{error}"
        );
        assert_eq!(
            pixels, source,
            "a failed GPU op must leave the frame untouched"
        );
        GPU.with(|cell| assert!(matches!(cell.borrow().slot, GpuSlot::Failed), "{error}"));
        let status = render_backend_status();
        assert_eq!(status.active, RenderBackend::Cpu, "{error}");
        assert_eq!(status.device, None, "{error}");
        assert_eq!(
            status.fallback,
            Some(FallbackReason::DeviceLost(error.to_owned())),
            "{error}"
        );

        let cpu_result = {
            let mut op = |backend: &mut dyn PixelBackend| {
                backend.paint_batch(&mut pixels, width, height, &ops)
            };
            CPU.with(|cpu| op(&mut *cpu.borrow_mut()))
        };
        cpu_result.expect("cpu fallback");
        assert_ne!(
            pixels, source,
            "the CPU tier must paint after the GPU failure"
        );
    }
}
