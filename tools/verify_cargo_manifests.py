#!/usr/bin/env python3
"""Fail before build or release when a Cargo manifest has malformed dependency/feature tables."""

from pathlib import Path
import sys
import tomllib


def validate(root: Path) -> int:
    paths = [root / "Cargo.toml", *sorted((root / "crates").glob("*/Cargo.toml"))]
    if len(paths) < 2:
        raise ValueError(f"workspace manifests missing: {root}")
    for path in paths:
        with path.open("rb") as stream:
            cargo = tomllib.load(stream)
        for feature, deps in cargo.get("features", {}).items():
            if not isinstance(deps, list) or not all(isinstance(item, str) for item in deps):
                raise ValueError(f"{path.relative_to(root)}: feature {feature!r} must be an array of strings, not a dependency table")
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name, spec in cargo.get(section, {}).items():
                if not isinstance(spec, (str, dict)):
                    raise ValueError(f"{path.relative_to(root)}: {section}.{name} has invalid type")
    # Cargo accepts an old API crate manifest even if dependent sources import
    # newly added modules. Detect the mixed-version overlay before compiling.
    clients = (
        root / "crates/flamewm-render-x11/src/app/mod.rs",
        root / "crates/flamewm-render-x11/src/external_drawable.rs",
        root / "crates/flamewm-wm-wayland/src/main.rs",
    )
    if any(client.is_file() and "flamewm_api::graphics" in client.read_text(encoding="utf-8")
           for client in clients):
        api = root / "crates/flamewm-api/src/lib.rs"
        module = root / "crates/flamewm-api/src/graphics.rs"
        if not api.is_file() or "pub mod graphics;" not in api.read_text(encoding="utf-8"):
            raise ValueError("mixed FlameWM versions: flamewm-api/src/lib.rs does not export 'pub mod graphics;' (use a complete source extraction)")
        if not module.is_file():
            raise ValueError("mixed FlameWM versions: flamewm-api/src/graphics.rs is missing (use a complete source extraction)")
        contents = module.read_text(encoding="utf-8")
        for required in ("pub enum GraphicsProvider", "pub enum DisplayProtocol", "pub fn choose_provider("):
            if required not in contents:
                raise ValueError(f"mixed FlameWM versions: flamewm-api/src/graphics.rs missing {required}")
    return len(paths)


if __name__ == "__main__":
    try:
        count = validate(Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve())
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"FLAMEWM-CARGO-MANIFEST: FAIL: {exc}", file=sys.stderr)
        raise SystemExit(1) from exc
    print(f"FLAMEWM-CARGO-MANIFEST: PASS ({count} manifests)")
