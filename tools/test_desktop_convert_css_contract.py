"""Regression gate for FlameWM's strict compiler and conversion-wheel stylesheet.

The RWR compiler does not support CSS `text-align`; use its canonical flex
layout properties instead, so the desktop build script cannot panic.
"""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[1]
CSS = ROOT / "ui/desktop/desktop.css"
SELECTOR = ".context-menu .convert-wheel-option"


def declarations_for(css: str, selector: str) -> dict[str, str]:
    pattern = re.compile(r"(?:^|\})\s*" + re.escape(selector) + r"\s*\{([^{}]*)\}", re.M)
    match = pattern.search(css)
    if not match:
        raise ValueError(f"missing stylesheet rule: {selector}")
    declarations = {}
    for item in match.group(1).split(";"):
        if item.strip():
            name, value = item.split(":", 1)
            declarations[name.strip()] = value.strip()
    return declarations


class ConvertWheelCssContract(unittest.TestCase):
    def test_convert_options_center_labels_using_compiler_supported_flex(self):
        declarations = declarations_for(CSS.read_text(encoding="utf-8"), SELECTOR)
        self.assertNotIn("text-align", declarations)
        self.assertEqual(declarations.get("display"), "flex")
        self.assertEqual(declarations.get("justify-content"), "center")
        self.assertEqual(declarations.get("align-items"), "center")

    def test_compiler_exposes_native_flex_alignment_properties(self):
        source = (ROOT / "crates/flamewm-render-compiler/src/compile.rs").read_text(encoding="utf-8")
        self.assertIn('"justify-content" =>', source)
        self.assertIn('"align-items" =>', source)
        self.assertIn('"flex" => Display::Flex', source)


if __name__ == "__main__":
    unittest.main()
