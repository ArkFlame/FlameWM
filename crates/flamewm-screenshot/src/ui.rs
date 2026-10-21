//! On-demand native X11 selection/editor. The frozen backing pixmap avoids
//! screen rereads, repaint loops, compositing, and capture-of-overlay artifacts.

use crate::capture::{Image, Rect, Result, decode_ximage_owned};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    ChangeGCAux, ConnectionExt as _, CoordMode, CreateGCAux, CreateWindowAux, EventMask, GrabMode,
    GrabStatus, ImageFormat, Point, Rectangle, WindowClass,
};
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn pack_color(v: u8, mask: u32) -> u32 {
    if mask == 0 {
        return 0;
    }
    let max = mask >> mask.trailing_zeros();
    (((u32::from(v) * max + 127) / 255) << mask.trailing_zeros()) & mask
}

fn native_color<C: Connection>(conn: &C, screen: usize, rgb: u32) -> Result<u32> {
    let root = &conn.setup().roots[screen];
    let visual = root
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == root.root_visual)
        .ok_or("root visual unavailable")?;
    Ok(pack_color(((rgb >> 16) & 255) as u8, visual.red_mask)
        | pack_color(((rgb >> 8) & 255) as u8, visual.green_mask)
        | pack_color((rgb & 255) as u8, visual.blue_mask))
}

/// Packs one bounded batch of screenshot rows using the server's native pixel format.
/// The caller owns and reuses the output buffer; padding must remain zero.
fn pack_rows(
    image: &Image,
    buffer: &mut [u8],
    start_row: usize,
    rows: usize,
    stride: usize,
    bpp: usize,
    masks: [u32; 3],
    little: bool,
) -> Result<()> {
    let source_stride = (image.width as usize)
        .checked_mul(4)
        .ok_or("image row overflow")?;
    let start = start_row
        .checked_mul(source_stride)
        .ok_or("image offset overflow")?;
    let end = (start_row + rows)
        .checked_mul(source_stride)
        .ok_or("image offset overflow")?;
    let source = image
        .rgba
        .get(start..end)
        .ok_or("short screenshot RGBA buffer")?;
    if buffer.len() != stride.checked_mul(rows).ok_or("image chunk overflow")? {
        return Err("invalid screenshot chunk size".into());
    }
    buffer.fill(0);
    let pixel_bytes = bpp / 8;
    for (row, pixels) in source.chunks_exact(source_stride).enumerate() {
        for (column, pixel) in pixels.chunks_exact(4).enumerate() {
            let value = pack_color(pixel[0], masks[0])
                | pack_color(pixel[1], masks[1])
                | pack_color(pixel[2], masks[2]);
            let offset = row * stride + column * pixel_bytes;
            for byte in 0..pixel_bytes {
                let shift = if little {
                    byte * 8
                } else {
                    (pixel_bytes - 1 - byte) * 8
                };
                buffer[offset + byte] = (value >> shift) as u8;
            }
        }
    }
    Ok(())
}

/// Sends a captured image in native X11 format without retaining a second full-screen copy.
/// With ordinary visuals, the staging buffer is bounded to approximately 64 KiB.
fn put_image<C: Connection>(
    conn: &C,
    screen: usize,
    pixmap: u32,
    gc: u32,
    image: &Image,
) -> Result<()> {
    let root = &conn.setup().roots[screen];
    let visual = root
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == root.root_visual)
        .ok_or("root visual unavailable")?;
    let format = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|p| p.depth == root.root_depth)
        .ok_or("pixmap format unavailable")?;
    let bpp = usize::from(format.bits_per_pixel);
    if !matches!(bpp, 16 | 24 | 32) {
        return Err("unsupported root pixmap depth".into());
    }
    let pad = usize::from(format.scanline_pad);
    if pad == 0 || pad % 8 != 0 {
        return Err("invalid root pixmap scanline pad".into());
    }
    let width = usize::try_from(image.width).map_err(err)?;
    let stride = width
        .checked_mul(bpp)
        .and_then(|bits| bits.checked_add(pad - 1))
        .map(|bits| bits / pad * (pad / 8))
        .ok_or("image row overflow")?;
    let max_request = conn.maximum_request_bytes().saturating_sub(128);
    if stride == 0 || stride > max_request {
        return Err("X11 maximum request too small for scanline".into());
    }
    let rows_per_request = (max_request.min(64 * 1024).max(stride) / stride).min(u16::MAX as usize);
    let mut buffer = vec![
        0u8;
        stride
            .checked_mul(rows_per_request)
            .ok_or("image chunk overflow")?
    ];
    let little = conn.setup().image_byte_order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST;
    let masks = [visual.red_mask, visual.green_mask, visual.blue_mask];
    for start in (0..image.height as usize).step_by(rows_per_request) {
        let rows = rows_per_request.min(image.height as usize - start);
        let bytes = rows * stride;
        pack_rows(
            image,
            &mut buffer[..bytes],
            start,
            rows,
            stride,
            bpp,
            masks,
            little,
        )?;
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            pixmap,
            gc,
            u16::try_from(image.width).map_err(|_| "image too wide")?,
            rows as u16,
            0,
            i16::try_from(start).map_err(|_| "image too tall")?,
            0,
            root.root_depth,
            &buffer[..bytes],
        )
        .map_err(err)?;
    }
    Ok(())
}

struct Overlay<'a, C: Connection> {
    conn: &'a C,
    win: u32,
    original: u32,
    work: u32,
    gc: u32,
    width: u16,
    height: u16,
    screen: usize,
}

impl<'a, C: Connection> Overlay<'a, C> {
    fn new(conn: &'a C, screen: usize, image: &Image, center: bool) -> Result<Self> {
        let root = &conn.setup().roots[screen];
        let width = u16::try_from(image.width).map_err(|_| "image too wide")?;
        let height = u16::try_from(image.height).map_err(|_| "image too tall")?;
        let original = conn.generate_id().map_err(err)?;
        let work = conn.generate_id().map_err(err)?;
        let gc = conn.generate_id().map_err(err)?;
        let win = conn.generate_id().map_err(err)?;
        conn.create_pixmap(root.root_depth, original, root.root, width, height)
            .map_err(err)?;
        conn.create_pixmap(root.root_depth, work, root.root, width, height)
            .map_err(err)?;
        conn.create_gc(
            gc,
            original,
            &CreateGCAux::new()
                .foreground(root.white_pixel)
                .graphics_exposures(0),
        )
        .map_err(err)?;
        put_image(conn, screen, original, gc, image)?;
        conn.copy_area(original, work, gc, 0, 0, 0, 0, width, height)
            .map_err(err)?;
        let x = if center {
            ((i32::from(root.width_in_pixels) - i32::from(width)) / 2).max(0) as i16
        } else {
            0
        };
        let y = if center {
            ((i32::from(root.height_in_pixels) - i32::from(height)) / 2).max(0) as i16
        } else {
            0
        };
        conn.create_window(
            COPY_DEPTH_FROM_PARENT,
            win,
            root.root,
            x,
            y,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            root.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixmap(work)
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::POINTER_MOTION
                        | EventMask::KEY_PRESS,
                ),
        )
        .map_err(err)?;
        conn.map_window(win).map_err(err)?;
        conn.configure_window(
            win,
            &x11rb::protocol::xproto::ConfigureWindowAux::new()
                .stack_mode(x11rb::protocol::xproto::StackMode::ABOVE),
        )
        .map_err(err)?;
        conn.flush().map_err(err)?;
        let overlay = Self {
            conn,
            win,
            original,
            work,
            gc,
            width,
            height,
            screen,
        };
        let pointer = conn
            .grab_pointer(
                false,
                win,
                EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                win,
                x11rb::NONE,
                CURRENT_TIME,
            )
            .map_err(err)?
            .reply()
            .map_err(err)?;
        if pointer.status != GrabStatus::SUCCESS {
            return Err(format!(
                "cannot select region: pointer grab {:?}",
                pointer.status
            ));
        }
        // A Print shortcut can launch us before the triggering key is released.
        // Retry rather than entering a modal UI with an ungrabbable Escape key.
        for _ in 0..50 {
            let keyboard = conn
                .grab_keyboard(false, win, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(err)?
                .reply()
                .map_err(err)?;
            if keyboard.status == GrabStatus::SUCCESS {
                return Ok(overlay);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Err("cannot select region: X11 keyboard grab unavailable".into())
    }
    fn repaint(&self) -> Result<()> {
        self.conn
            .copy_area(
                self.work,
                self.win,
                self.gc,
                0,
                0,
                0,
                0,
                self.width,
                self.height,
            )
            .map_err(err)?;
        Ok(())
    }
    fn rgba(&self) -> Result<Image> {
        let result = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.work,
                0,
                0,
                self.width,
                self.height,
                u32::MAX,
            )
            .map_err(err)?
            .reply()
            .map_err(err)?;
        decode_ximage_owned(
            self.conn,
            self.screen,
            result.depth,
            u32::from(self.width),
            u32::from(self.height),
            result.data,
        )
    }
}

impl<C: Connection> Drop for Overlay<'_, C> {
    fn drop(&mut self) {
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        let _ = self.conn.destroy_window(self.win);
        let _ = self.conn.free_gc(self.gc);
        let _ = self.conn.free_pixmap(self.work);
        let _ = self.conn.free_pixmap(self.original);
        let _ = self.conn.flush();
    }
}

fn sym<C: Connection>(conn: &C, keycode: u8) -> u32 {
    conn.get_keyboard_mapping(keycode, 1)
        .ok()
        .and_then(|r| r.reply().ok())
        .and_then(|r| r.keysyms.first().copied())
        .unwrap_or(0)
}

/// Returns None for Escape/cancel; a click-without-drag is also cancellation.
pub fn select_region<C: Connection>(conn: &C, screen: usize, full: &Image) -> Result<Option<Rect>> {
    let o = Overlay::new(conn, screen, full, false)?;
    let border_gc = conn.generate_id().map_err(err)?;
    // XOR draws a readable temporary selection border without resending a full screen image.
    conn.create_gc(
        border_gc,
        o.win,
        &CreateGCAux::new()
            .foreground(0x00ffffff)
            .function(x11rb::protocol::xproto::GX::XOR)
            .subwindow_mode(x11rb::protocol::xproto::SubwindowMode::INCLUDE_INFERIORS),
    )
    .map_err(err)?;
    let mut start: Option<(i32, i32)> = None;
    let mut last: Option<Rect> = None;
    let result = loop {
        match conn.wait_for_event().map_err(err)? {
            Event::ButtonPress(e) if e.event == o.win && e.detail == 1 => {
                start = Some((i32::from(e.event_x), i32::from(e.event_y)));
            }
            Event::MotionNotify(e) if e.event == o.win => {
                if let Some((sx, sy)) = start {
                    if let Some(r) = last.take() {
                        conn.poly_rectangle(
                            o.win,
                            border_gc,
                            &[Rectangle {
                                x: r.x as i16,
                                y: r.y as i16,
                                width: r.w as u16,
                                height: r.h as u16,
                            }],
                        )
                        .map_err(err)?;
                    }
                    let r = Rect::from_points(sx, sy, i32::from(e.event_x), i32::from(e.event_y));
                    conn.poly_rectangle(
                        o.win,
                        border_gc,
                        &[Rectangle {
                            x: r.x as i16,
                            y: r.y as i16,
                            width: r.w as u16,
                            height: r.h as u16,
                        }],
                    )
                    .map_err(err)?;
                    last = Some(r);
                    conn.flush().map_err(err)?;
                }
            }
            Event::ButtonRelease(e) if e.event == o.win && e.detail == 1 => {
                break start.and_then(|(x, y)| {
                    let r = Rect::from_points(x, y, i32::from(e.event_x), i32::from(e.event_y));
                    (r.w > 2 && r.h > 2).then_some(r)
                });
            }
            Event::KeyPress(e) if sym(conn, e.detail) == 0xff1b => break None,
            Event::Expose(e) if e.window == o.win && e.count == 0 => {
                o.repaint()?;
                if let Some(r) = last {
                    conn.poly_rectangle(
                        o.win,
                        border_gc,
                        &[Rectangle {
                            x: r.x as i16,
                            y: r.y as i16,
                            width: r.w as u16,
                            height: r.h as u16,
                        }],
                    )
                    .map_err(err)?;
                }
                conn.flush().map_err(err)?;
            }
            _ => {}
        }
    };
    let _ = conn.free_gc(border_gc);
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Pen,
    Line,
    Rect,
    Ellipse,
    Arrow,
    Blackout,
}
#[derive(Clone)]
struct Stroke {
    tool: Tool,
    color: u32,
    points: Vec<Point>,
}

fn draw<C: Connection>(conn: &C, dst: u32, gc: u32, s: &Stroke) -> Result<()> {
    let Some(first) = s.points.first() else {
        return Ok(());
    };
    let last = s.points.last().unwrap_or(first);
    conn.change_gc(
        gc,
        &ChangeGCAux::new()
            .foreground(if s.tool == Tool::Blackout { 0 } else { s.color })
            .line_width(if s.tool == Tool::Blackout { 12 } else { 3 }),
    )
    .map_err(err)?;
    match s.tool {
        Tool::Pen | Tool::Blackout => {
            if s.points.len() == 1 {
                conn.poly_point(CoordMode::ORIGIN, dst, gc, &s.points)
                    .map_err(err)?;
            } else {
                conn.poly_line(CoordMode::ORIGIN, dst, gc, &s.points)
                    .map_err(err)?;
            }
        }
        Tool::Line | Tool::Arrow => {
            conn.poly_line(CoordMode::ORIGIN, dst, gc, &[*first, *last])
                .map_err(err)?;
            if s.tool == Tool::Arrow {
                let dx = f64::from(last.x - first.x);
                let dy = f64::from(last.y - first.y);
                let angle = dy.atan2(dx);
                let a = Point {
                    x: last.x - (12.0 * (angle - 0.55).cos()) as i16,
                    y: last.y - (12.0 * (angle - 0.55).sin()) as i16,
                };
                let b = Point {
                    x: last.x - (12.0 * (angle + 0.55).cos()) as i16,
                    y: last.y - (12.0 * (angle + 0.55).sin()) as i16,
                };
                conn.poly_line(CoordMode::ORIGIN, dst, gc, &[a, *last, b])
                    .map_err(err)?;
            }
        }
        Tool::Rect => {
            let r = Rect::from_points(
                i32::from(first.x),
                i32::from(first.y),
                i32::from(last.x),
                i32::from(last.y),
            );
            conn.poly_rectangle(
                dst,
                gc,
                &[Rectangle {
                    x: r.x as i16,
                    y: r.y as i16,
                    width: r.w as u16,
                    height: r.h as u16,
                }],
            )
            .map_err(err)?;
        }
        Tool::Ellipse => {
            let cx = (f64::from(first.x) + f64::from(last.x)) / 2.0;
            let cy = (f64::from(first.y) + f64::from(last.y)) / 2.0;
            let rx = (f64::from(first.x) - f64::from(last.x)).abs() / 2.0;
            let ry = (f64::from(first.y) - f64::from(last.y)).abs() / 2.0;
            let mut pts = Vec::with_capacity(49);
            for i in 0..=48 {
                let angle = (i as f64) * std::f64::consts::TAU / 48.0;
                pts.push(Point {
                    x: (cx + rx * angle.cos()) as i16,
                    y: (cy + ry * angle.sin()) as i16,
                });
            }
            conn.poly_line(CoordMode::ORIGIN, dst, gc, &pts)
                .map_err(err)?;
        }
    }
    Ok(())
}

fn draw_bar<C: Connection>(conn: &C, window: u32, gc: u32, tool: Tool, color: u32) -> Result<()> {
    conn.change_gc(gc, &ChangeGCAux::new().foreground(0x202126).line_width(1))
        .map_err(err)?;
    conn.poly_fill_rectangle(
        window,
        gc,
        &[Rectangle {
            x: 0,
            y: 0,
            width: 820,
            height: 34,
        }],
    )
    .map_err(err)?;
    conn.change_gc(gc, &ChangeGCAux::new().foreground(0xffffff))
        .map_err(err)?;
    let text = format!(
        "FlameWM   P pen  L line  R rect  E oval  A arrow  X redact  C color  U undo  Enter save  Esc cancel  [{tool:?}]"
    );
    let bytes = text.as_bytes();
    conn.image_text8(window, gc, 8, 21, &bytes[..bytes.len().min(254)])
        .map_err(err)?;
    conn.change_gc(gc, &ChangeGCAux::new().foreground(color))
        .map_err(err)?;
    conn.poly_fill_rectangle(
        window,
        gc,
        &[Rectangle {
            x: 2,
            y: 27,
            width: 80,
            height: 3,
        }],
    )
    .map_err(err)?;
    Ok(())
}

/// Edit directly on the server pixmap; the final PNG reads that same pixmap.
/// Arrow/rectangle/ellipse/pen/redaction all use one retained stroke history.
pub fn annotate<C: Connection>(conn: &C, screen: usize, image: Image) -> Result<Option<Image>> {
    let o = Overlay::new(conn, screen, &image, true)?;
    let mut strokes: Vec<Stroke> = Vec::new();
    let mut current: Option<Stroke> = None;
    let mut tool = Tool::Pen;
    let colors = [0xef4048, 0xffffff, 0xffd166, 0x2d9cdb, 0x43c87a]
        .map(|rgb| native_color(conn, screen, rgb))
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    let mut color_index = 0usize;
    o.repaint()?;
    draw_bar(conn, o.win, o.gc, tool, colors[color_index])?;
    conn.flush().map_err(err)?;
    loop {
        match conn.wait_for_event().map_err(err)? {
            Event::Expose(e) if e.window == o.win && e.count == 0 => {
                o.repaint()?;
                draw_bar(conn, o.win, o.gc, tool, colors[color_index])?;
            }
            Event::KeyPress(e) => {
                match sym(conn, e.detail) {
                    0xff1b => return Ok(None),
                    0xff0d | 0xff8d => return o.rgba().map(Some),
                    0x70 => tool = Tool::Pen,
                    0x6c => tool = Tool::Line,
                    0x72 => tool = Tool::Rect,
                    0x65 => tool = Tool::Ellipse,
                    0x61 => tool = Tool::Arrow,
                    0x78 => tool = Tool::Blackout,
                    0x63 => color_index = (color_index + 1) % colors.len(),
                    0x75 | 0xff08 => {
                        strokes.pop();
                        conn.copy_area(o.original, o.work, o.gc, 0, 0, 0, 0, o.width, o.height)
                            .map_err(err)?;
                        for s in &strokes {
                            draw(conn, o.work, o.gc, s)?;
                        }
                    }
                    _ => {}
                }
                o.repaint()?;
                draw_bar(conn, o.win, o.gc, tool, colors[color_index])?;
            }
            Event::ButtonPress(e) if e.event == o.win && e.detail == 1 && e.event_y >= 36 => {
                current = Some(Stroke {
                    tool,
                    color: colors[color_index],
                    points: vec![Point {
                        x: e.event_x,
                        y: e.event_y,
                    }],
                });
            }
            Event::MotionNotify(e) if e.event == o.win => {
                if let Some(s) = current.as_mut() {
                    let p = Point {
                        x: e.event_x.max(0),
                        y: e.event_y.max(36),
                    };
                    if s.tool == Tool::Pen || s.tool == Tool::Blackout {
                        s.points.push(p);
                    } else if s.points.len() < 2 {
                        s.points.push(p)
                    } else {
                        s.points[1] = p
                    }
                    o.repaint()?;
                    draw(conn, o.win, o.gc, s)?;
                    draw_bar(conn, o.win, o.gc, tool, colors[color_index])?;
                }
            }
            Event::ButtonRelease(e) if e.event == o.win && e.detail == 1 => {
                if let Some(mut s) = current.take() {
                    let p = Point {
                        x: e.event_x.max(0),
                        y: e.event_y.max(36),
                    };
                    if s.tool != Tool::Pen && s.tool != Tool::Blackout {
                        if s.points.len() < 2 {
                            s.points.push(p)
                        } else {
                            s.points[1] = p
                        }
                    } else {
                        s.points.push(p);
                    }
                    draw(conn, o.work, o.gc, &s)?;
                    strokes.push(s);
                    o.repaint()?;
                    draw_bar(conn, o.win, o.gc, tool, colors[color_index])?;
                }
            }
            _ => {}
        }
        conn.flush().map_err(err)?;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tools_are_stable_for_undo() {
        assert_eq!(super::Tool::Pen, super::Tool::Pen);
    }
}

#[cfg(test)]
mod memory_tests {
    use super::*;

    #[test]
    fn bounded_row_packing_preserves_pixels_endianness_and_padding() {
        let image = Image {
            width: 2,
            height: 3,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 18, 52, 86,
                255, 171, 205, 239, 255,
            ],
        };
        for little in [true, false] {
            let mut all = vec![0xaa; 12 * 3];
            pack_rows(
                &image,
                &mut all,
                0,
                3,
                12,
                32,
                [0x00ff0000, 0x0000ff00, 0x000000ff],
                little,
            )
            .unwrap();
            let mut batched = Vec::new();
            for row in 0..3 {
                let mut buffer = vec![0xff; 12];
                pack_rows(
                    &image,
                    &mut buffer,
                    row,
                    1,
                    12,
                    32,
                    [0x00ff0000, 0x0000ff00, 0x000000ff],
                    little,
                )
                .unwrap();
                batched.extend(buffer);
            }
            assert_eq!(all, batched);
            assert!(all.chunks_exact(12).all(|row| row[8..] == [0, 0, 0, 0]));
            let red = if little {
                [0, 0, 255, 0]
            } else {
                [0, 255, 0, 0]
            };
            assert_eq!(&all[..4], &red);
        }
    }

    #[test]
    fn small_rgb565_batches_round_trip_in_pixel_order() {
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 0, 255, 255],
        };
        let mut output = vec![0; 4];
        pack_rows(
            &image,
            &mut output,
            0,
            1,
            4,
            16,
            [0xf800, 0x07e0, 0x001f],
            true,
        )
        .unwrap();
        assert_eq!(output, [0x00, 0xf8, 0x1f, 0x00]);
    }
}
