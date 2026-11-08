//! Shared human-readable labels for product surfaces.

/// Byte count as a short label with binary units (B, KiB, MiB, GiB, TiB).
#[must_use]
pub fn human_size(bytes: u64) -> String {
    let mut n = bytes as f64;
    let mut unit = "B";
    for candidate in ["KiB", "MiB", "GiB", "TiB"] {
        if n < 1024.0 {
            break;
        }
        n /= 1024.0;
        unit = candidate;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{n:.1} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_uses_binary_units() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(2048), "2.0 KiB");
    }
}
