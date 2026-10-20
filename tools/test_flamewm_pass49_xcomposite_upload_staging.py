"""Pass 49 source contracts for bounded X11 transition uploads.

Compilation, XPutImage correctness, and measured memory use require native tests.
"""
import os
from pathlib import Path
import unittest

ROOT = Path(os.getenv('FLAMEWM_SOURCE_ROOT', Path(__file__).resolve().parents[1]))

class TransitionUploadContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = (ROOT / 'crates/flamewm-render-x11/src/xcomposite.rs').read_text()

    def test_upload_uses_one_reused_staging_strip_not_full_capture(self):
        scope = self.source.split('pub(crate) unsafe fn upload_pixmap_depth(', 1)[1].split('#[cfg(test)]', 1)[0]
        self.assertIn('const MAX_UPLOAD_STAGING_BYTES: usize = 4 * 1024 * 1024;', self.source)
        self.assertIn('let strip_height = upload_strip_height(capture.width, capture.height)', scope)
        self.assertIn('let strip_bytes = row_bytes * strip_height as usize', scope)
        self.assertIn('malloc(strip_bytes)', scope)
        self.assertNotIn('malloc(len)', scope)
        self.assertIn('capture.argb[start..start + bytes].as_ptr()', scope)
        self.assertIn('y as c_int, capture.width, rows', scope)
        self.assertIn('XDestroyImage(image)', scope)

    def test_validation_and_failure_cleanup_retained(self):
        scope = self.source.split('pub(crate) unsafe fn upload_pixmap_depth(', 1)[1].split('#[cfg(test)]', 1)[0]
        self.assertIn('check_capture_size(capture.width, capture.height)', scope)
        self.assertIn('capture.argb.len() != len', scope)
        self.assertIn('XFreeGC(display, gc)', scope)
        self.assertIn('XFreePixmap(display, pixmap)', scope)
        self.assertIn('free(data as *mut c_void)', scope)
        self.assertIn('upload_staging_strips_cover_capture_without_truncating_last_row', self.source)

    def test_existing_shared_consumers_unchanged(self):
        self.assertIn('unsafe { upload_pixmap_depth(display, capture, 32) }', self.source)
        self.assertIn('pub fn rgba_to_argb_owned(', self.source)
        self.assertIn('pub fn capture_window(', self.source)

if __name__ == '__main__':
    unittest.main()
