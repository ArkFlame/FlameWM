#!/usr/bin/env python3
"""Detect a partial desktop Convert merge before Cargo compiles the UI consumer.

This is a source contract preflight, not a Rust compiler replacement.  No edits.
"""
from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path


def contract_errors(root: Path) -> list[str]:
    desktop = root / "crates/flamewm-desktop/src/main.rs"
    core = root / "crates/flamewm-desktop-core/src"
    if not desktop.is_file():
        return [f"desktop entrypoint missing: {desktop}"]
    consumer = desktop.read_text(encoding="utf-8")
    if "flamewm_desktop_core::conversion" not in consumer or "EntryMenuAction::Convert" not in consumer:
        return ["desktop main.rs is missing Convert consumer integration"]
    sources = {}
    errors = []
    for name in ("lib.rs", "file_actions.rs", "conversion.rs"):
        path = core / name
        if path.is_file():
            sources[name] = path.read_text(encoding="utf-8")
        else:
            errors.append(f"missing {path.relative_to(root)}")
            sources[name] = ""
    if not re.search(r"(?m)^\s*pub\s+mod\s+conversion\s*;", sources["lib.rs"]):
        errors.append("flamewm-desktop-core/src/lib.rs does not export `pub mod conversion;`")
    actions = sources["file_actions.rs"]
    if not re.search(r"(?s)enum\s+EntryMenuAction\s*\{[^}]*\bConvert\s*,", actions):
        errors.append("file_actions.rs EntryMenuAction lacks Convert variant")
    if not re.search(r"\bSelf::Convert\s*=>\s*\"Convert\"", actions):
        errors.append("file_actions.rs EntryMenuAction::label lacks Convert branch")
    if not re.search(r"fn\s+entry_context_menu\s*\(\s*is_trash\s*:\s*bool\s*,\s*convertible\s*:\s*bool\s*\)", actions):
        errors.append("file_actions.rs entry_context_menu must accept (is_trash: bool, convertible: bool)")
    if not re.search(r"if\s+convertible\s*\{\s*rows\.push\(EntryMenuAction::Convert\)\s*;\s*\}", actions):
        errors.append("file_actions.rs does not add Convert only when convertible")
    for old_call in ("entry_context_menu(true)", "entry_context_menu(false)"):
        if old_call in actions:
            errors.append(f"file_actions.rs still invokes legacy {old_call} in unit tests")
    provider = sources["conversion.rs"]
    for symbol in ("FORMAT_CHOICES", "offered_formats", "convert_image", "convert_file", "predicted_destination"):
        if not re.search(rf"\b(?:pub\s+(?:const|fn))\s+{symbol}\b", provider):
            errors.append(f"conversion.rs lacks public {symbol}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent,
                        help="source checkout to check (default: archive containing this tool)")
    args = parser.parse_args()
    root = args.root.resolve()
    errors = contract_errors(root)
    if not errors:
        core = root / "crates/flamewm-desktop-core/src"
        print(f"OK FlameWM desktop Convert SOURCE contract (Cargo cache not yet checked): {root}")
        for name in ("lib.rs", "file_actions.rs", "conversion.rs"):
            data = (core / name).read_bytes()
            print(f"  {name}: sha256={hashlib.sha256(data).hexdigest()[:16]}")
        return 0
    print(f"FLAMEWM-DESKTOP-CONVERSION-CONTRACT: INCOMPLETE SOURCE MERGE in {root}", file=sys.stderr)
    for problem in errors:
        print(f"  - {problem}", file=sys.stderr)
    print("This checkout has an incomplete desktop/core source merge. Overlay the newest"
          " complete source archive and run ./scripts/check-cargo before building.", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
