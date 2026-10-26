use super::*;
#[test]
fn window_stays_above_taskbar_reserve() {
    for root in [(1350, 641), (1600, 900), (1920, 1080), (800, 600)] {
        let (w, h, y) = window_frame(root);
        // The framed window (client plus WM chrome) must end above the taskbar reserve.
        assert!(
            y >= 0 && (y as u32) + h + FRAME_CHROME_HEIGHT <= root.1 - TASKBAR_RESERVE,
            "root {root:?} window {w}x{h} at {y}"
        );
        assert!(w <= root.0);
    }
    assert_eq!(window_frame((1920, 1080)), (1250, 740, 65));
    assert_eq!(window_frame((1350, 641)), (1215, 493, 59));
}
