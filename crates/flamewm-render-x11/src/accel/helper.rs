//! GPU work in a child process, `flamewm-gpu-helper`. The parent never loads a GL driver, so
//! ending the child (a switch to CPU, or dropping the slot) returns the driver's memory to the OS.
//!
//! Wire format: every message is a `u32` little-endian length, then the body. A request body is
//! `width u32, height u32, frame (u32 len + bytes), op count u32, ops`. A reply body is a status
//! byte, then the frame bytes (status 0) or UTF-8 error text (status 1). The helper's first reply
//! carries its device name, or the reason GL could not start.

use super::*;
use flamewm_render_core::{PixelBackend, PixelOp, RoundedRect};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const ENV_HELPER: &str = "FLAMEWM_GPU_HELPER";
const HELPER_NAME: &str = "flamewm-gpu-helper";

/// Which GL device the helper child opens. `Software` asks Mesa for an llvmpipe context and is the
/// second stage of the fallback chain when hardware GL cannot start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GpuMode {
    Hardware,
    Software,
}

const TAG_FILL: u8 = 0;
const TAG_BLUR: u8 = 1;
const TAG_MASK: u8 = 2;
const TAG_IMAGE: u8 = 3;
const TAG_BACKDROP: u8 = 4;
const TAG_FILL_CLIPPED: u8 = 5;

/// A GPU slot whose device lives in a child process. Dropping it kills the child.
pub(super) struct RemoteGpu {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    device: String,
    /// Retained wire scratch: one request body and one reply body, so a batch never allocates a
    /// whole-frame buffer per call.
    request: Vec<u8>,
    reply: Vec<u8>,
}

impl RemoteGpu {
    pub(super) fn spawn(mode: GpuMode) -> Result<Self, String> {
        let program = helper_path()?;
        let mut command = Command::new(&program);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if mode == GpuMode::Software {
            // Software GL is requested on the child only: the parent never loads a GL driver and
            // its environment stays untouched, so a later hardware attempt is still possible.
            command
                .env("LIBGL_ALWAYS_SOFTWARE", "1")
                .env("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe")
                .env("GALLIUM_DRIVER", "llvmpipe")
                .env("EGL_PLATFORM", "surfaceless");
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", program.display()))?;
        let stdin = child.stdin.take().ok_or("helper stdin is missing")?;
        let stdout = child.stdout.take().ok_or("helper stdout is missing")?;
        let mut remote = Self {
            child,
            stdin,
            stdout,
            device: String::new(),
            request: Vec::new(),
            reply: Vec::new(),
        };
        pipe_exchange_with_deadline(
            &mut remote.child,
            None,
            &mut remote.stdout,
            &mut remote.reply,
        )?;
        remote.device = String::from_utf8_lossy(reply_payload(&remote.reply)?).into_owned();
        Ok(remote)
    }

    /// Sends `self.request` and reads the reply into `self.reply`, both under one deadline.
    fn exchange(&mut self) -> Result<(), String> {
        pipe_exchange_with_deadline(
            &mut self.child,
            Some((&mut self.stdin, self.request.as_slice())),
            &mut self.stdout,
            &mut self.reply,
        )
    }
}

/// Runs one helper pipe exchange under a deadline. The request write and the reply read share one
/// scoped thread, so a child that stops reading stdin (a full pipe would block `write_all`) or
/// stops replying is killed and the paint thread is released instead of stalling. The caller turns
/// the timeout error into `FallbackReason::DeviceLost` and paints the frame on the CPU raster.
fn pipe_exchange_with_deadline(
    child: &mut Child,
    send: Option<(&mut ChildStdin, &[u8])>,
    stdout: &mut ChildStdout,
    reply: &mut Vec<u8>,
) -> Result<(), String> {
    const EXCHANGE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(1000);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let outcome = match send {
                Some((stdin, request)) => {
                    write_message(stdin, request).and_then(|()| read_message(stdout, reply))
                }
                None => read_message(stdout, reply),
            };
            let _ = tx.send(outcome);
        });
        match rx.recv_timeout(EXCHANGE_DEADLINE) {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                Err("helper reply timed out".to_owned())
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("helper reply reader disconnected".to_owned())
            }
        }
    })
}

impl Drop for RemoteGpu {
    fn drop(&mut self) {
        // The driver lives only in the child, so killing it is what frees the device memory.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl GpuPixels for RemoteGpu {
    fn kind(&self) -> RenderBackend {
        RenderBackend::OpenGl
    }

    fn device(&self) -> &str {
        &self.device
    }
}

impl PixelBackend for RemoteGpu {
    fn blur_rgba(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        radius: f32,
    ) -> Result<(), String> {
        self.paint_batch(pixels, width, height, &[PixelOp::Blur { radius }])
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
        self.paint_batch(
            pixels,
            width,
            height,
            &[PixelOp::FillRoundedRect { outer, hole, color }],
        )
    }

    fn paint_batch(
        &mut self,
        pixels: &mut [u8],
        width: u32,
        height: u32,
        ops: &[PixelOp<'_>],
    ) -> Result<(), String> {
        self.request.clear();
        encode_request_into(&mut self.request, width, height, pixels, ops)?;
        self.exchange()?;
        let frame = reply_payload(&self.reply)?;
        if frame.len() != pixels.len() {
            return Err("helper returned a frame of the wrong size".to_owned());
        }
        pixels.copy_from_slice(frame);
        Ok(())
    }
}

/// Entry point of the `flamewm-gpu-helper` binary: serves requests on stdin and stdout until the
/// parent closes stdin. The device is opened once, before the first request.
pub fn serve_gpu_helper() -> Result<(), String> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut gpu = match gles::GpuBlur::new() {
        Ok(gpu) => gpu,
        Err(error) => {
            // The first reply carries the failure reason; returning Ok exits this helper cleanly so
            // no child abort reaches the parent. The parent degrades through software GL to CPU.
            write_message(&mut output, &reply_error(&error))?;
            eprintln!("flamewm-gpu-helper: GL init failed: {error}");
            return Ok(());
        }
    };
    // One message buffer, one frame buffer and one reply buffer for the life of the helper: a
    // request reuses their capacity instead of allocating a whole frame per call.
    let mut body = Vec::new();
    let mut frame = Vec::new();
    let mut reply = reply_ok(gpu.device().as_bytes());
    write_message(&mut output, &reply)?;
    loop {
        // A closed or broken stdin means the parent is gone or dropped the slot: exit cleanly.
        if read_message(&mut input, &mut body).is_err() {
            return Ok(());
        }
        reply.clear();
        match serve_one(&mut gpu, &body, &mut frame, &mut reply) {
            Ok(()) => {}
            Err(error) => {
                reply.push(1);
                reply.extend_from_slice(error.as_bytes());
            }
        }
        write_message(&mut output, &reply)?;
    }
}

/// Runs one decoded request into `frame` and writes the status byte plus the result into `reply`.
fn serve_one(
    gpu: &mut gles::GpuBlur,
    body: &[u8],
    frame: &mut Vec<u8>,
    reply: &mut Vec<u8>,
) -> Result<(), String> {
    let request = decode_request(body)?;
    frame.clear();
    frame.extend_from_slice(request.frame);
    gpu.paint_batch(frame, request.width, request.height, &request.ops)?;
    reply.push(0);
    reply.extend_from_slice(frame);
    Ok(())
}

/// `FLAMEWM_GPU_HELPER` overrides the path; otherwise the helper sits next to this binary.
fn helper_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(ENV_HELPER) {
        return Ok(path.into());
    }
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(HELPER_NAME)));
    match sibling {
        Some(path) if path.exists() => Ok(path),
        _ => Err(format!("{HELPER_NAME} not found next to this binary")),
    }
}

fn io_error(context: &'static str) -> impl Fn(io::Error) -> String {
    move |error| format!("{context}: {error}")
}

fn write_message(out: &mut impl Write, body: &[u8]) -> Result<(), String> {
    let len = u32::try_from(body.len()).map_err(|_| "helper message is too large".to_owned())?;
    out.write_all(&len.to_le_bytes())
        .and_then(|()| out.write_all(body))
        .and_then(|()| out.flush())
        .map_err(io_error("helper write failed"))
}

fn read_message(input: &mut impl Read, body: &mut Vec<u8>) -> Result<(), String> {
    let mut len = [0_u8; 4];
    input
        .read_exact(&mut len)
        .map_err(io_error("helper read failed"))?;
    let len = u32::from_le_bytes(len) as usize;
    body.clear();
    body.resize(len, 0);
    input
        .read_exact(body.as_mut_slice())
        .map_err(io_error("helper read failed"))
}

fn reply_ok(payload: &[u8]) -> Vec<u8> {
    let mut reply = Vec::with_capacity(payload.len() + 1);
    reply.push(0);
    reply.extend_from_slice(payload);
    reply
}

fn reply_error(text: &str) -> Vec<u8> {
    let mut reply = vec![1];
    reply.extend_from_slice(text.as_bytes());
    reply
}

/// The frame or device text of a reply body, borrowed from `body`.
fn reply_payload(body: &[u8]) -> Result<&[u8], String> {
    match body.split_first() {
        Some((0, payload)) => Ok(payload),
        Some((_, text)) => Err(String::from_utf8_lossy(text).into_owned()),
        None => Err("empty helper reply".to_owned()),
    }
}

#[cfg(test)]
fn parse_reply(body: &[u8]) -> Result<Vec<u8>, String> {
    reply_payload(body).map(<[u8]>::to_vec)
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), String> {
    put_u32(
        out,
        u32::try_from(bytes.len()).map_err(|_| "helper payload is too large".to_owned())?,
    );
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_rect(out: &mut Vec<u8>, rect: RoundedRect) {
    put_i32(out, rect.x);
    put_i32(out, rect.y);
    put_u32(out, rect.width);
    put_u32(out, rect.height);
    put_u32(out, rect.radius);
}

fn put_opt_rect(out: &mut Vec<u8>, rect: Option<RoundedRect>) {
    match rect {
        Some(rect) => {
            out.push(1);
            put_rect(out, rect);
        }
        None => out.push(0),
    }
}

type BoxRect = (i32, i32, u32, u32);

fn put_box(out: &mut Vec<u8>, rect: BoxRect) {
    put_i32(out, rect.0);
    put_i32(out, rect.1);
    put_u32(out, rect.2);
    put_u32(out, rect.3);
}

fn encode_request_into(
    out: &mut Vec<u8>,
    width: u32,
    height: u32,
    frame: &[u8],
    ops: &[PixelOp<'_>],
) -> Result<(), String> {
    put_u32(out, width);
    put_u32(out, height);
    put_bytes(out, frame)?;
    put_u32(
        out,
        u32::try_from(ops.len()).map_err(|_| "too many helper ops".to_owned())?,
    );
    for op in ops {
        match *op {
            PixelOp::FillRoundedRect { outer, hole, color } => {
                out.push(TAG_FILL);
                put_rect(out, outer);
                put_opt_rect(out, hole);
                out.extend_from_slice(&color);
            }
            PixelOp::Blur { radius } => {
                out.push(TAG_BLUR);
                put_f32(out, radius);
            }
            PixelOp::BlendMask {
                x,
                y,
                width,
                height,
                mask,
                color,
            } => {
                out.push(TAG_MASK);
                put_i32(out, x);
                put_i32(out, y);
                put_u32(out, width);
                put_u32(out, height);
                put_bytes(out, mask)?;
                out.extend_from_slice(&color);
            }
            PixelOp::BlendImage {
                x,
                y,
                width,
                height,
                rgba,
            } => {
                out.push(TAG_IMAGE);
                put_i32(out, x);
                put_i32(out, y);
                put_u32(out, width);
                put_u32(out, height);
                put_bytes(out, rgba)?;
            }
            PixelOp::BackdropBlur {
                clip,
                node,
                corner,
                radius,
            } => {
                out.push(TAG_BACKDROP);
                put_box(out, clip);
                put_box(out, node);
                put_u32(out, corner);
                put_f32(out, radius);
            }
            PixelOp::FillRoundedRectClipped {
                outer,
                hole,
                color,
                clip,
            } => {
                out.push(TAG_FILL_CLIPPED);
                put_rect(out, outer);
                put_opt_rect(out, hole);
                out.extend_from_slice(&color);
                put_box(out, clip);
            }
        }
    }
    Ok(())
}

/// Test-only allocating wrapper around [`encode_request_into`].
#[cfg(test)]
fn encode_request(
    width: u32,
    height: u32,
    frame: &[u8],
    ops: &[PixelOp<'_>],
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(frame.len() + 64);
    encode_request_into(&mut out, width, height, frame, ops)?;
    Ok(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        if self.bytes.len() < count {
            return Err("truncated helper request".to_owned());
        }
        let (head, tail) = self.bytes.split_at(count);
        self.bytes = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "truncated helper request".to_owned())
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let count = self.u32()? as usize;
        self.take(count)
    }

    fn rect(&mut self) -> Result<RoundedRect, String> {
        Ok(RoundedRect {
            x: self.i32()?,
            y: self.i32()?,
            width: self.u32()?,
            height: self.u32()?,
            radius: self.u32()?,
        })
    }

    fn opt_rect(&mut self) -> Result<Option<RoundedRect>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.rect()?)),
            _ => Err("invalid helper hole flag".to_owned()),
        }
    }

    fn box_rect(&mut self) -> Result<BoxRect, String> {
        Ok((self.i32()?, self.i32()?, self.u32()?, self.u32()?))
    }

    fn op(&mut self) -> Result<PixelOp<'a>, String> {
        Ok(match self.u8()? {
            TAG_FILL => PixelOp::FillRoundedRect {
                outer: self.rect()?,
                hole: self.opt_rect()?,
                color: self.array()?,
            },
            TAG_BLUR => PixelOp::Blur {
                radius: self.f32()?,
            },
            TAG_MASK => PixelOp::BlendMask {
                x: self.i32()?,
                y: self.i32()?,
                width: self.u32()?,
                height: self.u32()?,
                mask: self.bytes()?,
                color: self.array()?,
            },
            TAG_IMAGE => PixelOp::BlendImage {
                x: self.i32()?,
                y: self.i32()?,
                width: self.u32()?,
                height: self.u32()?,
                rgba: self.bytes()?,
            },
            TAG_BACKDROP => PixelOp::BackdropBlur {
                clip: self.box_rect()?,
                node: self.box_rect()?,
                corner: self.u32()?,
                radius: self.f32()?,
            },
            TAG_FILL_CLIPPED => PixelOp::FillRoundedRectClipped {
                outer: self.rect()?,
                hole: self.opt_rect()?,
                color: self.array()?,
                clip: self.box_rect()?,
            },
            _ => return Err("unknown helper op".to_owned()),
        })
    }
}

struct Request<'a> {
    width: u32,
    height: u32,
    frame: &'a [u8],
    ops: Vec<PixelOp<'a>>,
}

fn decode_request(body: &[u8]) -> Result<Request<'_>, String> {
    let mut reader = Reader { bytes: body };
    let width = reader.u32()?;
    let height = reader.u32()?;
    let frame = reader.bytes()?;
    let count = reader.u32()?;
    let mut ops = Vec::new();
    for _ in 0..count {
        ops.push(reader.op()?);
    }
    Ok(Request {
        width,
        height,
        frame,
        ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_every_op_variant() {
        let frame = vec![7_u8; 16];
        let mask = vec![9_u8; 4];
        let rgba = vec![1_u8; 16];
        let rect = RoundedRect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
            radius: 5,
        };
        let ops = [
            PixelOp::FillRoundedRect {
                outer: rect,
                hole: Some(rect),
                color: [1, 2, 3, 4],
            },
            PixelOp::Blur { radius: 2.5 },
            PixelOp::BlendMask {
                x: -1,
                y: 2,
                width: 2,
                height: 2,
                mask: &mask,
                color: [9, 8, 7, 6],
            },
            PixelOp::BlendImage {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
                rgba: &rgba,
            },
            PixelOp::BackdropBlur {
                clip: (0, 0, 4, 4),
                node: (1, 1, 2, 2),
                corner: 3,
                radius: 4.0,
            },
            PixelOp::FillRoundedRectClipped {
                outer: rect,
                hole: None,
                color: [5, 5, 5, 5],
                clip: (1, 1, 2, 2),
            },
        ];
        let body = encode_request(2, 2, &frame, &ops).unwrap();
        let request = decode_request(&body).unwrap();
        assert_eq!((request.width, request.height), (2, 2));
        assert_eq!(request.frame, &frame[..]);
        assert_eq!(format!("{:?}", request.ops), format!("{ops:?}"));
    }

    #[test]
    fn truncated_request_is_an_error() {
        let body = encode_request(1, 1, &[0; 4], &[PixelOp::Blur { radius: 1.0 }]).unwrap();
        assert!(decode_request(&body[..body.len() - 1]).is_err());
    }

    #[test]
    fn replies_carry_payload_or_error_text() {
        assert_eq!(parse_reply(&reply_ok(b"ok")).unwrap(), b"ok");
        assert_eq!(parse_reply(&reply_error("boom")), Err("boom".to_owned()));
    }

    #[test]
    fn messages_round_trip_through_the_length_prefix() {
        let mut wire = Vec::new();
        write_message(&mut wire, b"hello").unwrap();
        let mut body = Vec::new();
        read_message(&mut wire.as_slice(), &mut body).unwrap();
        assert_eq!(body, b"hello");
        // The retained buffer is reused, not regrown, for a message of the same size.
        let capacity = body.capacity();
        read_message(&mut wire.as_slice(), &mut body).unwrap();
        assert_eq!(body, b"hello");
        assert_eq!(body.capacity(), capacity);
    }
}
