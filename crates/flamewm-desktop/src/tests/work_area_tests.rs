use super::{
    desktop_surface_rect, grids_for_outputs, normalize_outputs, output_at_in,
    pointer_point_for_grid, subtract_panel_strip,
};
use flamewm_api::{OutputId, Point, Rect};
use flamewm_desktop_core::layout::GridConfig;
use std::collections::BTreeMap;

#[test]
fn desktop_surface_covers_full_root_under_bottom_strut() {
    let work_area = Rect::new(0, 0, 1280, 756);
    let surface = desktop_surface_rect(1280, 800);

    assert_eq!(surface, Rect::new(0, 0, 1280, 800));
    assert_eq!((surface.width as u32, surface.height as u32), (1280, 800));
    assert!(surface.width >= work_area.width && surface.height > work_area.height);
}

#[test]
fn panel_edges_keep_icons_in_work_area_with_surface_origin_at_root() {
    // The surface sits at root origin, so window-local == screen. Cells must map inside the
    // work area for top, left, bottom and right struts, and hit-testing must invert them.
    let surface = desktop_surface_rect(1280, 800);
    assert_eq!(surface.origin(), flamewm_api::Point::new(0, 0));
    for work_area in [
        Rect::new(0, 44, 1280, 756),
        Rect::new(44, 0, 1236, 800),
        Rect::new(0, 0, 1280, 756),
        Rect::new(0, 0, 1236, 800),
    ] {
        let grid = GridConfig::compute(work_area, 100);
        for column in 0..grid.columns {
            for row in 0..grid.rows {
                let cell = flamewm_desktop_core::layout::Cell::new(column, row);
                let point = grid.cell_to_pixel(cell);
                assert!(work_area.contains(point), "{point:?} outside {work_area:?}");
                assert!(surface.contains(point));
                assert_eq!(grid.pixel_to_cell(point), cell);
            }
        }
    }
}

#[test]
fn icon_grid_stays_inside_work_area_while_surface_is_root() {
    let work_area = Rect::new(0, 0, 1280, 756);
    let grid = GridConfig::compute(work_area, 100);
    let last = grid.clamp_cell(flamewm_desktop_core::layout::Cell::new(i32::MAX, i32::MAX));
    let point = grid.cell_to_pixel(last);

    assert!(work_area.contains(point));
    assert_eq!(grid.work_area, work_area);
}

#[test]
fn bottom_panel_strip_leaves_root_minus_panel_height_at_origin() {
    let root = Rect::new(0, 0, 1350, 641);
    let panel = Rect::new(0, 641 - 55, 1350, 55);

    assert_eq!(
        subtract_panel_strip(root, panel),
        Rect::new(0, 0, 1350, 586)
    );
}

#[test]
fn each_output_uses_its_own_taskbar_work_area() {
    let left = OutputId::new("left");
    let right = OutputId::new("right");
    let outputs = [
        (left.clone(), Rect::new(0, 0, 1920, 1080)),
        (right.clone(), Rect::new(1920, 200, 2560, 1440)),
    ];
    let panels = [
        (left.clone(), Rect::new(4, 1036, 1912, 44)),
        (right.clone(), Rect::new(1924, 1596, 2552, 44)),
    ];
    let grids = grids_for_outputs(&outputs, &panels, &BTreeMap::new(), 100);

    assert_eq!(grids[&left].work_area, Rect::new(0, 0, 1920, 1036));
    assert_eq!(grids[&right].work_area, Rect::new(1920, 200, 2560, 1396));
    assert!(grids[&left].work_area.bottom() <= panels[0].1.y);
    assert!(grids[&right].work_area.bottom() <= panels[1].1.y);
}

#[test]
fn distinct_output_scales_keep_independent_grid_and_panel_bounds() {
    let left = OutputId::new("left");
    let right = OutputId::new("right");
    let outputs = [
        (left.clone(), Rect::new(0, 0, 1920, 1080)),
        (right.clone(), Rect::new(1920, 0, 1920, 1080)),
    ];
    let panels = [
        (left.clone(), Rect::new(0, 1030, 1920, 50)),
        (right.clone(), Rect::new(1920, 1030, 1920, 50)),
    ];
    let scales = BTreeMap::from([(left.clone(), 105), (right.clone(), 195)]);
    let grids = grids_for_outputs(&outputs, &panels, &scales, 100);
    assert_eq!(grids[&left].scale_percent, 105);
    assert_eq!(grids[&right].scale_percent, 195);
    assert_ne!(grids[&left].cell_width, grids[&right].cell_width);
    assert_eq!(grids[&left].work_area.bottom(), 1030);
    assert_eq!(grids[&right].work_area.bottom(), 1030);
    for grid in grids.values() {
        let origin = grid.cell_to_pixel(flamewm_desktop_core::layout::Cell::new(0, 0));
        assert!(grid.work_area.contains(origin));
    }
}

#[test]
fn entry_scale_preference_grows_cells_without_changing_output_scale() {
    let output = OutputId::new("primary");
    let outputs = [(output.clone(), Rect::new(0, 0, 1920, 1080))];
    let grids = grids_for_outputs(&outputs, &[], &BTreeMap::new(), 150);
    let grid = grids[&output];
    assert_eq!(grid.scale_percent, 100);
    assert_eq!(grid.entry_scale_percent, 150);
    assert_eq!(grid.visual_scale_percent(), 150);
    assert_eq!(grid.cell_width, 144);
}

#[test]
fn negative_monitor_origin_is_normalized_and_pointer_selects_its_output() {
    let left = OutputId::new("left");
    let right = OutputId::new("right");
    let raw = [
        (left.clone(), Rect::new(-1920, 0, 1920, 1080), false),
        (right.clone(), Rect::new(0, 200, 2560, 1440), true),
    ];
    let root = Rect::new(0, 0, 4480, 1640);
    let (outputs, origin_x, origin_y) = normalize_outputs(&raw, root);

    assert_eq!((origin_x, origin_y), (-1920, 0));
    assert_eq!(outputs[0].1, Rect::new(0, 0, 1920, 1080));
    assert_eq!(outputs[1].1, Rect::new(1920, 200, 2560, 1440));
    assert_eq!(output_at_in(&outputs, &left, Point::new(2500, 300)), right);
    assert_eq!(output_at_in(&outputs, &left, Point::new(1919, 100)), left);
    assert_eq!(output_at_in(&outputs, &left, Point::new(4600, 100)), right);
    let work = Rect::new(1920, 200, 2560, 1396);
    assert_eq!(
        pointer_point_for_grid(Point::new(2500, 1640), GridConfig::compute(work, 100)),
        Point::new(2500, 1595)
    );
}

#[test]
fn panel_height_change_updates_output_grid() {
    let root = Rect::new(0, 0, 1350, 641);
    let before = subtract_panel_strip(root, Rect::new(0, 641 - 44, 1350, 44));
    let after = subtract_panel_strip(root, Rect::new(0, 641 - 55, 1350, 55));

    assert_eq!(before, Rect::new(0, 0, 1350, 597));
    assert_eq!(after, Rect::new(0, 0, 1350, 586));
}
