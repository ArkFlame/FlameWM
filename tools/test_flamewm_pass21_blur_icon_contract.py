"""Regression contracts for the Pass 21 compiler repair and cache policy.

Native Rust tests remain authoritative for raster output and allocation behavior.
"""
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def source(rel):
    return (ROOT / rel).read_text()


class BlurIconPass21Contracts(unittest.TestCase):
    def test_blur_capacity_expression_has_balanced_delimiters(self):
        blur = source("crates/flamewm-image-core/src/blur.rs")
        method = blur.split("pub fn release_if_over_budget", 1)[1].split("/// Blurs straight", 1)[0]
        self.assertIn("std::mem::size_of::<(usize, usize, f32)>()", method)
        self.assertNotIn("std::mem::size_of::<(usize, usize, f32)>())))", method)
        self.assertIn("while retained > max_bytes", method)
        self.assertIn("let Some((index, &bytes))", method)
        self.assertIn("fn release_if_over_budget_keeps_useful_small_scratch()", blur)

    def test_shell_cache_retains_original_entry_count_and_32mib_payload(self):
        source_text = source("crates/flamewm-shell/src/async_projection.rs")
        self.assertIn("const ICON_CACHE_ENTRY_BUDGET: usize = 2048;", source_text)
        self.assertIn("const ICON_CACHE_BYTE_BUDGET: usize = 32 * 1024 * 1024;", source_text)
        self.assertIn("fn icon_cache_shares_name_storage_between_icon_sizes()", source_text)

    def test_rasterizer_already_decodes_to_requested_physical_size(self):
        icons = source("crates/flamewm-integrations-linux/src/icons.rs")
        loader = source("crates/flamewm-shell/src/icon_loader.rs")
        self.assertIn("fn scale_to_physical(", icons)
        self.assertIn("natural.width == size.physical && natural.height == size.physical", icons)
        self.assertIn("flamewm_image_core::scale::scale_rgba(", icons)
        self.assertIn("let size = IconSize::new(req.width, req.height);", loader)

    def test_build_guard_contains_both_affected_crates(self):
        guard = source("tools/ensure_shared_surface_cargo.py")
        self.assertIn('"flamewm-image-core"', guard)
        self.assertIn('"flamewm-shell"', guard)


if __name__ == "__main__":
    unittest.main()
