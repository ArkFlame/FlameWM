//! Redraw planning for the X11 scene: a pure decision between a full repaint and a
//! region-clipped repaint, plus the counters that measure either path.
//!
//! The live redraw path does not consume this yet. Region painting is only enabled
//! after the clip plumbing (GC, XRender picture, Xft draw) and the Full/Region
//! equivalence harness exist; until then every call resolves to `RedrawPlan::Full`.

use flamewm_render_core::{PaintCommand, Rect, SurfaceDamage};

/// Device-pixel rectangle, always inside the window bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PixelRegion {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl PixelRegion {
    pub(crate) fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    pub(crate) fn intersects(self, other: PixelRegion) -> bool {
        self.x < other.x + other.width as i32
            && other.x < self.x + self.width as i32
            && self.y < other.y + other.height as i32
            && other.y < self.y + self.height as i32
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RedrawPlan {
    Full,
    Region(PixelRegion),
}

/// Everything besides the damage that must be unchanged for a region repaint to be
/// equivalent to a full one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PlanInput {
    pub(crate) damage: SurfaceDamage,
    pub(crate) viewport: (u32, u32),
    /// Viewport, scale, interaction and scroll fingerprint match the retained scene.
    pub(crate) fingerprint_unchanged: bool,
    /// A node or surface backdrop samples the scene beneath it and must be refreshed.
    pub(crate) backdrop_refresh_needed: bool,
    /// A Full repaint was latched and not yet consumed.
    pub(crate) full_latched: bool,
    /// The retained backbuffer still holds the last presented scene.
    pub(crate) backbuffer_valid: bool,
}

/// Chooses the repaint for one redraw. Anything not provably local is `Full`.
pub(crate) fn plan_redraw(input: PlanInput) -> RedrawPlan {
    let SurfaceDamage::Region(rect) = input.damage else {
        return RedrawPlan::Full;
    };
    let stale = !input.fingerprint_unchanged
        || input.backdrop_refresh_needed
        || input.full_latched
        || !input.backbuffer_valid;
    if stale {
        return RedrawPlan::Full;
    }
    match pixel_region(rect, input.viewport) {
        Some(region) => RedrawPlan::Region(region),
        None => RedrawPlan::Full,
    }
}

/// Rounds a logical damage rect outward to device pixels and clips it to the viewport.
/// Returns `None` when the result is empty, so the caller falls back to `Full`.
pub(crate) fn pixel_region(rect: Rect, viewport: (u32, u32)) -> Option<PixelRegion> {
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    let x0 = rect.x.floor().max(0.0).min(vw);
    let y0 = rect.y.floor().max(0.0).min(vh);
    let x1 = (rect.x + rect.width).ceil().max(0.0).min(vw);
    let y1 = (rect.y + rect.height).ceil().max(0.0).min(vh);
    (x1 > x0 && y1 > y0).then_some(PixelRegion {
        x: x0 as i32,
        y: y0 as i32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    })
}

/// Per-redraw counters: the cost model for the Full and Region paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RedrawCounters {
    pub(crate) frames: u64,
    /// Pixels the clear, backdrop and paint passes were allowed to touch.
    pub(crate) clip_area_px: u64,
    /// Pixels copied from the backbuffer to the window.
    pub(crate) copy_area_px: u64,
    /// Paint commands visited, whether or not they drew.
    pub(crate) paint_command_visits: u64,
    /// Shape-mask refresh calls that reached the XShape bridge.
    pub(crate) shape_calls: u64,
}

impl RedrawCounters {
    /// Records one redraw. `visits` is the command count; `shape` marks a shape call.
    pub(crate) fn record(
        &mut self,
        plan: RedrawPlan,
        viewport: (u32, u32),
        visits: u64,
        shape: bool,
    ) {
        let full = u64::from(viewport.0) * u64::from(viewport.1);
        let area = match plan {
            RedrawPlan::Full => full,
            RedrawPlan::Region(region) => region.area(),
        };
        self.frames += 1;
        self.clip_area_px += area;
        self.copy_area_px += area;
        self.paint_command_visits += visits;
        self.shape_calls += u64::from(shape);
    }
}

/// Device bounds that a supported opaque command can touch, including its anti-aliased
/// edge. `None` means the command is outside the region-paint subset.
fn supported_bounds(command: &PaintCommand) -> Option<(Rect, f32)> {
    match command {
        PaintCommand::FillRect { rect, radius, .. } => Some((*rect, *radius)),
        PaintCommand::StrokeRect {
            rect,
            width,
            radius,
            ..
        } => Some((*rect, width.max(*radius))),
        _ => None,
    }
}

/// Whether a rect fill or stroke is solid. Translucent ones blend over the scene beneath them,
/// so they are only region-safe while that scene is the same one the backbuffer was painted on.
fn opaque_fill(command: &PaintCommand) -> bool {
    matches!(
        command,
        PaintCommand::FillRect { color, .. } | PaintCommand::StrokeRect { color, .. } if color.a == 255
    )
}

/// Conservative device pixels a supported command may write.
pub(crate) fn command_bounds(command: &PaintCommand, viewport: (u32, u32)) -> Option<PixelRegion> {
    if let PaintCommand::PushClip { rect } = command {
        return pixel_region(*rect, viewport);
    }
    if let PaintCommand::Text { x, y, size, .. } = command {
        // Glyph ink lies between the ascent (about `size` above the baseline) and the descent;
        // a run may extend to the window's right edge, so the bound spans the whole row.
        let rect = Rect {
            x: x - size,
            y: y - size,
            width: viewport.0 as f32 + size,
            height: 3.0 * size,
        };
        return pixel_region(rect, viewport);
    }
    if let PaintCommand::Image { rect, .. } = command {
        // Device pixels are the logical rect at scale 1 (region paint requires it); one pixel
        // of padding covers the edge rounding.
        let r = *rect;
        return pixel_region(
            Rect {
                x: r.x - 1.0,
                y: r.y - 1.0,
                width: r.width + 2.0,
                height: r.height + 2.0,
            },
            viewport,
        );
    }
    if let PaintCommand::TextShadow {
        x,
        y,
        size,
        blur_radius,
        ..
    } = command
    {
        // Mirrors `paint_text_shadow`: the coverage mask extends `pad` beyond the glyph box on
        // every side, plus a few pixels of slack for the glyph origin rounding.
        let (x, y, size) = (*x, *y, *size);
        let pad = (blur_radius.clamp(0.0, 8.0) * 3.0).ceil() + 8.0;
        let rect = Rect {
            x: x - size - pad,
            y: y - size - pad,
            width: viewport.0 as f32 + size + 2.0 * pad,
            height: 4.0 * size + 2.0 * pad,
        };
        return pixel_region(rect, viewport);
    }
    let (rect, reach) = supported_bounds(command)?;
    let pad = reach.ceil() + 2.0;
    let grown = Rect {
        x: rect.x - pad,
        y: rect.y - pad,
        width: rect.width + 2.0 * pad,
        height: rect.height + 2.0 * pad,
    };
    pixel_region(grown, viewport)
}

/// Opaque text: a run in a solid colour. Translucent runs need a stable backdrop, like fills.
fn opaque_text(command: &PaintCommand) -> bool {
    matches!(command, PaintCommand::Text { color, .. } if color.a == 255)
}

/// Images and text shadows blend over what is beneath them. The planner does not read the
/// decoded pixels, so both need the same stable-backdrop proof as translucent fills.
fn backdrop_blended(command: &PaintCommand) -> bool {
    matches!(
        command,
        PaintCommand::Image { .. } | PaintCommand::TextShadow { .. }
    )
}

/// Clip pushes and pops: no pixels of their own. They are always replayed so the stack
/// matches the Full path, and a push bounds the pixels its clip can change.
fn clip_command(command: &PaintCommand) -> bool {
    matches!(
        command,
        PaintCommand::PushClip { .. } | PaintCommand::PopClip
    )
}

/// `translucent` admits translucent rect fills and strokes; the caller proves the backdrop under
/// them is the one the backbuffer was painted over.
fn region_supported(command: &PaintCommand, translucent: bool) -> bool {
    (supported_bounds(command).is_some() && (translucent || opaque_fill(command)))
        || (matches!(command, PaintCommand::Text { .. }) && (translucent || opaque_text(command)))
        || (backdrop_blended(command) && translucent)
        || clip_command(command)
}

/// Whether every command in `commands` is region-safe (see [`region_eligible`]).
pub(crate) fn region_supported_all(commands: &[PaintCommand], translucent: bool) -> bool {
    commands
        .iter()
        .all(|command| region_supported(command, translucent))
}

/// True when every command is an unscaled rect fill or stroke, a text run, an image, a text shadow,
/// or a clip push or pop. Node backdrops stay on the Full path. Translucent fills, translucent
/// text, images and text shadows need `translucent` (a stable backdrop).
pub(crate) fn region_eligible(commands: &[PaintCommand], scale: f32, translucent: bool) -> bool {
    scale == 1.0 && !commands.is_empty() && region_supported_all(commands, translucent)
}

/// Inputs to the region-paint decision for one redraw.
pub(crate) struct RegionInput<'a> {
    /// Commands last painted into the backbuffer; `None` before the first paint.
    pub(crate) old: Option<&'a [PaintCommand]>,
    pub(crate) new: &'a [PaintCommand],
    pub(crate) scale: f32,
    pub(crate) viewport: (u32, u32),
    /// `FLAMEWM_RENDER_REGION=1` (or a test override).
    pub(crate) enabled: bool,
    /// Viewport and scale equal the last painted layout key.
    pub(crate) viewport_unchanged: bool,
    /// A surface or window backdrop that changed since the backbuffer was painted, or an active
    /// clip stack, samples beyond the region.
    pub(crate) backdrops: bool,
    /// The window backdrop is unchanged since the backbuffer was painted, so translucent fills
    /// may take the region path over it.
    pub(crate) translucent: bool,
    /// A Full repaint was latched for this redraw.
    pub(crate) full_latched: bool,
    /// The backbuffer holds the last fully presented frame.
    pub(crate) base_valid: bool,
}

/// Region when every gate holds and the command diff is local; `Full` otherwise.
pub(crate) fn plan_region_paint(input: RegionInput<'_>) -> RedrawPlan {
    if !input.enabled
        || !input.viewport_unchanged
        || input.backdrops
        || input.full_latched
        || !input.base_valid
    {
        return RedrawPlan::Full;
    }
    let Some(old) = input.old else {
        return RedrawPlan::Full;
    };
    if !region_eligible(input.new, input.scale, input.translucent)
        || !region_supported_all(old, input.translucent)
    {
        return RedrawPlan::Full;
    }
    let Some(region) = diff_region(old, input.new, input.viewport) else {
        return RedrawPlan::Full;
    };
    let viewport_area = u64::from(input.viewport.0) * u64::from(input.viewport.1);
    if region.area() * 2 > viewport_area {
        return RedrawPlan::Full;
    }
    RedrawPlan::Region(region)
}

/// Smallest region that covers every command whose old or new form differs. Equal command
/// lists give `None`. Pixels outside the region see the same sequence of covering commands
/// in both lists, so they are unchanged.
pub(crate) fn diff_region(
    old: &[PaintCommand],
    new: &[PaintCommand],
    viewport: (u32, u32),
) -> Option<PixelRegion> {
    if old.len() != new.len() {
        return None;
    }
    let mut union: Option<PixelRegion> = None;
    for (before, after) in old.iter().zip(new) {
        if before == after {
            continue;
        }
        for command in [before, after] {
            let bounds = command_bounds(command, viewport)?;
            union = Some(match union {
                None => bounds,
                Some(u) => {
                    let x0 = u.x.min(bounds.x);
                    let y0 = u.y.min(bounds.y);
                    let x1 = (u.x + u.width as i32).max(bounds.x + bounds.width as i32);
                    let y1 = (u.y + u.height as i32).max(bounds.y + bounds.height as i32);
                    PixelRegion {
                        x: x0,
                        y: y0,
                        width: (x1 - x0) as u32,
                        height: (y1 - y0) as u32,
                    }
                }
            });
        }
    }
    union
}

#[cfg(test)]
mod region_tests {
    use super::*;
    use flamewm_render_core::Color;

    const VIEW: (u32, u32) = (200, 100);

    fn fill(x: f32, y: f32, w: f32, h: f32, a: u8) -> PaintCommand {
        PaintCommand::FillRect {
            rect: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            color: Color {
                r: 1,
                g: 2,
                b: 3,
                a,
            },
            radius: 0.0,
        }
    }

    #[test]
    fn unchanged_list_has_no_region() {
        let list = vec![fill(0.0, 0.0, 10.0, 10.0, 255)];
        assert_eq!(diff_region(&list, &list, VIEW), None);
    }

    #[test]
    fn one_moved_rect_covers_old_and_new_only() {
        let old = vec![fill(10.0, 10.0, 20.0, 20.0, 255)];
        let new = vec![fill(11.0, 10.0, 20.0, 20.0, 255)];
        let region = diff_region(&old, &new, VIEW).expect("moved rect");
        // Padding is 2px: old bounds start at 10-2=8, new bounds end at 11+20+2=33 on x.
        assert_eq!(region.x, 8);
        assert_eq!(region.y, 8);
        assert_eq!(region.x + region.width as i32, 33);
        assert!(region.area() < (VIEW.0 * VIEW.1) as u64 / 10);
    }

    #[test]
    fn translucent_or_text_commands_are_ineligible() {
        assert!(!region_eligible(
            &[fill(0.0, 0.0, 5.0, 5.0, 128)],
            1.0,
            false
        ));
        assert!(
            region_eligible(&[fill(0.0, 0.0, 5.0, 5.0, 128)], 1.0, true),
            "translucent fill over a stable backdrop"
        );
        assert!(!region_eligible(
            &[fill(0.0, 0.0, 5.0, 5.0, 255)],
            1.5,
            false
        ));
        assert!(!region_eligible(&[], 1.0, false));
        let text = PaintCommand::PushClip {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        };
        assert!(region_eligible(&[text], 1.0, false));
        assert!(region_eligible(
            &[fill(0.0, 0.0, 5.0, 5.0, 255)],
            1.0,
            false
        ));
        let shadow = PaintCommand::TextShadow {
            x: 0.0,
            y: 0.0,
            color: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 128,
            },
            size: 12.0,
            weight: 400,
            blur_radius: 2.0,
            text: "x".into(),
        };
        assert!(!region_eligible(&[shadow], 1.0, false));
        let text = |alpha| PaintCommand::Text {
            x: 4.0,
            y: 20.0,
            color: Color {
                r: 1,
                g: 2,
                b: 3,
                a: alpha,
            },
            size: 12.0,
            weight: 400,
            text: "ab".into(),
        };
        assert!(region_eligible(&[text(255)], 1.0, false));
        assert!(!region_eligible(&[text(128)], 1.0, false));
    }

    #[test]
    fn text_bounds_cover_the_glyph_box_to_the_viewport_edge() {
        let run = PaintCommand::Text {
            x: 10.0,
            y: 50.0,
            color: Color {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            },
            size: 10.0,
            weight: 400,
            text: "xyz".into(),
        };
        let bounds = command_bounds(&run, VIEW).expect("opaque text is bounded");
        assert_eq!((bounds.x, bounds.y), (0, 40));
        assert_eq!(bounds.x + bounds.width as i32, 200);
        assert_eq!(bounds.y + bounds.height as i32, 70);
    }

    #[test]
    fn moved_translucent_rect_over_unchanged_backdrop_stays_region() {
        let old = vec![fill(10.0, 10.0, 20.0, 20.0, 128)];
        let new = vec![fill(12.0, 10.0, 20.0, 20.0, 128)];
        let input = |translucent| RegionInput {
            old: Some(old.as_slice()),
            new: new.as_slice(),
            scale: 1.0,
            viewport: VIEW,
            enabled: true,
            viewport_unchanged: true,
            backdrops: false,
            translucent,
            full_latched: false,
            base_valid: true,
        };
        assert!(
            matches!(plan_region_paint(input(true)), RedrawPlan::Region(_)),
            "translucent move over an unchanged window backdrop must stay region"
        );
        assert_eq!(
            plan_region_paint(input(false)),
            RedrawPlan::Full,
            "without a stable backdrop the translucent move must repaint in full"
        );
    }

    #[test]
    fn length_change_falls_back() {
        let one = vec![fill(0.0, 0.0, 5.0, 5.0, 255)];
        let two = vec![fill(0.0, 0.0, 5.0, 5.0, 255), fill(9.0, 9.0, 5.0, 5.0, 255)];
        assert_eq!(diff_region(&one, &two, VIEW), None);
    }

    #[test]
    fn bounds_clip_to_viewport() {
        let edge = fill(-2.0, 95.0, 4.0, 4.0, 255);
        let bounds = command_bounds(&edge, VIEW).expect("supported");
        assert_eq!(bounds.x, 0);
        assert_eq!(bounds.y + bounds.height as i32, 100);
    }

    fn image(x: f32, y: f32, w: f32, h: f32) -> PaintCommand {
        PaintCommand::Image {
            rect: Rect {
                x,
                y,
                width: w,
                height: h,
            },
            asset: 0,
            node: 0,
            revision: 0,
            treatment: flamewm_render_core::ImageTreatment::Original,
            tint: None,
        }
    }

    fn shadow(x: f32, y: f32, blur: f32) -> PaintCommand {
        PaintCommand::TextShadow {
            x,
            y,
            color: Color {
                r: 0,
                g: 0,
                b: 0,
                a: 160,
            },
            size: 12.0,
            weight: 400,
            blur_radius: blur,
            text: "label".into(),
        }
    }

    #[test]
    fn images_and_shadows_need_a_stable_backdrop() {
        // Images and shadows blend over the scene beneath them, like translucent fills.
        assert!(!region_eligible(&[image(0.0, 0.0, 8.0, 8.0)], 1.0, false));
        assert!(region_eligible(&[image(0.0, 0.0, 8.0, 8.0)], 1.0, true));
        assert!(!region_eligible(&[shadow(4.0, 20.0, 2.0)], 1.0, false));
        assert!(region_eligible(&[shadow(4.0, 20.0, 2.0)], 1.0, true));
        // Scaled scenes keep the Full path for every command.
        assert!(!region_eligible(&[image(0.0, 0.0, 8.0, 8.0)], 2.0, true));
    }

    #[test]
    fn translucent_text_needs_a_stable_backdrop() {
        let run = |alpha| PaintCommand::Text {
            x: 4.0,
            y: 20.0,
            color: Color {
                r: 1,
                g: 2,
                b: 3,
                a: alpha,
            },
            size: 12.0,
            weight: 400,
            text: "ab".into(),
        };
        assert!(!region_eligible(&[run(128)], 1.0, false));
        assert!(region_eligible(&[run(128)], 1.0, true));
    }

    #[test]
    fn image_bounds_cover_the_rect_and_its_edge() {
        let bounds = command_bounds(&image(10.4, 20.0, 30.0, 16.0), VIEW).expect("image");
        assert!(bounds.x <= 10 && bounds.x + bounds.width as i32 >= 41);
        assert!(bounds.y <= 19 && bounds.y + bounds.height as i32 >= 37);
    }

    #[test]
    fn shadow_bounds_cover_the_blur_mask_on_both_sides() {
        let bounds = command_bounds(&shadow(50.0, 60.0, 2.0), VIEW).expect("shadow");
        // The coverage mask reaches `ceil(3 * blur) + 4` past the glyph box; `size` is 12.
        assert!(bounds.x <= 50 - 12 - 10 && bounds.y <= 60 - 12 - 10);
        assert_eq!(bounds.x + bounds.width as i32, VIEW.0 as i32);
        assert_eq!(bounds.y + bounds.height as i32, VIEW.1 as i32);
    }

    #[test]
    fn moved_image_region_is_old_union_new() {
        let old = vec![image(0.0, 0.0, 16.0, 16.0)];
        let new = vec![image(40.0, 0.0, 16.0, 16.0)];
        let region = diff_region(&old, &new, VIEW).expect("moved image");
        assert!(region.x <= 0 && region.x + region.width as i32 >= 56);
        assert!(region.area() < (VIEW.0 * VIEW.1) as u64 / 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn input(damage: SurfaceDamage) -> PlanInput {
        PlanInput {
            damage,
            viewport: (1920, 1080),
            fingerprint_unchanged: true,
            backdrop_refresh_needed: false,
            full_latched: false,
            backbuffer_valid: true,
        }
    }

    #[test]
    fn one_cell_region_plans_region_and_touches_far_fewer_pixels() {
        let cell = rect(100.4, 200.2, 9.0, 18.0);
        let RedrawPlan::Region(region) = plan_redraw(input(SurfaceDamage::Region(cell))) else {
            panic!("one-cell damage must plan a region repaint");
        };
        assert_eq!(
            region,
            PixelRegion {
                x: 100,
                y: 200,
                width: 10,
                height: 19
            }
        );
        let mut full = RedrawCounters::default();
        let mut partial = RedrawCounters::default();
        full.record(RedrawPlan::Full, (1920, 1080), 500, false);
        partial.record(RedrawPlan::Region(region), (1920, 1080), 3, false);
        assert_eq!(full.clip_area_px, 1920 * 1080);
        assert_eq!(partial.clip_area_px, 190);
        assert!(partial.clip_area_px * 1000 < full.clip_area_px);
        assert_eq!(partial.copy_area_px, partial.clip_area_px);
    }

    #[test]
    fn any_staleness_condition_forces_full() {
        let damage = SurfaceDamage::Region(rect(0.0, 0.0, 10.0, 10.0));
        let stale = [
            PlanInput {
                fingerprint_unchanged: false,
                ..input(damage)
            },
            PlanInput {
                backdrop_refresh_needed: true,
                ..input(damage)
            },
            PlanInput {
                full_latched: true,
                ..input(damage)
            },
            PlanInput {
                backbuffer_valid: false,
                ..input(damage)
            },
        ];
        for case in stale {
            assert_eq!(plan_redraw(case), RedrawPlan::Full, "{case:?}");
        }
    }

    #[test]
    fn non_region_damage_is_full() {
        assert_eq!(plan_redraw(input(SurfaceDamage::Full)), RedrawPlan::Full);
        assert_eq!(plan_redraw(input(SurfaceDamage::None)), RedrawPlan::Full);
    }

    #[test]
    fn offscreen_and_negative_regions_clip_to_viewport() {
        let offscreen = rect(-50.0, -20.0, 30.0, 40.0);
        assert_eq!(pixel_region(offscreen, (1920, 1080)), None);
        let straddle = rect(-5.5, 1070.0, 20.0, 20.0);
        assert_eq!(
            pixel_region(straddle, (1920, 1080)),
            Some(PixelRegion {
                x: 0,
                y: 1070,
                width: 15,
                height: 10
            })
        );
    }
}
