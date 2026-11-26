//! Dock backdrop: the scene beneath a translucent strut surface (the Dock), blurred and cached.
//!
//! A translucent Dock blends over whatever lies beneath it, windows included. For each Dock the
//! compositor renders the scene elements below the Dock's rect into a Dock-sized target, reduces
//! and blurs that image, and places it beneath the Dock surface. The result is cached under a
//! [`BackdropKey`]. A recapture runs only when the rect, the material, the scale, the corner radius
//! or the scene stamp changes; the stamp is the physical rect and commit counter of each element
//! beneath the Dock that overlaps it, so a window moving, resizing, restacking or committing under
//! the Dock is a change and nothing else is. A static scene costs one element build and an
//! allocation-free stamp comparison per frame: no render, readback or blur. Opaque material
//! captures nothing.

use super::*;
use smithay::backend::renderer::element::utils::{Relocate, RelocateRenderElement};
use smithay::utils::Scale;
use std::hash::Hash;
use std::sync::atomic::{AtomicU16, Ordering};

/// Dock `opacity` and `blur` percents, as the shared settings resolve them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DockMaterial {
    pub(crate) opacity: u8,
    pub(crate) blur: u8,
}

impl DockMaterial {
    /// Opaque material hides whatever lies beneath, so it captures nothing. Same rule as the X11
    /// `backdrop_needed`: translucent or blurred material needs the scene.
    pub(crate) fn needs_backdrop(self) -> bool {
        self.opacity < 100 || self.blur > 0
    }
}

/// A rect as `(x, y, w, h)`, logical for a Dock site and physical inside a capture.
pub(super) type RectKey = (i32, i32, i32, i32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum BackdropSite {
    Dock(RectKey),
    Window(WindowId),
}

pub(super) fn window_needs_backdrop(
    visible: bool,
    covered: bool,
    material: DockMaterial,
    flat: bool,
) -> bool {
    visible && !covered && material.needs_backdrop() && !flat
}

/// Everything a cached Dock backdrop was built from. Equal keys mean the cached pixels are current.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BackdropKey {
    pub(super) site: BackdropSite,
    pub(super) rect: RectKey,
    /// Dock corner radius in device pixels, as published by the Dock.
    pub(super) radius: u32,
    pub(super) window_radii: [u8; 4],
    /// Output scale in thousandths, so a scale change recaptures at the new resolution.
    pub(super) scale_milli: u32,
    pub(super) material: DockMaterial,
    /// Scene generation: the physical rect and commit counter of each element beneath the Dock that
    /// overlaps it, front to back.
    pub(super) scene: Vec<(RectKey, CommitCounter)>,
}

/// A cached site backdrop: the key it was captured under and the rendered buffer.
pub(super) struct DockBacking {
    pub(super) key: BackdropKey,
    pub(super) buffer: MemoryRenderBuffer,
}

/// Dock material from the committed settings, packed as `opacity << 8 | blur`. The default is
/// opaque, so nothing is captured before the first settings sync.
static DOCK_MATERIAL: AtomicU16 = AtomicU16::new(100 << 8);

/// Records the Dock material. Returns whether it changed, so the caller requests a redraw.
pub(crate) fn set_dock_material(material: DockMaterial) -> bool {
    let packed = (u16::from(material.opacity) << 8) | u16::from(material.blur);
    DOCK_MATERIAL.swap(packed, Ordering::Relaxed) != packed
}

fn current_dock_material() -> DockMaterial {
    let packed = DOCK_MATERIAL.load(Ordering::Relaxed);
    DockMaterial {
        opacity: (packed >> 8) as u8,
        blur: (packed & 0xff) as u8,
    }
}

/// Refresh policy. A recapture is needed unless the cached backdrop was built under exactly the
/// key that is wanted now. Opaque material never reaches this check: the caller drops the cache.
pub(super) fn needs_capture(wanted: &BackdropKey, cached: Option<&BackdropKey>) -> bool {
    cached != Some(wanted)
}

/// Coverage (0..=255) of pixel `(x, y)` inside a `w` x `h` rounded rectangle of corner `radius`,
/// sampled at the pixel centre. The interior and the edges are fully covered; corners fall off.
pub(super) fn rounded_coverage(w: usize, h: usize, radius: f32, x: usize, y: usize) -> u8 {
    let (w, h) = (w as f32, h as f32);
    let r = radius.clamp(0.0, w.min(h) / 2.0);
    // Signed distance from the pixel centre to the rounded outline (negative inside), then one
    // pixel of antialiasing across the edge.
    let (px, py) = (x as f32 + 0.5 - w / 2.0, y as f32 + 0.5 - h / 2.0);
    let (qx, qy) = (px.abs() - (w / 2.0 - r), py.abs() - (h / 2.0 - r));
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let inside = qx.max(qy).min(0.0);
    let distance = outside + inside - r;
    ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// A mapped strut surface that may be a translucent Dock.
struct DockSite {
    index: usize,
    site: BackdropSite,
    rect: RectKey,
    radius: u32,
}

fn dock_sites(state: &State) -> Vec<DockSite> {
    state
        .unmanaged
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.below && entry.strut.is_some())
        .filter_map(|(index, entry)| {
            entry.surface.wl_surface()?;
            let geometry = entry.surface.geometry();
            Some(DockSite {
                index,
                site: BackdropSite::Dock((
                    geometry.loc.x,
                    geometry.loc.y,
                    geometry.size.w,
                    geometry.size.h,
                )),
                rect: (
                    geometry.loc.x,
                    geometry.loc.y,
                    geometry.size.w,
                    geometry.size.h,
                ),
                radius: entry.corner_radius,
            })
        })
        .collect()
}

fn rect_key(rect: Rectangle<i32, Physical>) -> RectKey {
    (rect.loc.x, rect.loc.y, rect.size.w, rect.size.h)
}

/// Physical pixels of a logical Dock rect at `scale`. The origin rounds the same way the element
/// location does, so the captured region lines up with where the Dock is drawn.
fn physical_rect(rect: RectKey, scale: f64) -> Rectangle<i32, Physical> {
    let origin = physical_point(rect.0, rect.1, scale);
    let size = Size::from((
        (f64::from(rect.2) * scale).round() as i32,
        (f64::from(rect.3) * scale).round() as i32,
    ));
    Rectangle::new(origin, size)
}

/// The scene beneath `site_index` that overlaps `region`, front to back. Built in capture mode, so
/// no Dock or menu backdrop is produced inside another backdrop's scene.
fn scene_beneath(
    state: &State,
    renderer: &mut GlowRenderer,
    decorations: &mut Decorations,
    site_index: usize,
    region: Rectangle<i32, Physical>,
    scale: f64,
) -> Vec<Elem> {
    let below = crate::scene::front_to_back(state)
        .into_iter()
        .skip(site_index + 1);
    let mut elements = Vec::new();
    for element in scene_elements(state, renderer, decorations, below, true) {
        if region.overlaps(element.geometry(Scale::from(scale))) {
            elements.push(element);
        }
    }
    elements
}

/// The stamp of `elements`: the physical rect and commit counter of each one overlapping `region`,
/// in the order they are drawn. Produced only when a capture is due.
fn scene_stamp(
    elements: &[Elem],
    region: Rectangle<i32, Physical>,
    scale: f64,
) -> Vec<(RectKey, CommitCounter)> {
    let mut stamp = Vec::new();
    for element in elements {
        let geometry = element.geometry(Scale::from(scale));
        if region.overlaps(geometry) {
            stamp.push((rect_key(geometry), element.current_commit()));
        }
    }
    stamp
}

/// Whether `wanted` is exactly the cached stamp, in order and length.
fn stamp_matches(
    wanted: impl Iterator<Item = (RectKey, CommitCounter)>,
    cached: &[(RectKey, CommitCounter)],
) -> bool {
    let mut seen = 0;
    for entry in wanted {
        if cached.get(seen) != Some(&entry) {
            return false;
        }
        seen += 1;
    }
    seen == cached.len()
}

/// Whether `elements` still carry exactly the cached scene stamp: same overlapping elements, same
/// rects, same commit counters. Compares in place, so a frame that changes nothing allocates no
/// stamp.
fn scene_matches(
    elements: &[Elem],
    region: Rectangle<i32, Physical>,
    scale: f64,
    cached: &[(RectKey, CommitCounter)],
) -> bool {
    stamp_matches(
        elements.iter().filter_map(|element| {
            let geometry = element.geometry(Scale::from(scale));
            region
                .overlaps(geometry)
                .then(|| (rect_key(geometry), element.current_commit()))
        }),
        cached,
    )
}

/// Renders the overlapping scene into a Dock-sized target, reduces and blurs it, and builds the
/// buffer beneath the Dock: opaque inside the Dock's rounded outline and transparent in its corners.
fn capture_dock(
    renderer: &mut GlowRenderer,
    decorations: &mut Decorations,
    elements: Vec<Elem>,
    key: BackdropKey,
    region: Rectangle<i32, Physical>,
    scale: f64,
) -> Result<DockBacking, Box<dyn Error>> {
    let dims = (region.size.w.max(1) as usize, region.size.h.max(1) as usize);
    let target_size = Size::<i32, Physical>::from((dims.0 as i32, dims.1 as i32));
    let mut texture: GlesTexture = match take_scratch(&mut decorations.dock_scratch, dims) {
        Some(texture) => texture,
        None => renderer.create_buffer(
            Fourcc::Abgr8888,
            Size::<i32, Buffer>::from((dims.0 as i32, dims.1 as i32)),
        )?,
    };
    // The scene is positioned in output space; shift it so the Dock region starts at the origin.
    let shifted: Vec<RelocateRenderElement<Elem>> = elements
        .into_iter()
        .map(|element| {
            RelocateRenderElement::from_element(
                element,
                (-region.loc.x, -region.loc.y),
                Relocate::Relative,
            )
        })
        .collect();
    {
        let mut target = renderer.bind(&mut texture)?;
        let mut tracker = OutputDamageTracker::new(target_size, 1.0, Transform::Normal);
        tracker.render_output(renderer, &mut target, 0, &shifted, CLEAR)?;
    }
    let mut clock = PhaseClock::start();
    let stored = read_reduced(renderer, &mut texture, dims, &mut clock)?;
    decorations.dock_scratch = Some((dims, texture));
    let blurred = blur_reduced(stored, stored_size(dims), key.material.blur);

    // The logical buffer is sampled from the blurred capture, then its alpha is the Dock's outline.
    let (w, h) = (key.rect.2.max(1) as usize, key.rect.3.max(1) as usize);
    let radius = key.radius as f32 / scale as f32;
    let mut pixels = crop_capture(&blurred, dims, (0, 0), w, h, scale);
    for y in 0..h {
        for x in 0..w {
            let alpha = if key.site == BackdropSite::Dock(key.rect) {
                rounded_coverage(w, h, radius, x, y)
            } else {
                client_mask_coverage(w, h, key.window_radii, x, y)
            };
            pixels[(y * w + x) * 4 + 3] = alpha;
        }
    }
    let buffer = MemoryRenderBuffer::from_slice(
        &premultiplied_argb_bytes(&pixels),
        Fourcc::Argb8888,
        (w as i32, h as i32),
        1,
        Transform::Normal,
        None,
    );
    Ok(DockBacking { key, buffer })
}

fn client_mask_coverage(w: usize, h: usize, radii: [u8; 4], x: usize, y: usize) -> u8 {
    let corner = if x < usize::from(radii[0]) && y < usize::from(radii[0]) {
        Some((0, radii[0]))
    } else if x + usize::from(radii[1]) >= w && y < usize::from(radii[1]) {
        Some((1, radii[1]))
    } else if x + usize::from(radii[2]) >= w && y + usize::from(radii[2]) >= h {
        Some((2, radii[2]))
    } else if x < usize::from(radii[3]) && y + usize::from(radii[3]) >= h {
        Some((3, radii[3]))
    } else {
        None
    };
    corner.map_or(255, |(index, radius)| {
        let r = f32::from(radius);
        let (cx, cy) = match index {
            0 => (r, r),
            1 => (w as f32 - r, r),
            2 => (w as f32 - r, h as f32 - r),
            _ => (r, h as f32 - r),
        };
        let dx = x as f32 + 0.5 - cx;
        let dy = y as f32 + 0.5 - cy;
        ((r + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

/// Corner radii and material a site is captured under: a Dock uses the shared Dock material, a
/// managed window its own published material.
fn site_parameters(
    state: &State,
    site: BackdropSite,
    dock: DockMaterial,
) -> ([u8; 4], DockMaterial) {
    match site {
        BackdropSite::Dock(_) => ([0; 4], dock),
        BackdropSite::Window(id) => state.model.get(id).map_or(
            (
                [0; 4],
                DockMaterial {
                    opacity: 100,
                    blur: 0,
                },
            ),
            |m| {
                (
                    client_corner_radii(m, m.titlebars),
                    DockMaterial {
                        opacity: m.material.0,
                        blur: m.material.1,
                    },
                )
            },
        ),
    }
}

/// Refreshes the cached backdrop of every translucent Dock for this frame. Called before the frame
/// elements are built, by the winit and DRM paths. While nothing beneath a Dock changes it costs
/// one element build and an allocation-free stamp comparison: no stamp vector, render, readback or
/// blur.
pub(crate) fn refresh_dock_backdrops(
    state: &State,
    renderer: &mut GlowRenderer,
) -> Result<(), Box<dyn Error>> {
    let material = current_dock_material();
    let mut decorations = state.decorations.borrow_mut();
    let mut sites = if material.needs_backdrop() && !glass_flat() {
        dock_sites(state)
    } else {
        Vec::new()
    };
    let scene_order = crate::scene::front_to_back(state);
    for site in &mut sites {
        if let Some(position) = scene_order.iter().position(
            |item| matches!(item, crate::scene::SceneItem::Unmanaged(index) if *index == site.index),
        ) {
            site.index = position;
        }
    }
    if !glass_flat() {
        let scale = state.output_scale();
        let visible = client_backdrop_rect(state.output_rect(), scale);
        for id in state.model.visible_top_first_iter() {
            let Some(managed) = state.model.get(id) else {
                continue;
            };
            let material = DockMaterial {
                opacity: managed.material.0,
                blur: managed.material.1,
            };
            if !window_needs_backdrop(
                client_backdrop_rect(managed.rect, scale).overlaps(visible),
                false,
                material,
                false,
            ) || state.window_wl_surface(id).is_none()
            {
                continue;
            }
            let Some(index) = scene_order.iter().position(
                |item| matches!(item, crate::scene::SceneItem::Managed(window) if *window == id),
            ) else {
                continue;
            };
            let front = scene_elements(
                state,
                renderer,
                &mut decorations,
                scene_order[..index].iter().cloned(),
                true,
            );
            let mut occluders = Vec::new();
            collect_occluders(&front, scale, &mut occluders);
            let client = client_backdrop_rect(managed.rect, scale);
            if occluders.iter().any(|outer| covers(*outer, client)) {
                continue;
            }
            let rect = managed.rect;
            let rect = (rect.x, rect.y, rect.w, rect.h);
            sites.push(DockSite {
                index,
                site: BackdropSite::Window(id),
                rect,
                radius: u32::from(
                    client_corner_radii(managed, managed.titlebars)[0]
                        .max(client_corner_radii(managed, managed.titlebars)[1])
                        .max(client_corner_radii(managed, managed.titlebars)[2])
                        .max(client_corner_radii(managed, managed.titlebars)[3]),
                ),
            });
        }
    }
    // Flat (`reduceTransparency`) material is opaque and an opaque Dock hides the scene beneath it.
    if glass_flat() {
        decorations.drop_docks();
        return Ok(());
    }
    let scale = state.output_scale();
    let scale_milli = (scale * 1000.0).round() as u32;
    let mut live = Vec::with_capacity(sites.len());
    for site in sites {
        let region = physical_rect(site.rect, scale);
        let elements = scene_beneath(state, renderer, &mut decorations, site.index, region, scale);
        let (window_radii, site_material) = site_parameters(state, site.site, material);
        let cached = decorations.docks.get(&site.site);
        // The cheap key fields first: when the rect, radius, scale or material moved, a recapture is
        // due whatever the scene holds, and no stamp is needed. Otherwise compare the freshly built
        // elements against the cached stamp in place; a frame that changes nothing allocates no
        // stamp and rebuilds no texture.
        if let Some(backing) = cached {
            let same_fields = backing.key.rect == site.rect
                && backing.key.radius == site.radius
                && backing.key.window_radii == window_radii
                && backing.key.scale_milli == scale_milli
                && backing.key.material == site_material;
            if same_fields && scene_matches(&elements, region, scale, &backing.key.scene) {
                live.push(site.site);
                continue;
            }
        }
        let key = BackdropKey {
            site: site.site,
            rect: site.rect,
            radius: site.radius,
            window_radii,
            scale_milli,
            material: site_material,
            scene: scene_stamp(&elements, region, scale),
        };
        if needs_capture(
            &key,
            decorations
                .docks
                .get(&site.site)
                .map(|backing| &backing.key),
        ) {
            let backing = capture_dock(renderer, &mut decorations, elements, key, region, scale)?;
            decorations.docks.insert(site.site, backing);
        }
        live.push(site.site);
    }
    decorations.docks.retain(|site, _| live.contains(site));
    Ok(())
}

impl Decorations {
    /// Frees every Dock backdrop and the capture texture, for opaque or flat material.
    pub(super) fn drop_docks(&mut self) {
        self.docks.clear();
        self.dock_scratch = None;
    }

    /// The cached backdrop of the Dock at `rect`, placed beneath the Dock surface. `None` when no
    /// backdrop was captured for it (opaque material, or no mapped Dock there).
    pub(super) fn dock_backdrop(
        &self,
        renderer: &mut GlowRenderer,
        rect: RectKey,
        scale: f64,
    ) -> Option<Elem> {
        let backing = self.docks.get(&BackdropSite::Dock(rect))?;
        Self::backing_element(backing, renderer, rect, scale)
    }

    pub(super) fn window_backdrop(
        &self,
        renderer: &mut GlowRenderer,
        id: WindowId,
        rect: RectKey,
        scale: f64,
    ) -> Option<Elem> {
        let backing = self.docks.get(&BackdropSite::Window(id))?;
        Self::backing_element(backing, renderer, rect, scale)
    }

    fn backing_element(
        backing: &DockBacking,
        renderer: &mut GlowRenderer,
        rect: RectKey,
        scale: f64,
    ) -> Option<Elem> {
        let origin = physical_point(rect.0, rect.1, scale);
        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            Point::from((f64::from(origin.x), f64::from(origin.y))),
            &backing.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
        .map(Elem::Memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(
        rect: RectKey,
        radius: u32,
        opacity: u8,
        blur: u8,
        scene: &[(RectKey, CommitCounter)],
    ) -> BackdropKey {
        BackdropKey {
            site: BackdropSite::Dock(rect),
            rect,
            radius,
            window_radii: [0; 4],
            scale_milli: 1000,
            material: DockMaterial { opacity, blur },
            scene: scene.to_vec(),
        }
    }

    fn committed(times: usize) -> CommitCounter {
        let mut commit = CommitCounter::default();
        for _ in 0..times {
            commit.increment();
        }
        commit
    }

    #[test]
    fn opaque_material_without_blur_needs_no_backdrop() {
        assert!(
            !DockMaterial {
                opacity: 100,
                blur: 0
            }
            .needs_backdrop()
        );
        assert!(
            DockMaterial {
                opacity: 99,
                blur: 0
            }
            .needs_backdrop()
        );
        assert!(
            DockMaterial {
                opacity: 100,
                blur: 40
            }
            .needs_backdrop()
        );
    }

    #[test]
    fn material_round_trips_through_the_packed_static() {
        let translucent = DockMaterial {
            opacity: 80,
            blur: 55,
        };
        set_dock_material(translucent);
        assert_eq!(current_dock_material(), translucent);
        assert!(
            !set_dock_material(translucent),
            "same value is not a change"
        );
        set_dock_material(DockMaterial {
            opacity: 100,
            blur: 0,
        });
    }

    #[test]
    fn first_frame_and_every_key_field_recapture() {
        let scene = vec![((0, 0, 100, 100), committed(1))];
        let base = key((10, 20, 300, 40), 20, 80, 50, &scene);
        assert!(needs_capture(&base, None), "no cached backdrop yet");
        assert!(
            !needs_capture(&base, Some(&base)),
            "identical key reuses the cache"
        );

        let moved = key((11, 20, 300, 40), 20, 80, 50, &scene);
        assert!(needs_capture(&moved, Some(&base)), "dock rect changed");
        let material = key((10, 20, 300, 40), 20, 70, 50, &scene);
        assert!(needs_capture(&material, Some(&base)), "opacity changed");
        let blur = key((10, 20, 300, 40), 20, 80, 60, &scene);
        assert!(needs_capture(&blur, Some(&base)), "blur changed");
        let radius = key((10, 20, 300, 40), 16, 80, 50, &scene);
        assert!(needs_capture(&radius, Some(&base)), "corner radius changed");
        let mut rescaled = key((10, 20, 300, 40), 20, 80, 50, &scene);
        rescaled.scale_milli = 2000;
        assert!(
            needs_capture(&rescaled, Some(&base)),
            "output scale changed"
        );
    }

    #[test]
    fn site_identity_material_and_skip_policy_are_distinct() {
        let rect = (1, 2, 30, 40);
        assert_ne!(BackdropSite::Dock(rect), BackdropSite::Window(1));
        let dock = key(rect, 0, 80, 20, &[]);
        let mut window = dock.clone();
        window.site = BackdropSite::Window(1);
        assert!(needs_capture(&window, Some(&dock)));
        let material = DockMaterial {
            opacity: 80,
            blur: 0,
        };
        assert!(window_needs_backdrop(true, false, material, false));
        assert!(!window_needs_backdrop(false, false, material, false));
        assert!(!window_needs_backdrop(true, true, material, false));
        assert!(!window_needs_backdrop(true, false, material, true));
        assert!(!window_needs_backdrop(
            true,
            false,
            DockMaterial {
                opacity: 100,
                blur: 0
            },
            false
        ));
    }

    #[test]
    fn scene_changes_under_the_dock_recapture_and_nothing_else_does() {
        let base = key(
            (0, 0, 200, 40),
            0,
            80,
            50,
            &[((0, 0, 100, 100), committed(1))],
        );
        // A commit on a window beneath the dock.
        let committed_again = key(
            (0, 0, 200, 40),
            0,
            80,
            50,
            &[((0, 0, 100, 100), committed(2))],
        );
        assert!(needs_capture(&committed_again, Some(&base)));
        // The same window moved under the dock.
        let moved = key(
            (0, 0, 200, 40),
            0,
            80,
            50,
            &[((5, 0, 100, 100), committed(1))],
        );
        assert!(needs_capture(&moved, Some(&base)));
        // A window restacked above another beneath the dock changes the order.
        let two = key(
            (0, 0, 200, 40),
            0,
            80,
            50,
            &[
                ((0, 0, 50, 50), committed(0)),
                ((0, 0, 100, 100), committed(1)),
            ],
        );
        let swapped = key(
            (0, 0, 200, 40),
            0,
            80,
            50,
            &[
                ((0, 0, 100, 100), committed(1)),
                ((0, 0, 50, 50), committed(0)),
            ],
        );
        assert!(needs_capture(&swapped, Some(&two)));
        // A static scene reuses the cache across a full second at 60 frames per second.
        let mut recaptures = 0;
        for _ in 0..60 {
            if needs_capture(&base, Some(&base)) {
                recaptures += 1;
            }
        }
        assert_eq!(recaptures, 0, "a static scene never recaptures");
    }

    #[test]
    fn corner_mask_is_transparent_outside_the_pill_and_opaque_inside() {
        let (w, h, r) = (40usize, 20usize, 10.0f32);
        assert_eq!(
            rounded_coverage(w, h, r, 0, 0),
            0,
            "corner pixel is outside the arc"
        );
        assert_eq!(rounded_coverage(w, h, r, w - 1, h - 1), 0);
        assert_eq!(
            rounded_coverage(w, h, r, w / 2, h / 2),
            255,
            "centre is covered"
        );
        assert_eq!(
            rounded_coverage(w, h, r, w / 2, 0),
            255,
            "top edge is covered"
        );
        assert!(
            rounded_coverage(w, h, r, 0, h / 2) >= 250,
            "the left edge mid-height is covered up to the arc's antialiasing"
        );
        assert_eq!(
            rounded_coverage(w, h, 0.0, 0, 0),
            255,
            "a square dock has no corner to cut"
        );
    }

    #[test]
    fn stamp_matches_needs_the_same_entries_in_the_same_order() {
        let cached = vec![
            ((0, 0, 10, 10), committed(1)),
            ((20, 0, 10, 10), committed(2)),
        ];
        // A cache hit is an exact, ordered match; no allocation is needed to decide it.
        assert!(stamp_matches(cached.iter().copied(), &cached));
        assert!(
            !stamp_matches(cached.iter().copied().take(1), &cached),
            "a missing entry recaptures"
        );
        assert!(
            !stamp_matches(
                cached
                    .iter()
                    .copied()
                    .chain(std::iter::once(((0, 0, 1, 1), committed(3)))),
                &cached
            ),
            "an extra entry recaptures"
        );
        let swapped = [
            ((20, 0, 10, 10), committed(2)),
            ((0, 0, 10, 10), committed(1)),
        ];
        assert!(
            !stamp_matches(swapped.iter().copied(), &cached),
            "restacking recaptures"
        );
        let recommitted = [
            ((0, 0, 10, 10), committed(2)),
            ((20, 0, 10, 10), committed(2)),
        ];
        assert!(
            !stamp_matches(recommitted.iter().copied(), &cached),
            "a commit recaptures"
        );
    }
}
