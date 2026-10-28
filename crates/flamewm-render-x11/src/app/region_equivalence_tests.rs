//! Xvfb equivalence harness: the same scripted document events are painted by a Full app
//! and by a region-forced app. After every event the backbuffer bytes (ARGB32, alpha
//! included) must match exactly. Scenarios the planner must not take as Region are checked
//! too, so a Full fallback cannot pass by accident. Needs `FLAMEWM_TEST_DISPLAY`.

use super::*;
use flamewm_render_core::{
    Color, ColorValue, CompiledDocument, CompiledNode, ImageAsset, Length, NodeKind, Overflow,
    Position, Style, TextShadow,
};
use std::os::raw::{c_int, c_ulong};

const Z_PIXMAP: c_int = 2;

/// Root (opaque background) with three absolutely positioned opaque boxes.
/// Box `b` has a 2px border so its stroke is a separate command.
fn fixture() -> RuntimeDocument {
    let node = |id: &str,
                parent: Option<u32>,
                first_child: Option<u32>,
                next: Option<u32>,
                style: Style| {
        CompiledNode {
            kind: NodeKind::Element,
            parent,
            first_child,
            next_sibling: next,
            id: id.to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: "".to_string().into(),
            image: None,
            style: style.into(),
            hover_style: None,
            active_style: None,
        }
    };
    let child = |left: f32, top: f32, border: f32| Style {
        position: Position::Absolute,
        left: Length::Px(left),
        top: Length::Px(top),
        width: Length::Px(60.0),
        height: Length::Px(40.0),
        border_width: border,
        ..Style::default()
    };
    let root_style = Style {
        width: Length::Px(240.0),
        height: Length::Px(160.0),
        ..Style::default()
    };
    let mut doc = RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes: vec![
            node("root", None, Some(1), None, root_style),
            node("a", Some(0), None, Some(2), child(10.0, 10.0, 0.0)),
            node("b", Some(0), None, Some(3), child(90.0, 20.0, 2.0)),
            node("c", Some(0), None, None, child(40.0, 90.0, 0.0)),
        ],
    })
    .expect("fixture document validates");
    let opaque = |r, g, b| Color { r, g, b, a: 255 };
    doc.set_background_color("root", opaque(40, 44, 52))
        .expect("root bg");
    doc.set_background_color("a", opaque(200, 80, 60))
        .expect("a bg");
    doc.set_background_color("b", opaque(60, 180, 90))
        .expect("b bg");
    doc.set_background_color("c", opaque(70, 90, 220))
        .expect("c bg");
    doc.set_border_color("b", opaque(255, 255, 255))
        .expect("b border");
    doc
}

fn readback(display: *mut Display, app: &X11App) -> Vec<u8> {
    let (w, h) = (app.width, app.height);
    // SAFETY: the backbuffer is a live pixmap of this display; the image is freed below.
    let image = unsafe { XGetImage(display, app.backbuffer, 0, 0, w, h, c_ulong::MAX, Z_PIXMAP) };
    assert!(!image.is_null(), "XGetImage failed");
    // SAFETY: XGetImage returned a valid XImage; XImageHead is its public prefix layout.
    let head = unsafe { &*(image as *const XImageHead) };
    let stride = head.bytes_per_line as usize;
    let row_bytes = w as usize * 4;
    let mut out = Vec::with_capacity(row_bytes * h as usize);
    for row in 0..h as usize {
        // SAFETY: each row lies inside the image data (stride * height bytes).
        let line = unsafe {
            std::slice::from_raw_parts(head.data.add(row * stride) as *const u8, row_bytes)
        };
        out.extend_from_slice(line);
    }
    // SAFETY: matches the XGetImage above.
    unsafe { XDestroyImage(image) };
    out
}

type Step = fn(&mut RuntimeDocument) -> Result<(), String>;

/// (label, mutation, whether Region is expected for this event).
fn script() -> Vec<(&'static str, Step, bool)> {
    vec![
        ("initial paint", |_| Ok(()), false),
        (
            "1px move of box a",
            |d| d.set_position_px("a", 11.0, 10.0),
            true,
        ),
        (
            "overlay resize of box b",
            |d| d.set_size_px("b", 70.0, 40.0),
            true,
        ),
        (
            "hide box b (command count changes)",
            |d| d.set_visible("b", false),
            false,
        ),
        (
            "show box b (command count changes)",
            |d| d.set_visible("b", true),
            false,
        ),
        (
            "recolour box c (selected row)",
            |d| {
                d.set_background_color(
                    "c",
                    Color {
                        r: 255,
                        g: 200,
                        b: 0,
                        a: 255,
                    },
                )
            },
            true,
        ),
        (
            "rounded corners on box a",
            |d| d.set_border_radius("a", 8.0),
            true,
        ),
        (
            "border colour change on b",
            |d| {
                d.set_border_color(
                    "b",
                    Color {
                        r: 10,
                        g: 10,
                        b: 10,
                        a: 255,
                    },
                )
            },
            true,
        ),
        (
            "offscreen negative x for box c",
            |d| d.set_position_px("c", -30.0, 100.0),
            true,
        ),
        (
            "translucent box b (not eligible)",
            |d| {
                d.set_background_color(
                    "b",
                    Color {
                        r: 60,
                        g: 180,
                        b: 90,
                        a: 128,
                    },
                )
            },
            false,
        ),
        // The previous frame held a translucent command, which is outside the opaque subset, so
        // the diff has no bounds for its old form and the planner falls back to Full once.
        (
            "opaque box b restored (previous frame translucent)",
            |d| {
                d.set_background_color(
                    "b",
                    Color {
                        r: 60,
                        g: 180,
                        b: 90,
                        a: 255,
                    },
                )
            },
            false,
        ),
        (
            "1px move of opaque box b after restore",
            |d| d.set_position_px("b", 91.0, 20.0),
            true,
        ),
        (
            "ui scale 1.5 (not eligible)",
            |d| d.set_ui_scale(1.5),
            false,
        ),
        (
            "ui scale back to 1.0 (not eligible)",
            |d| d.set_ui_scale(1.0),
            false,
        ),
    ]
}

/// Text scenes: an opaque root holding a clipped box. The box clips a label with one text
/// child and a caret box. Node 0 root, 1 clip box, 2 label, 3 the label's text, 4 caret.
fn text_fixture() -> RuntimeDocument {
    let element = |parent: Option<u32>,
                   first_child: Option<u32>,
                   next: Option<u32>,
                   style: Style| CompiledNode {
        kind: NodeKind::Element,
        parent,
        first_child,
        next_sibling: next,
        id: "".to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: "".to_string().into(),
        image: None,
        style: style.into(),
        hover_style: None,
        active_style: None,
    };
    let text = CompiledNode {
        kind: NodeKind::Text,
        parent: Some(2),
        first_child: None,
        next_sibling: None,
        id: "".to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: "a long line of text that runs past the clip edge"
            .to_string()
            .into(),
        image: None,
        style: Style::default().into(),
        hover_style: None,
        active_style: None,
    };
    let absolute = |left: f32, top: f32, width: f32, height: f32| Style {
        position: Position::Absolute,
        left: Length::Px(left),
        top: Length::Px(top),
        width: Length::Px(width),
        height: Length::Px(height),
        ..Style::default()
    };
    let clip_box = Style {
        overflow_x: Overflow::Hidden,
        overflow_y: Overflow::Hidden,
        ..absolute(4.0, 4.0, 120.0, 28.0)
    };
    let mut nodes = vec![
        element(
            None,
            Some(1),
            None,
            Style {
                width: Length::Px(240.0),
                height: Length::Px(160.0),
                ..Style::default()
            },
        ),
        element(Some(0), Some(2), None, clip_box),
        element(Some(1), Some(3), Some(4), absolute(6.0, 4.0, 200.0, 20.0)),
        text,
        element(Some(1), None, None, absolute(60.0, 10.0, 2.0, 16.0)),
    ];
    nodes[0].id = "root".to_string().into();
    nodes[1].id = "clip".to_string().into();
    nodes[2].id = "label".to_string().into();
    nodes[4].id = "caret".to_string().into();
    let mut doc = RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: Vec::new(),
        nodes,
    })
    .expect("text fixture validates");
    let opaque = |r, g, b| Color { r, g, b, a: 255 };
    doc.set_background_color("root", opaque(40, 44, 52))
        .expect("root bg");
    doc.set_background_color("caret", opaque(230, 230, 230))
        .expect("caret bg");
    doc.set_font_size_px("label", 13.0).expect("label font");
    doc
}

/// Text steps: a one-character edit, a caret move, a row translation (the scroll-like
/// move), then a translucent root with a node backdrop, where Region must not be taken.
fn text_script() -> Vec<(&'static str, Step, bool)> {
    vec![
        ("initial text paint", |_| Ok(()), false),
        (
            "one-character edit in a clipped label",
            |d| d.set_text("label", "a lang line of text that runs past the clip edge"),
            true,
        ),
        (
            "caret moved 8px inside the clip",
            |d| d.set_position_px("caret", 68.0, 10.0),
            true,
        ),
        (
            "label translated within its clip (scroll-like)",
            |d| d.set_position_px("label", 6.0, 0.0),
            true,
        ),
        (
            "translucent root with node backdrop",
            |d| {
                d.set_background_color(
                    "root",
                    Color {
                        r: 40,
                        g: 44,
                        b: 52,
                        a: 200,
                    },
                )?;
                d.set_node_backdrop("root", Some(40))
            },
            false,
        ),
        (
            "one-character edit under translucent root",
            |d| d.set_text("label", "a long line of text that runs past the clip edge"),
            false,
        ),
    ]
}

/// Window backdrop at 80% opacity (`surface_alpha(80)` = 204 on the root), the terminal default.
fn opacity_80_backdrop() -> crate::backdrop::WindowBackdrop {
    crate::backdrop::WindowBackdrop {
        wallpaper: String::new(),
        fit: "fill".to_string(),
        packaged_root: std::path::PathBuf::new(),
        opacity_percent: 80,
        blur_percent: 20,
    }
}

/// Text scene whose root is translucent at 80% opacity, so it sits over the window backdrop.
fn translucent_text_fixture() -> RuntimeDocument {
    let mut doc = text_fixture();
    doc.set_background_color(
        "root",
        Color {
            r: 40,
            g: 44,
            b: 52,
            a: 204,
        },
    )
    .expect("translucent root");
    doc
}

/// Opacity-80 steps: text edits and a caret move must take Region over the unchanged backdrop.
fn backdrop_script() -> Vec<(&'static str, Step, bool)> {
    vec![
        ("initial paint over window backdrop", |_| Ok(()), false),
        (
            "one-character edit over window backdrop",
            |d| d.set_text("label", "a lang line of text that runs past the clip edge"),
            true,
        ),
        (
            "caret moved over window backdrop",
            |d| d.set_position_px("caret", 68.0, 10.0),
            true,
        ),
    ]
}

/// Runs `script` on a Full app and a Region-enabled app over `make`, asserting the planner's
/// path and 0 differing backbuffer pixels after every step. Closes `display`; returns the Region
/// step count.
unsafe fn compare_region_and_full(
    display: *mut Display,
    make: fn() -> RuntimeDocument,
    script: Vec<(&'static str, Step, bool)>,
    backdrop: Option<crate::backdrop::WindowBackdrop>,
) -> usize {
    // SAFETY: the caller keeps `display` open for the whole call.
    unsafe {
        let config = X11Config {
            width: 240,
            height: 160,
            backdrop,
            ..X11Config::default()
        };
        let mut full = X11App::new(display, &config).expect("full app");
        let mut region = X11App::new(display, &config).expect("region app");
        full.region_paint = false;
        region.region_paint = true;
        let mut doc_full = make();
        let mut doc_region = make();
        let viewport_area = u64::from(config.width) * u64::from(config.height);

        let mut region_steps = 0usize;
        let mut previous = readback(display, &full);
        for (label, step, expect_region) in script {
            step(&mut doc_full).expect("full mutation");
            step(&mut doc_region).expect("region mutation");
            let before = region.render_counters.clip_area_px;
            let full_before = full.render_counters.clip_area_px;
            full.redraw(&doc_full).expect("full redraw");
            region.redraw(&doc_region).expect("region redraw");
            let used_region = region.render_counters.clip_area_px - before < viewport_area;
            let full_used = full.render_counters.clip_area_px - full_before;
            assert_eq!(
                full_used, viewport_area,
                "{label}: full app must always paint full"
            );
            assert_eq!(
                used_region, expect_region,
                "{label}: planner path mismatch (region used = {used_region})"
            );
            if used_region {
                region_steps += 1;
            }
            let full_pixels = readback(display, &full);
            let region_pixels = readback(display, &region);
            let differing = full_pixels
                .chunks_exact(4)
                .zip(region_pixels.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            // Proves the step changed the frame, so 0 differing px is not two blank surfaces.
            let changed = previous
                .iter()
                .zip(&full_pixels)
                .filter(|(a, b)| a != b)
                .count();
            previous = full_pixels.clone();
            eprintln!(
                "region-equivalence {label:<44} path={} differing_px={differing} changed_px={changed}",
                if used_region { "Region" } else { "Full" }
            );
            assert_eq!(differing, 0, "{label}: Region pixels differ from Full");
        }

        // Owner order (see runtime.rs): take deferred native handles, drop the apps, close the
        // display, then release the handles.
        let deferred_full = full.take_deferred_native_libraries();
        let deferred_region = region.take_deferred_native_libraries();
        drop(full);
        drop(region);
        XCloseDisplay(display);
        drop(deferred_full);
        drop(deferred_region);
        region_steps
    }
}

/// Opens `FLAMEWM_TEST_DISPLAY` and passes it to `body`, which must close it; returns false
/// when the variable is unset.
fn with_test_display(body: impl FnOnce(*mut Display)) -> bool {
    let Some(name) = std::env::var_os("FLAMEWM_TEST_DISPLAY") else {
        return false;
    };
    let name = std::ffi::CString::new(name.to_string_lossy().as_bytes()).unwrap();
    // SAFETY: the display stays open until `body` closes it.
    unsafe {
        let display = XOpenDisplay(name.as_ptr());
        assert!(!display.is_null(), "XOpenDisplay failed");
        body(display);
    }
    true
}

#[test]
fn region_paint_matches_full_for_every_scripted_event() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration.
        let region_steps = unsafe { compare_region_and_full(display, fixture, script(), None) };
        assert!(
            region_steps >= 5,
            "Region path must be exercised by the scripted events"
        );
    });
}

#[test]
fn region_paint_matches_full_for_text_scenarios() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration.
        let region_steps =
            unsafe { compare_region_and_full(display, text_fixture, text_script(), None) };
        assert!(
            region_steps >= 3,
            "text edit, caret and row translation must take Region"
        );
    });
}

#[test]
fn region_paint_matches_full_over_window_backdrop_at_opacity_80() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration.
        let region_steps = unsafe {
            compare_region_and_full(
                display,
                translucent_text_fixture,
                backdrop_script(),
                Some(opacity_80_backdrop()),
            )
        };
        assert!(
            region_steps >= 2,
            "edits and caret over an unchanged backdrop must take Region"
        );
    });
}

/// Image and shadow scene over a translucent root (the 80% window backdrop applies): an opaque
/// full-window wallpaper, a translucent icon, a translucent selection overlay above the icon, and
/// a label with a blurred text shadow. Node 0 root, 1 wallpaper, 2 icon, 3 selection overlay,
/// 4 label box, 5 label text.
fn image_fixture() -> RuntimeDocument {
    let node = |id: &str,
                kind: NodeKind,
                parent: Option<u32>,
                first_child: Option<u32>,
                next: Option<u32>,
                image: Option<u16>,
                style: Style|
     -> CompiledNode {
        CompiledNode {
            kind,
            parent,
            first_child,
            next_sibling: next,
            id: id.to_string().into(),
            action: std::borrow::Cow::Borrowed(""),
            text: "".to_string().into(),
            image,
            style: style.into(),
            hover_style: None,
            active_style: None,
        }
    };
    let absolute = |left: f32, top: f32, width: f32, height: f32| Style {
        position: Position::Absolute,
        left: Length::Px(left),
        top: Length::Px(top),
        width: Length::Px(width),
        height: Length::Px(height),
        ..Style::default()
    };
    // A 8x8 gradient; the wallpaper is opaque, the icon half transparent.
    let pixels = |alpha: u8| -> Vec<u8> {
        (0..64u32)
            .flat_map(|i| [(i * 4) as u8, 90, 160, alpha])
            .collect()
    };
    let mut doc = RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: vec![
            ImageAsset {
                source: "wall.png".into(),
                width: 8,
                height: 8,
                pixels: pixels(255).into(),
            },
            ImageAsset {
                source: "icon.png".into(),
                width: 8,
                height: 8,
                pixels: pixels(128).into(),
            },
        ],
        nodes: vec![
            node(
                "root",
                NodeKind::Element,
                None,
                Some(1),
                None,
                None,
                Style {
                    width: Length::Px(240.0),
                    height: Length::Px(160.0),
                    ..Style::default()
                },
            ),
            node(
                "wall",
                NodeKind::Image,
                Some(0),
                None,
                Some(2),
                Some(0),
                absolute(0.0, 0.0, 240.0, 160.0),
            ),
            node(
                "icon",
                NodeKind::Image,
                Some(0),
                None,
                Some(3),
                Some(1),
                absolute(20.0, 20.0, 16.0, 16.0),
            ),
            node(
                "sel",
                NodeKind::Element,
                Some(0),
                None,
                Some(4),
                None,
                absolute(60.0, 60.0, 40.0, 40.0),
            ),
            node(
                "label",
                NodeKind::Element,
                Some(0),
                Some(5),
                None,
                None,
                absolute(100.0, 110.0, 120.0, 24.0),
            ),
            CompiledNode {
                text: "shadowed label".to_string().into(),
                style: Style {
                    font_size: 13.0,
                    text_shadow: Some(TextShadow {
                        offset_x: 1.0,
                        offset_y: 1.0,
                        blur_radius: 2.0,
                        color: ColorValue::Literal(Color {
                            r: 0,
                            g: 0,
                            b: 0,
                            a: 160,
                        }),
                    }),
                    ..Style::default()
                }
                .into(),
                ..node(
                    "label-text",
                    NodeKind::Text,
                    Some(4),
                    None,
                    None,
                    None,
                    Style::default(),
                )
            },
        ],
    })
    .expect("image fixture validates");
    doc.set_background_color(
        "root",
        Color {
            r: 40,
            g: 44,
            b: 52,
            a: 204,
        },
    )
    .expect("translucent root");
    doc.set_background_color(
        "sel",
        Color {
            r: 60,
            g: 120,
            b: 220,
            a: 100,
        },
    )
    .expect("translucent selection");
    doc
}

/// Wallpaper, icon and label steps over the window backdrop. Overlay moves and resizes above the
/// icon, an icon move, and a shadowed label edit and move must take Region. Hiding or showing the
/// overlay changes the command count, so it must take Full.
fn image_script() -> Vec<(&'static str, Step, bool)> {
    vec![
        ("initial paint with images and shadow", |_| Ok(()), false),
        (
            "selection overlay moved over icon",
            |d| d.set_position_px("sel", 61.0, 60.0),
            true,
        ),
        (
            "selection overlay resized over icon",
            |d| d.set_size_px("sel", 52.0, 44.0),
            true,
        ),
        (
            "icon moved under selection",
            |d| d.set_position_px("icon", 21.0, 21.0),
            true,
        ),
        (
            "selection hidden (command count changes)",
            |d| d.set_visible("sel", false),
            false,
        ),
        (
            "selection shown again (command count changes)",
            |d| d.set_visible("sel", true),
            false,
        ),
        (
            "shadowed label edited",
            |d| d.set_text("label-text", "shadowed label, edited"),
            true,
        ),
        (
            "shadowed label box moved",
            |d| d.set_position_px("label", 104.0, 112.0),
            true,
        ),
    ]
}

/// Desktop-like scene at 1280x800 for timing: an opaque-image wallpaper over a translucent root,
/// a 6x4 grid of icon images, each with a shadowed label, and a translucent selection overlay.
fn bench_fixture() -> RuntimeDocument {
    let mut nodes = vec![CompiledNode {
        kind: NodeKind::Element,
        parent: None,
        first_child: None,
        next_sibling: None,
        id: "root".to_string().into(),
        action: std::borrow::Cow::Borrowed(""),
        text: "".to_string().into(),
        image: None,
        style: Style {
            width: Length::Px(1280.0),
            height: Length::Px(800.0),
            ..Style::default()
        }
        .into(),
        hover_style: None,
        active_style: None,
    }];
    let mut parents = vec![None];
    let push = |nodes: &mut Vec<CompiledNode>,
                parents: &mut Vec<Option<u32>>,
                node: CompiledNode,
                parent: Option<u32>|
     -> u32 {
        nodes.push(node);
        parents.push(parent);
        (nodes.len() - 1) as u32
    };
    let plain =
        |id: String, kind: NodeKind, image: Option<u16>, style: Style, text: &str| CompiledNode {
            kind,
            parent: None,
            first_child: None,
            next_sibling: None,
            id: id.into(),
            action: std::borrow::Cow::Borrowed(""),
            text: text.to_string().into(),
            image,
            style: style.into(),
            hover_style: None,
            active_style: None,
        };
    let absolute = |left: f32, top: f32, width: f32, height: f32| Style {
        position: Position::Absolute,
        left: Length::Px(left),
        top: Length::Px(top),
        width: Length::Px(width),
        height: Length::Px(height),
        ..Style::default()
    };
    push(
        &mut nodes,
        &mut parents,
        plain(
            "wall".into(),
            NodeKind::Image,
            Some(0),
            absolute(0.0, 0.0, 1280.0, 800.0),
            "",
        ),
        Some(0),
    );
    for row in 0..4u32 {
        for col in 0..6u32 {
            let (x, y) = (60.0 + col as f32 * 150.0, 60.0 + row as f32 * 170.0);
            let icon = push(
                &mut nodes,
                &mut parents,
                plain(
                    format!("icon-{row}-{col}"),
                    NodeKind::Image,
                    Some(1),
                    absolute(x + 40.0, y, 48.0, 48.0),
                    "",
                ),
                Some(0),
            );
            let label = push(
                &mut nodes,
                &mut parents,
                plain(
                    format!("label-{row}-{col}"),
                    NodeKind::Element,
                    None,
                    absolute(x, y + 56.0, 128.0, 20.0),
                    "",
                ),
                Some(0),
            );
            let text_style = Style {
                font_size: 13.0,
                text_shadow: Some(TextShadow {
                    offset_x: 1.0,
                    offset_y: 1.0,
                    blur_radius: 2.0,
                    color: ColorValue::Literal(Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 160,
                    }),
                }),
                ..Style::default()
            };
            push(
                &mut nodes,
                &mut parents,
                plain(
                    format!("text-{row}-{col}"),
                    NodeKind::Text,
                    None,
                    text_style,
                    "Item label",
                ),
                Some(label),
            );
            let _ = icon;
        }
    }
    push(
        &mut nodes,
        &mut parents,
        plain(
            "sel".into(),
            NodeKind::Element,
            None,
            absolute(100.0, 100.0, 220.0, 150.0),
            "",
        ),
        Some(0),
    );
    // Link children in document order: first_child is the first node whose parent is `p`.
    for index in 0..nodes.len() {
        let parent = parents[index];
        nodes[index].parent = parent;
        if let Some(p) = parent {
            let siblings: Vec<usize> = (0..nodes.len())
                .filter(|&j| parents[j] == Some(p))
                .collect();
            let pos = siblings.iter().position(|&j| j == index).expect("sibling");
            nodes[index].next_sibling = siblings.get(pos + 1).map(|&j| j as u32);
            if pos == 0 {
                nodes[p as usize].first_child = Some(index as u32);
            }
        }
    }
    let mut doc = RuntimeDocument::new(CompiledDocument {
        source_fingerprint: 0,
        root: 0,
        variables: Vec::new(),
        assets: vec![
            ImageAsset {
                source: "wall.png".into(),
                width: 8,
                height: 8,
                pixels: (0..64u32)
                    .flat_map(|i| [(i * 4) as u8, 90, 160, 255])
                    .collect::<Vec<u8>>()
                    .into(),
            },
            ImageAsset {
                source: "icon.png".into(),
                width: 8,
                height: 8,
                pixels: (0..64u32)
                    .flat_map(|i| [(i * 4) as u8, 200, 90, 128])
                    .collect::<Vec<u8>>()
                    .into(),
            },
        ],
        nodes,
    })
    .expect("bench fixture validates");
    doc.set_background_color(
        "root",
        Color {
            r: 40,
            g: 44,
            b: 52,
            a: 204,
        },
    )
    .expect("translucent root");
    doc.set_background_color(
        "sel",
        Color {
            r: 60,
            g: 120,
            b: 220,
            a: 100,
        },
    )
    .expect("translucent selection");
    doc
}

/// Timing for the desktop selection drag: per-frame Full and Region redraw cost over the same
/// overlay moves. Ignored by default; run with `FLAMEWM_TEST_DISPLAY=:N cargo test -- --ignored`.
#[test]
#[ignore = "timing benchmark, needs FLAMEWM_TEST_DISPLAY"]
fn selection_overlay_drag_full_vs_region_timing() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration; apps and display are closed below.
        unsafe {
            let config = X11Config {
                width: 1280,
                height: 800,
                backdrop: Some(opacity_80_backdrop()),
                ..X11Config::default()
            };
            let mut full = X11App::new(display, &config).expect("full app");
            let mut region = X11App::new(display, &config).expect("region app");
            full.region_paint = false;
            region.region_paint = true;
            let mut doc_full = bench_fixture();
            let mut doc_region = bench_fixture();
            full.redraw(&doc_full).expect("full warm-up");
            region.redraw(&doc_region).expect("region warm-up");
            let steps = 200u32;
            let (mut full_time, mut region_time) =
                (std::time::Duration::ZERO, std::time::Duration::ZERO);
            let mut region_frames = 0u32;
            for step in 0..steps {
                let x = 100.0 + (step % 300) as f32;
                let y = 100.0 + (step % 40) as f32;
                doc_full.set_position_px("sel", x, y).expect("full move");
                doc_region
                    .set_position_px("sel", x, y)
                    .expect("region move");
                let t = std::time::Instant::now();
                full.redraw(&doc_full).expect("full redraw");
                full_time += t.elapsed();
                let before = region.render_counters.clip_area_px;
                let t = std::time::Instant::now();
                region.redraw(&doc_region).expect("region redraw");
                region_time += t.elapsed();
                if region.render_counters.clip_area_px - before < 1280 * 800 {
                    region_frames += 1;
                }
            }
            eprintln!(
                "selection-timing steps={steps} full_ms_per_frame={:.3} region_ms_per_frame={:.3} region_frames={region_frames}",
                full_time.as_secs_f64() * 1000.0 / f64::from(steps),
                region_time.as_secs_f64() * 1000.0 / f64::from(steps),
            );
            let deferred_full = full.take_deferred_native_libraries();
            let deferred_region = region.take_deferred_native_libraries();
            drop(full);
            drop(region);
            XCloseDisplay(display);
            drop(deferred_full);
            drop(deferred_region);
        }
    });
}

/// Overlay moves and resizes redraw from the patched retained layout: no whole-document layout
/// pass runs for them (render-core proves the patched boxes equal a full compute).
#[test]
fn overlay_geometry_moves_patch_layout_without_full_compute() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration; the app is closed below.
        unsafe {
            let config = X11Config {
                width: 1280,
                height: 800,
                backdrop: Some(opacity_80_backdrop()),
                ..X11Config::default()
            };
            let mut app = X11App::new(display, &config).expect("region app");
            app.region_paint = true;
            let mut doc = bench_fixture();
            app.redraw(&doc).expect("warm-up");
            for step in 0..20u32 {
                let x = 100.0 + step as f32 * 7.0;
                let y = 100.0 + (step % 5) as f32 * 3.0;
                doc.set_position_px("sel", x, y).expect("overlay move");
                let passes = flamewm_render_core::layout_passes_on_this_thread();
                app.redraw(&doc).expect("overlay move redraw");
                assert_eq!(
                    flamewm_render_core::layout_passes_on_this_thread(),
                    passes,
                    "overlay move {step} must patch the retained layout"
                );
                doc.set_size_px("sel", 40.0 + step as f32, 40.0)
                    .expect("overlay resize");
                let passes = flamewm_render_core::layout_passes_on_this_thread();
                app.redraw(&doc).expect("overlay resize redraw");
                assert_eq!(
                    flamewm_render_core::layout_passes_on_this_thread(),
                    passes,
                    "overlay resize {step} must patch the retained layout"
                );
            }
            let deferred = app.take_deferred_native_libraries();
            drop(app);
            XCloseDisplay(display);
            drop(deferred);
        }
    });
}

#[test]
fn region_paint_matches_full_for_images_shadows_and_overlay() {
    with_test_display(|display| {
        // SAFETY: the display is open for the body's duration.
        let region_steps = unsafe {
            compare_region_and_full(
                display,
                image_fixture,
                image_script(),
                Some(opacity_80_backdrop()),
            )
        };
        assert!(
            region_steps >= 5,
            "overlay, icon, and shadow steps must take Region"
        );
    });
}
