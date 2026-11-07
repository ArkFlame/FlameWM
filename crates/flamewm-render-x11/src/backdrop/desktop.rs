use super::*;

/// Root property on which the desktop surface publishes its presented frame for in-process readers:
/// `CARDINAL[6]` = backbuffer pixmap, window root `x`, `y` (two's complement), `width`, `height`,
/// present serial. The pixmap holds what the desktop shows, so readers sample it without reading
/// the root. It is rewritten after every present: the serial change tells the WM
/// that a read racing that repaint must be redone. Deleted when the desktop surface goes away.
pub const DESKTOP_FRAME_PROPERTY: &str = "_FLAMEWM_DESKTOP_FRAME";

/// Root property on which the WM lists the frames it redirects automatically through Composite
/// (`WINDOW[]`, bottom to top). Their contents stay valid off screen even where other windows
/// cover them, so translucent app windows compose what lies behind them from these pixmaps.
/// Absent when the WM does not redirect (no Composite, or a compositing manager runs).
pub const REDIRECTED_FRAMES_PROPERTY: &str = "_FLAMEWM_REDIRECTED_FRAMES";

/// A desktop frame read back from [`DESKTOP_FRAME_PROPERTY`]: the pixmap and its window's root
/// origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DesktopFrame {
    pub(crate) pixmap: Pixmap,
    pub(crate) x: i32,
    pub(crate) y: i32,
}

/// Parses the [`DESKTOP_FRAME_PROPERTY`] words; `None` when malformed.
pub(super) fn desktop_frame_from_words(words: &[u32]) -> Option<DesktopFrame> {
    let [pixmap, x, y, width, height, _serial] = *words else {
        return None;
    };
    (pixmap != 0 && width > 0 && height > 0).then_some(DesktopFrame {
        pixmap: Pixmap::from(pixmap),
        x: x as i32,
        y: y as i32,
    })
}

/// The desktop frame currently published on the root of `display`; `None` without a desktop.
///
/// # Safety
///
/// `display` must be live.
pub(crate) unsafe fn published_desktop_frame(display: *mut Display) -> Option<DesktopFrame> {
    // SAFETY: live display per the contract.
    desktop_frame_from_words(&unsafe { root_cardinals(display, DESKTOP_FRAME_PROPERTY, 6) })
}

/// Root property on which the WM publishes the wallpaper layout the desktop draws:
/// `CARDINAL[4n]` = `x`, `y` (two's complement), `width`, `height` per output in root coordinates.
/// Empty or absent means one wallpaper over the whole root (a single output or `shareWallpaper`).
pub const WALLPAPER_OUTPUTS_PROPERTY: &str = "_FLAMEWM_WALLPAPER_OUTPUTS";

/// Most outputs read from [`WALLPAPER_OUTPUTS_PROPERTY`].
pub(super) const WALLPAPER_OUTPUTS_MAX: usize = 16;

/// Parses [`WALLPAPER_OUTPUTS_PROPERTY`] words; empty rects and a trailing partial rect are
/// dropped.
pub(super) fn wallpaper_outputs_from_words(words: &[u32]) -> Vec<(i32, i32, u32, u32)> {
    words
        .chunks_exact(4)
        .filter(|rect| rect[2] > 0 && rect[3] > 0)
        .map(|rect| (rect[0] as i32, rect[1] as i32, rect[2], rect[3]))
        .collect()
}

/// The wallpaper layout the WM published on the root of `display`; empty means one wallpaper
/// over the root.
///
/// # Safety
///
/// `display` must be live.
pub(crate) unsafe fn published_wallpaper_outputs(
    display: *mut Display,
) -> Vec<(i32, i32, u32, u32)> {
    // SAFETY: live display per the contract.
    wallpaper_outputs_from_words(&unsafe {
        root_cardinals(
            display,
            WALLPAPER_OUTPUTS_PROPERTY,
            WALLPAPER_OUTPUTS_MAX * 4,
        )
    })
}

/// Up to `max_words` format-32 `CARDINAL` words of the root property `name`; empty when absent.
///
/// # Safety
///
/// `display` must be live.
pub(super) unsafe fn root_cardinals(
    display: *mut Display,
    name: &str,
    max_words: usize,
) -> Vec<u32> {
    let Ok(name) = std::ffi::CString::new(name) else {
        return Vec::new();
    };
    // SAFETY: live display per the contract; `only_if_exists` keeps the lookup from creating it.
    let atom = unsafe { XInternAtom(display, name.as_ptr(), 1) };
    if atom == 0 {
        return Vec::new();
    }
    let root = unsafe { XRootWindow(display, XDefaultScreen(display)) };
    let (mut actual_type, mut actual_format, mut items, mut after) = (0, 0, 0, 0);
    let mut data: *mut u8 = std::ptr::null_mut();
    // SAFETY: all out-pointers reference live locals; Xlib allocates `data`, freed below.
    let status = unsafe {
        crate::xlib_ext::XGetWindowProperty(
            display,
            root,
            atom,
            0,
            max_words as _,
            0,
            XA_CARDINAL,
            &mut actual_type,
            &mut actual_format,
            &mut items,
            &mut after,
            &mut data,
        )
    };
    if status != 0 || data.is_null() {
        return Vec::new();
    }
    let words: Vec<u32> = if actual_format == 32 {
        // SAFETY: format-32 properties come back as `items` C longs at `data`.
        unsafe {
            std::slice::from_raw_parts(data.cast::<std::os::raw::c_long>(), items as usize)
                .iter()
                .map(|&word| word as u32)
                .collect()
        }
    } else {
        Vec::new()
    };
    unsafe { XFree(data.cast()) };
    words
}

/// What the desktop layer is made of, as the desktop draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesktopSource {
    Solid([u8; 3]),
    Image {
        path: std::path::PathBuf,
        fit: flamewm_image_core::wallpaper::WallpaperFit,
    },
}

/// Flat colour of the layer when the wallpaper cannot be decoded (the desktop's solid default).
pub(super) const DESKTOP_FALLBACK_RGB: [u8; 3] = [0x23, 0x26, 0x29];

/// One process's handle on the desktop layer that popups and panels blend over. Blurs that the
/// canvas stores downscaled share [`desktop_canvas`] (one small canvas, cached on disk); smaller
/// blurs and unblurred crops are composed per request from the decoded wallpaper and never kept,
/// so no root-sized full-resolution copy stays resident.
#[derive(Debug)]
pub struct DesktopLayer {
    source: DesktopSource,
    canvas: Option<(CanvasLayout, Option<BlurredCanvas>)>,
}

/// Root size, output layout and blur a [`DesktopLayer`] canvas was built for.
pub(super) type CanvasLayout = ((u32, u32), Vec<(i32, i32, u32, u32)>, u8);

impl DesktopLayer {
    #[must_use]
    pub fn new(source: DesktopSource) -> Self {
        Self {
            source,
            canvas: None,
        }
    }

    #[must_use]
    pub fn source(&self) -> &DesktopSource {
        &self.source
    }

    /// Opaque ARGB32 words of the layer under the root-space `rect`, blurred as a surface at
    /// `opacity_percent` and `blur_percent` would be, with the wallpaper laid out over `outputs`
    /// like the desktop draws it. A solid or undecodable wallpaper is flat.
    pub fn crop(
        &mut self,
        root: (u32, u32),
        outputs: &[(i32, i32, u32, u32)],
        rect: (i32, i32, u32, u32),
        opacity_percent: u8,
        blur_percent: u8,
    ) -> Vec<u32> {
        let (x, y, width, height) = rect;
        let flat = |rgb: [u8; 3]| {
            let word =
                0xff00_0000 | u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
            vec![word; width as usize * height as usize]
        };
        let (path, fit) = match &self.source {
            DesktopSource::Solid(rgb) => return flat(*rgb),
            DesktopSource::Image { path, fit } => (path, *fit),
        };
        let radius = blur_radius_px(opacity_percent, blur_percent);
        if radius < 2.0 * BLUR_DOWNSCALE as f32 {
            return flamewm_image_core::wallpaper::decode_file(path)
                .ok()
                .and_then(|natural| {
                    flamewm_image_core::wallpaper::compose_outputs_region(
                        &natural, root, outputs, fit, rect,
                    )
                })
                .map_or_else(
                    || flat(DESKTOP_FALLBACK_RGB),
                    |region| {
                        blurred_argb_words(
                            width,
                            height,
                            region.pixels,
                            opacity_percent,
                            blur_percent,
                        )
                    },
                );
        }
        if self
            .canvas
            .as_ref()
            .is_none_or(|((size, layout, blur), _)| {
                (*size, layout.as_slice(), *blur) != (root, outputs, blur_percent)
            })
        {
            let canvas = desktop_canvas(path, fit, outputs, root, blur_percent)
                .map_err(|error| eprintln!("flamewm-render-x11: desktop layer: {error}"))
                .ok();
            self.canvas = Some(((root, outputs.to_vec(), blur_percent), canvas));
        }
        match self.canvas.as_ref().and_then(|(_, canvas)| canvas.as_ref()) {
            Some(canvas) => canvas.crop((x, y), (width, height)),
            None => flat(DESKTOP_FALLBACK_RGB),
        }
    }
}

/// [`desktop_canvas`] for a window backdrop spec, laid out over `outputs`.
pub(crate) fn wallpaper_canvas(
    spec: &WindowBackdrop,
    root: (u32, u32),
    outputs: &[(i32, i32, u32, u32)],
) -> Result<BlurredCanvas, String> {
    desktop_canvas(
        &wallpaper_file(&spec.wallpaper, &spec.packaged_root),
        flamewm_image_core::wallpaper::parse(&spec.fit),
        outputs,
        root,
        spec.blur_percent,
    )
}
