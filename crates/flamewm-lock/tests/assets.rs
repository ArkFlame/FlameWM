use std::fs;
use std::path::Path;

const LOCK_ICONS: [&str; 5] = [
    "system-suspend.svg",
    "system-reboot.svg",
    "system-shutdown.svg",
    "system-switch-user.svg",
    "system-log-out.svg",
];

fn staged() -> &'static Path {
    Path::new(env!("FLAMEWM_LOCK_STAGED_ASSETS"))
}

#[test]
fn staged_lock_icons_exist_and_are_near_white() {
    for icon in LOCK_ICONS {
        let path = staged().join("breeze").join(icon);
        let svg = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("missing staged lock icon {}: {error}", path.display()));
        assert!(svg.contains("color:#eaf0ff"), "{icon} is not lock-coloured");
        assert!(svg.contains("viewBox=\"0 0 22 22\""), "{icon} viewBox changed");
    }
}

#[test]
fn staged_lock_avatar_outline_exists() {
    let path = staged().join("lock").join("avatar-outline.svg");
    let svg = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("missing staged avatar {}: {error}", path.display()));
    assert!(svg.contains("viewBox=\"0 0 68 68\""));
    assert!(svg.contains("stroke=\"#FFFFFF\""));
    assert!(svg.contains("r=\"33.15\""));
}
