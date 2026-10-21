"""Keep Settings scale-value CSS compatible with FlameWM’s strict RWR compiler."""

from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


def declarations(css: str, selector: str) -> dict[str, str]:
    match = re.search(r"(?:^|})\s*" + re.escape(selector) + r"\s*\{([^{}]*)\}", css)
    if match is None:
        raise AssertionError(f"missing CSS selector: {selector}")
    return dict(part.strip().split(":", 1) for part in match.group(1).split(";") if part.strip())


class SettingsScaleValueCSS(unittest.TestCase):
    def test_scale_label_uses_supported_centering(self):
        css = (ROOT / "ui/settings.css").read_text()
        html = (ROOT / "ui/settings.html").read_text()
        compiler = (ROOT / "crates/flamewm-render-compiler/src/compile.rs").read_text()
        label = declarations(css, "#display-scale-value")
        pill = declarations(css, ".value-pill")
        self.assertNotIn("text-align", css)
        self.assertEqual(label["min-width"], "52px")
        self.assertRegex(html, r'class="value-pill value" id="display-scale-value"')
        for prop, expected in (("display", "flex"), ("align-items", "center"), ("justify-content", "center")):
            self.assertEqual(pill[prop].strip(), expected)
            self.assertIn(f'"{prop}" =>', compiler)

    def test_all_settings_stylesheet_properties_are_compiler_supported(self):
        css = (ROOT / "ui/settings.css").read_text()
        compiler = (ROOT / "crates/flamewm-render-compiler/src/compile.rs").read_text()
        start = compiler.index("fn apply_declaration(")
        end = compiler.index("fn unsupported(", start)
        known = set(re.findall(r'"([a-z-]+)"', compiler[start:end]))
        names = {name for body in re.findall(r"\{([^{}]*)\}", css)
                 for name in re.findall(r"(?:^|;)\s*([a-z-]+)\s*:", body)
                 if not name.startswith("--")}
        self.assertFalse(names - known, f"Unsupported Settings CSS properties: {sorted(names - known)}")


if __name__ == "__main__":
    unittest.main()
