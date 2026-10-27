"""Reject unsupported Task Manager CSS before invoking the long release build.

The native flamewm-render-compiler build.rs remains the authoritative parser.
This preflight checks every declaration against that compiler's property subset.
"""
from __future__ import annotations

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CSS = ROOT / "ui/task-manager/task-manager.css"
SUPPORTED = frozenset((
    "display flex-direction justify-content align-items position width height min-width "
    "min-height max-width max-height top right bottom left margin margin-top margin-right "
    "margin-bottom margin-left padding padding-top padding-right padding-bottom padding-left "
    "gap flex-grow flex background background-color color border-color border-width "
    "border-radius border font-size font-weight text-shadow opacity cursor white-space "
    "overflow-wrap word-wrap box-sizing overflow overflow-x overflow-y scrollbar-width "
    "user-select -flamewm-image-treatment"
).split())


def unsupported_properties(source: str) -> list[str]:
    properties = []
    for body in re.findall(r"\{([^{}]*)\}", source):
        for declaration in body.split(";"):
            if ":" not in declaration:
                continue
            name, value = declaration.split(":", 1)
            name, value = name.strip(), value.strip()
            if name.startswith("--"):
                continue
            # The native compiler accepts `border: <px> solid <color>`, not
            # browser shorthands like `border: 0` or `border: none`.
            border = value.split() if name == "border" else ()
            invalid_border = name == "border" and (
                len(border) != 3
                or border[1] != "solid"
                or not re.fullmatch(r"(?:0|[0-9]+(?:\.[0-9]+)?px)", border[0])
                or not (border[2].startswith("var(") and border[2].endswith(")")
                        or re.fullmatch(r"#[0-9a-fA-F]{3,8}", border[2])
                        or border[2] in {"transparent", "black", "white", "red", "gray", "grey"})
            )
            invalid_width = name == "border-width" and not re.fullmatch(
                r"(?:0|[0-9]+(?:\.[0-9]+)?px)", value
            )
            if name not in SUPPORTED or not value or invalid_border or invalid_width or (
                name.startswith(("margin", "padding")) and re.search(r"\bauto\b", value)
            ):
                properties.append(name + ":" + value)
    return properties


class NativeTaskManagerCssTests(unittest.TestCase):
    def test_entire_task_manager_css_is_in_renderer_subset(self):
        self.assertEqual(unsupported_properties(CSS.read_text()), [])

    def test_detects_each_original_compile_failure(self):
        bad = ".root { border-bottom: 1px solid red; text-align: left; margin-left: auto; }"
        self.assertEqual(len(unsupported_properties(bad)), 3)

    def test_border_reset_is_validated_at_value_level(self):
        self.assertEqual(unsupported_properties("button { border: 0; }"), ["border:0"])
        self.assertEqual(unsupported_properties("button { border: none; }"), ["border:none"])
        self.assertEqual(unsupported_properties("button { border-width: none; }"), ["border-width:none"])
        self.assertEqual(unsupported_properties("button { border-width: 0; }"), [])
        self.assertEqual(unsupported_properties("button { border: 1px solid var(--border); }"), [])
        self.assertIn("button { background:transparent; color:var(--text); cursor:default; border-width:0;", CSS.read_text())

    def test_xephyr_enforces_both_preflights_before_release_build(self):
        script = (ROOT / "scripts/xephyr").read_text()
        css = script.index("tools/test_task_manager_rwr_css.py")
        cache = script.index("tools/ensure_shared_surface_cargo.py")
        build = script.index("cargo build --quiet --release")
        self.assertLess(css, cache)
        self.assertLess(cache, build)

    def test_header_uses_supported_flex_spacer(self):
        self.assertIn('class="header-spacer"', (ROOT / "ui/task-manager/task-manager.html").read_text())
        self.assertIn(".header-spacer { flex-grow:1; }", CSS.read_text())


if __name__ == "__main__":
    unittest.main()
