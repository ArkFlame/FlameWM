//! EWMH `_NET_WM_STRUT(_PARTIAL)` validation shared by the X11 and `XWayland` work-area paths.

/// Validates one strut `[left, right, top, bottom]` against the output size `(width, height)`.
///
/// `ranges` carries the `_NET_WM_STRUT_PARTIAL` `[start, end]` pair of each edge, in the same
/// order. A used edge whose range runs backwards is rejected. Returns `None` when the strut must be
/// ignored: a negative value, an edge larger than the output, or a remaining work area under a
/// quarter of the output (a client cannot reserve most of the screen).
#[must_use]
pub fn sanitize_strut(
    strut: [i32; 4],
    ranges: Option<[[i32; 2]; 4]>,
    output: (i32, i32),
) -> Option<[i32; 4]> {
    let (width, height) = output;
    if width <= 0 || height <= 0 || strut.iter().any(|value| *value < 0) {
        return None;
    }
    let [left, right, top, bottom] = strut;
    if left > width || right > width || top > height || bottom > height {
        return None;
    }
    let work_width = (i64::from(width) - i64::from(left) - i64::from(right)).max(0);
    let work_height = (i64::from(height) - i64::from(top) - i64::from(bottom)).max(0);
    if work_width * work_height * 4 < i64::from(width) * i64::from(height) {
        return None;
    }
    if let Some(ranges) = ranges {
        let backwards = strut
            .iter()
            .zip(ranges)
            .any(|(value, [start, end])| *value > 0 && start > end);
        if backwards {
            return None;
        }
    }
    Some(strut)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (i32, i32) = (1920, 1080);

    #[test]
    fn sane_struts_pass_through() {
        assert_eq!(
            sanitize_strut([0, 0, 0, 44], None, SCREEN),
            Some([0, 0, 0, 44])
        );
        assert_eq!(sanitize_strut([0; 4], None, SCREEN), Some([0; 4]));
    }

    #[test]
    fn negative_oversize_and_collapsed_work_area_are_rejected() {
        assert_eq!(sanitize_strut([0, 0, -5, 44], None, SCREEN), None);
        assert_eq!(sanitize_strut([0, 0, 0, 1081], None, SCREEN), None);
        assert_eq!(sanitize_strut([1921, 0, 0, 0], None, SCREEN), None);
        // Leaves 1920x100 of 1920x1080 is under a quarter of the output.
        assert_eq!(sanitize_strut([0, 0, 490, 490], None, SCREEN), None);
        assert_eq!(sanitize_strut([0, 0, 0, 1080], None, (0, 0)), None);
    }

    #[test]
    fn partial_ranges_must_not_run_backwards_for_used_edges_only() {
        let good = [[0, 1079], [0, 1079], [0, 1919], [0, 1919]];
        assert_eq!(
            sanitize_strut([0, 0, 0, 44], Some(good), SCREEN),
            Some([0, 0, 0, 44])
        );
        let backwards = [[0, 1079], [0, 1079], [0, 1919], [1500, 100]];
        assert_eq!(sanitize_strut([0, 0, 0, 44], Some(backwards), SCREEN), None);
        assert_eq!(
            sanitize_strut([0, 0, 0, 0], Some(backwards), SCREEN),
            Some([0; 4])
        );
    }
}
