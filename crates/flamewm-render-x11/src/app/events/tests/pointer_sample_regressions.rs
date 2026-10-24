use super::{Rect, pointer_sample_point};

#[test]
fn fractional_track_top_row_is_hit_at_pixel_center() {
    // AP06 measured: the animation slider track laid out at y=451.30 and the
    // pointer pixel row 451 was painted as the track but missed the hit test.
    let track = Rect {
        x: 528.0,
        y: 451.30005,
        width: 180.0,
        height: 12.0,
    };
    assert!(
        !track.contains(531.0, 451.0),
        "corner sample misses the track"
    );
    let (x, y) = pointer_sample_point(531.0, 451.0, 1.0);
    assert!(
        track.contains(x, y),
        "pixel-center sample must hit the track"
    );
}

#[test]
fn pixel_center_sampling_scales_device_pixels_to_logical_points() {
    assert_eq!(pointer_sample_point(10.0, 20.0, 2.0), (5.25, 10.25));
}
