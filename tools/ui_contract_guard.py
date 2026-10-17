#!/usr/bin/env python3
"""Check that UI markup and the Rust code that drives it agree.

Errors (exit 1):
- duplicate `id` values in an HTML document;
- a literal node id passed to a `document.<op>(...)` call, or listed in an `*_IDS`
  const, that the owning HTML does not declare (`format!` templates must match at least one id);
- a `data-action` value that no owning Rust source mentions (exact literal or dotted prefix).

Warnings (exit 0):
- a class used in markup with no selector in the paired stylesheet;
- an HTML document with no owning crate mapping (only duplicate and class checks apply).

Run: python3 -I tools/ui_contract_guard.py
"""

from pathlib import Path
import re
import sys

# html -> (owning Rust source dir or None, stylesheet paths)
DOCUMENTS = {
    "ui/settings.html": ("crates/flamewm-settings/src", ["ui/settings.css"]),
    "ui/desktop/index.html": ("crates/flamewm-desktop/src", ["ui/desktop/desktop.css"]),
    "ui/shell/index.html": (None, ["ui/shell/flamewm.css"]),
    "ui/index.html": (None, ["ui/flamewm.css"]),
}

DOCUMENT_OPS = (
    "text|background|visible|border|position|size|toggle|foreground|font_size|font_weight|"
    "overflow|layer|image_rgb8|image_rgba8|image_reset|corner_radius|node_backdrop|"
    "background_clear|border_clear|foreground_clear"
)
CALL_RE = re.compile(
    r'document\.(?:' + DOCUMENT_OPS + r')\(\s*(?:&)?(?P<arg>format!\(\s*)?"(?P<lit>[^"]*)"'
)
CONST_RE = re.compile(r"const\s+(\w*(?:_IDS|NODE\w*))\s*:[^=]*=\s*&\[(?P<body>.*?)\];", re.S)
STRING_RE = re.compile(r'"([^"]*)"')
ID_RE = re.compile(r'id="([^"]+)"')
# Node templates the desktop build script injects into its document (escaped quotes in Rust source).
GENERATED_ID_RE = re.compile(r'id=\\?"([^"\\]+)\\?"')
GENERATED_SOURCES = {"crates/flamewm-desktop/src": "crates/flamewm-desktop/build.rs"}
ACTION_RE = re.compile(r'data-action="([^"]+)"')
CLASS_ATTR_RE = re.compile(r'class="([^"]*)"')
SELECTOR_RE = re.compile(r"\.([A-Za-z_][\w-]*)")


def html_ids(text):
    return ID_RE.findall(text)


def template_regex(pattern):
    """`display-output-{slot}` -> regex matching any concrete id it can produce."""
    parts = re.split(r"\{[^{}]*\}", pattern)
    return re.compile("^" + ".+".join(re.escape(part) for part in parts) + "$")


CFG_TEST_RE = re.compile(r"#\[cfg\(test\)\]\s*mod \w+\s*(?P<open>[;{])")


def strip_test_modules(text):
    """Drops `#[cfg(test)] mod x;` lines and inline `#[cfg(test)] mod x { ... }` blocks."""
    out, pos = [], 0
    for match in CFG_TEST_RE.finditer(text):
        if match.start() < pos:
            continue
        out.append(text[pos:match.start()])
        if match.group("open") == ";":
            pos = match.end()
            continue
        depth, index = 1, match.end()
        while depth and index < len(text):
            depth += {"{": 1, "}": -1}.get(text[index], 0)
            index += 1
        pos = index
    out.append(text[pos:])
    return "".join(out)


def rust_sources(directory):
    """Production Rust under `directory`, with test modules removed."""
    return [strip_test_modules(path.read_text(encoding="utf-8")) for path in sorted(directory.rglob("*.rs"))]


def check_document(root, html_rel, owner_dir, css_rels, errors, warnings):
    html = (root / html_rel).read_text(encoding="utf-8")
    ids = html_ids(html)
    id_set = set(ids)

    seen = set()
    for node in ids:
        if node in seen:
            errors.append(f"{html_rel}: duplicate id '{node}'")
        seen.add(node)

    css_text = "".join((root / rel).read_text(encoding="utf-8") for rel in css_rels if (root / rel).exists())
    selectors = set(SELECTOR_RE.findall(css_text))
    classes = set()
    for value in CLASS_ATTR_RE.findall(html):
        classes.update(value.split())
    for cls in sorted(classes - selectors):
        warnings.append(f"{html_rel}: class '{cls}' has no selector in {', '.join(css_rels)}")

    if owner_dir is None:
        warnings.append(f"{html_rel}: no owning crate mapping; document-call and action checks skipped")
        return

    generated = []
    if owner_dir in GENERATED_SOURCES:
        generated = GENERATED_ID_RE.findall((root / GENERATED_SOURCES[owner_dir]).read_text(encoding="utf-8"))
    generated_templates = [template_regex(node) for node in generated if "{" in node]
    sources = rust_sources(root / owner_dir)
    joined = "\n".join(sources)
    for source in sources:
        for match in CALL_RE.finditer(source):
            literal = match.group("lit")
            if match.group("arg"):
                pattern = template_regex(literal)
                built = literal in generated or any(pattern.match(node) for node in generated)
                if not built and not any(pattern.match(node) for node in ids):
                    errors.append(f"{owner_dir}: format id '{literal}' matches no id in {html_rel}")
            elif literal not in id_set and not any(pattern.match(literal) for pattern in generated_templates):
                errors.append(f"{owner_dir}: document call uses id '{literal}' missing from {html_rel}")
        for const in CONST_RE.finditer(source):
            for node in STRING_RE.findall(const.group("body")):
                if node not in id_set:
                    errors.append(f"{owner_dir}: const {const.group(1)} lists id '{node}' missing from {html_rel}")

    literals = [literal for literal in STRING_RE.findall(joined) if "." in literal]
    templates = [template_regex(literal) for literal in literals if "{" in literal]
    for action in sorted(set(ACTION_RE.findall(html))):
        handled = any(
            action == literal or (literal.endswith(".") and action.startswith(literal))
            for literal in literals
        ) or any(pattern.match(action) for pattern in templates)
        if not handled:
            errors.append(f"{html_rel}: data-action '{action}' has no handler in {owner_dir}")


def main(root=None):
    root = Path(root) if root else Path(__file__).resolve().parents[1]
    errors, warnings = [], []
    for html_rel, (owner_dir, css_rels) in DOCUMENTS.items():
        if not (root / html_rel).exists():
            errors.append(f"missing document {html_rel}")
            continue
        check_document(root, html_rel, owner_dir, css_rels, errors, warnings)
    for warning in warnings:
        print(f"warning: {warning}")
    for error in errors:
        print(f"error: {error}")
    print(f"ui_contract_guard: {len(errors)} error(s), {len(warnings)} warning(s)")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
