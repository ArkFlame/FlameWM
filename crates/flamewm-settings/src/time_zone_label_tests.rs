use super::pick_time_zone;

#[test]
fn cached_auto_zone_is_ignored_when_automatic_switch_is_off() {
    assert_eq!(
        pick_time_zone(false, "Asia/Tokyo", "Europe/Paris"),
        "Europe/Paris"
    );
}

#[test]
fn cached_auto_zone_is_shown_when_automatic_switch_is_on() {
    assert_eq!(
        pick_time_zone(true, "Asia/Tokyo", "Europe/Paris"),
        "Asia/Tokyo"
    );
    assert_eq!(pick_time_zone(true, "", "Europe/Paris"), "Europe/Paris");
}
