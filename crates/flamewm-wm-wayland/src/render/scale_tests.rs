use super::{physical_point, physical_radius};

#[test]
fn default_scale_is_identity_for_points_and_radii() {
    assert_eq!(physical_point(-7, 1366, 1.0), (-7, 1366).into());
    for radius in [0_u8, 1, 12, 255] {
        assert_eq!(physical_radius(radius, 1.0), radius);
    }
}

#[test]
fn fractional_scale_rounds_points_and_radii_to_physical_pixels() {
    assert_eq!(physical_point(1000, 500, 1.25), (1250, 625).into());
    assert_eq!(physical_point(3, 3, 1.25), (4, 4).into());
    assert_eq!(physical_radius(12, 1.25), 15);
    assert_eq!(physical_radius(250, 2.0), 255);
}
