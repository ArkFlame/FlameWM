import unittest

from flamewm_desktop_stacking_acceptance import check_desktop_bottom, root_children_top_first

TREE_DESKTOP_BOTTOM = """xwininfo: Window id: 0x230 (the root window) (has no name)

  Root window id: 0x230 (the root window) (has no name)
  Parent window id: 0x0 (none)
     4 children:
     0x1000012 "FlameWM-T06-Canary": ("xterm" "XTerm")  484x316+0+31  +91+91
        1 child:
        0x1000021 (has no name): ()  484x316+0+0  +91+91
     0xc00001 "FlameWM panel": ()  1334x44+8+589  +8+589
     0xc0005e "FlameWM start": ()  550x349+0+244  +0+244
     0xa00001 "FlameWM Desktop": ()  1350x641+0+0  +0+0
"""

class DesktopStackingAcceptanceTest(unittest.TestCase):
    def test_children_are_parsed_topmost_first_and_nested_lines_skipped(self):
        children = root_children_top_first(TREE_DESKTOP_BOTTOM)
        self.assertEqual([c[0] for c in children], ["0x1000012", "0xc00001", "0xc0005e", "0xa00001"])

    def test_desktop_last_in_listing_passes(self):
        ok, message = check_desktop_bottom(TREE_DESKTOP_BOTTOM)
        self.assertTrue(ok, message)

    def test_desktop_above_a_frame_fails(self):
        # Desktop listed first = topmost: the regression the acceptance must reject.
        raised = TREE_DESKTOP_BOTTOM.replace(
            '     0xa00001 "FlameWM Desktop": ()  1350x641+0+0  +0+0\n', ""
        ).replace(
            "     4 children:\n",
            '     4 children:\n     0xa00001 "FlameWM Desktop": ()  1350x641+0+0  +0+0\n',
        )
        ok, message = check_desktop_bottom(raised)
        self.assertFalse(ok)
        self.assertIn("not bottom-most", message)

    def test_missing_desktop_fails(self):
        ok, message = check_desktop_bottom(TREE_DESKTOP_BOTTOM.replace("FlameWM Desktop", "other"))
        self.assertFalse(ok)
        self.assertIn("found 0", message)


if __name__ == "__main__":
    unittest.main()
