"""Source-level guards for Pass 48's non-wallpaper allocation reductions.

Actual Rust equivalence tests are in flamewm-screenshot/src/capture.rs;
these checks do not establish native-compilation or RSS measurements.
"""
import os
import unittest
from pathlib import Path

ROOT = Path(os.getenv("FLAMEWM_SOURCE_ROOT", Path(__file__).resolve().parents[1]))

def source(name):
    return (ROOT / name).read_text()

class NonWallpaperAllocationContracts(unittest.TestCase):
    def test_screenshot_getimage_reply_is_consumed_by_owned_decoder(self):
        capture = source('crates/flamewm-screenshot/src/capture.rs')
        ui = source('crates/flamewm-screenshot/src/ui.rs')
        self.assertIn('decode_ximage_owned(conn, screen, reply.depth, rect.w, rect.h, reply.data)', capture)
        self.assertIn('decode_ximage_owned(self.conn,self.screen,result.depth,u32::from(self.width),u32::from(self.height),result.data)', ui)
        self.assertNotIn('decode_ximage(conn, screen, reply.depth, rect.w, rect.h, &reply.data)', capture)

    def test_32_bit_reply_reuses_allocation_but_odd_depths_fall_back(self):
        capture = source('crates/flamewm-screenshot/src/capture.rs')
        scope = capture.split('pub fn decode_ximage_owned<C: Connection>', 1)[1].split('pub fn grab<C: Connection>', 1)[0]
        self.assertIn('mut data: Vec<u8>', scope)
        self.assertIn('format.bits_per_pixel == 32', scope)
        self.assertIn('data.len() == len', scope)
        self.assertIn('decode_32bpp_in_place(&mut data', scope)
        self.assertIn('rgba: data', scope)
        self.assertIn('decode_ximage(conn, screen, depth, width, height, &data)', scope)
        self.assertIn('owned_ximage_fast_path_matches_mask_decoding_and_reuses_pixels', capture)
        self.assertIn('owned_ximage_fast_path_preserves_nonstandard_channel_masks', capture)

    def test_xrender_staging_allocated_only_when_visual_available(self):
        images = source('crates/flamewm-render-x11/src/app/images.rs')
        scope = images.split('pub(super) unsafe fn image_pixmap(', 1)[1].split('pub(crate) unsafe fn upload_argb32_words(', 1)[0]
        self.assertGreater(scope.index('let argb = match treatment'), scope.index('if matched != 0 && !vinfo.visual.is_null()'))
        self.assertLess(scope.index('let argb = match treatment'), scope.index('self.upload_argb32_words('))
        self.assertLess(scope.index('let argb = match treatment'), scope.index('// Degraded path (no XRender)'))
        self.assertEqual(scope.count('let argb = match treatment'), 1)

    def test_other_existing_source_optimizations_preserved(self):
        self.assertIn('pub fn crop(mut self, rect: Rect)', source('crates/flamewm-screenshot/src/capture.rs'))
        self.assertIn('const MAX_IMAGE_CACHE_BYTES: usize = 16 * 1024 * 1024;', source('crates/flamewm-render-x11/src/app/images.rs'))
        self.assertIn('const ICON_CACHE_BYTE_BUDGET: usize = 32 * 1024 * 1024;', source('crates/flamewm-shell/src/async_projection.rs'))
        self.assertIn('pub const CAPTURE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;', source('crates/flamewm-wm-x11/src/animation.rs'))

if __name__ == '__main__':
    unittest.main()
