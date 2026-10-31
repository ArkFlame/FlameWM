use super::*;

#[test]
fn glass_zone_is_translucent_black_overlay() {
    let fill = card_fill(false, 0x0018_253e);
    assert_eq!((fill.r, fill.g, fill.b, fill.a), (0, 0, 0, 0x33));
}

#[test]
fn reduce_transparency_zone_is_opaque_darker_body() {
    let fill = card_fill(true, 0x00ff_ffff);
    assert_eq!((fill.r, fill.g, fill.b, fill.a), (204, 204, 204, 255));
}
