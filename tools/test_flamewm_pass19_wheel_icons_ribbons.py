"""Offline source-contract regressions for the FlameWM Pass 19 fixes.

The native Rust/X11 harness is the final authority; these checks ensure the
wheel geometry, icon assets and lock-only ribbon preset survive packaging.
"""
from __future__ import annotations

import math
import re
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def source(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


class ConvertWheelSourceTests(unittest.TestCase):
    def test_all_eight_format_label_centers_are_on_one_symmetric_ring(self):
        css = source("ui/desktop/desktop.css")
        boxes = []
        for i in range(8):
            m = re.search(r"\.convert-wheel-pos-%s\s*\{\s*left:(\d+)px;\s*top:(\d+)px;" % i, css)
            self.assertIsNotNone(m, f"format {i} must have a wheel location")
            boxes.append((int(m[1]) + 40, int(m[2]) + 16))
        for i, (x, y) in enumerate(boxes):
            angle = i * math.pi / 4
            expected = (164 + 113 * math.sin(angle), 164 - 113 * math.cos(angle))
            self.assertLess(math.dist((x, y), expected), 3, (i, (x, y), expected))

    def test_text_and_center_hub_are_heavier_without_pills(self):
        css = source("ui/desktop/desktop.css")
        self.assertRegex(css, r"\.convert-wheel-hub[^}]+font-weight:700")
        self.assertRegex(css, r"\.context-menu \.convert-wheel-option[^}]+font-weight:650")
        self.assertRegex(css, r"\.context-menu \.convert-wheel-option[^}]+background:transparent")

    def test_x11_hover_not_just_node_transition(self):
        events = source("crates/flamewm-render-x11/src/app/events.rs")
        after = events[events.index("// Hover is a *pointer-position* event") :]
        self.assertIn("ActionPhase::Hover", after)
        self.assertIn("if changed { self.mark_full(); }", after)
        self.assertIn("fire_node(on_action, document, &mut buffers, index, &mut event)?", after)

    def test_wheel_hit_test_shared_for_hover_and_click(self):
        main = source("crates/flamewm-desktop/src/main.rs")
        # The wheel's own keyboard block sits inside this handler; the boundary is the top-level keyboard block.
        handling = main[main.index("fn handle_event(") : main.index('\n    if action.action == "keyboard.input"', main.index("fn handle_event("))]
        self.assertGreaterEqual(handling.count("wheel_hit_at(state, action.x, action.y)"), 2)
        self.assertIn("update_wheel_hover(document, state, sector)", handling)
        self.assertIn("handle_menu_action(&format!(\"convert.slot.{index}\")", handling)


class MenuIconsSourceTests(unittest.TestCase):
    ICONS = ("flamewm-convert", "flamewm-new-folder", "flamewm-add-workspace")

    def test_icons_are_owned_staged_and_referenced_by_menu(self):
        build = source("crates/flamewm-desktop/build.rs")
        html = source("ui/desktop/index.html")
        self.assertIn('root.join("ui/desktop/icons")', build)
        self.assertIn('staged_assets.join("breeze").join(icon)', build)
        for name in self.ICONS:
            self.assertIn(f'{name}.svg', build)
            self.assertIn(f'assets/breeze/{name}.svg', html)
            svg = ET.parse(ROOT / f"ui/desktop/icons/{name}.svg").getroot()
            self.assertEqual(svg.attrib["viewBox"], "0 0 24 24")
            self.assertTrue(len(svg) > 0)
        self.assertIn('id="menu-row-convert" class="menu-row"', html)
        self.assertNotIn("Convert ▸", html)


class RibbonsSourceTests(unittest.TestCase):
    def test_curvy_motion_preset_is_only_selected_by_lock_screen(self):
        sim = source("crates/flamewm-ribbons/src/sim.rs")
        lock = source("crates/flamewm-lock/src/ribbons.rs")
        self.assertIn("pub fn new_fluid_style(config: Config, now: f64) -> Self", sim)
        self.assertIn("bend_factor: 1.0,", sim)
        self.assertIn("this.bend_factor = 1.70;", sim)
        self.assertIn("Ribbons::new_fluid_style(config, 0.0)", lock)


if __name__ == "__main__":
    unittest.main()
