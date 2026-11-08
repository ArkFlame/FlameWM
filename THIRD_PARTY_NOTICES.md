# Third-party notices

FlameWM code is LGPL-2.0-only (`LICENSE`). The theme data below is separate
files with their own licence text. Aggregation, not linking: the code does not
compile or link any of it.

| Component | Path | Licence text | Status |
| --- | --- | --- | --- |
| Breeze cursors (`FlameWM-Breeze-Dark`) | `assets/cursors` | `assets/cursors/COPYING-ICONS` (LGPL-3.0) | Bundled, default cursor |
| Breeze UI glyphs | `assets/web/breeze` | `assets/web/breeze/COPYING-ICONS` (LGPL-3.0) | Bundled, shell and lock |
| Breeze icons (`breeze`, `breeze-dark`) | distro package `breeze-icon-theme` / `breeze-icons` | `LICENSES/LGPL-3.0.txt` | Default icon theme; package dependency |
| MacTahoe icons and cursors | `assets/themes/icons/MacTahoe`, `assets/themes/cursors/MacTahoe-dark-cursors` | `COPYING` / `LICENSE` in each directory (GPL-3.0) | Bundled, optional selection |
| Papirus icons | distro package `papirus-icon-theme` | `LICENSES/GPL-3.0.txt` | Optional package, not bundled |
| Bibata cursors (Bibata-Modern-Ice) | not bundled (27 MB extracted, above base budget) | `LICENSES/GPL-3.0.txt`; upstream archive has no licence member | Detect-only, optional package |
| Adwaita | system `adwaita-icon-theme` | LGPL-3.0 or CC-BY-SA | Fallback, detect-only |

Licence texts: `LICENSES/GPL-3.0.txt`, `LICENSES/LGPL-3.0.txt`, `LICENSES/IceWM-LGPL-2.0.txt`, `LICENSES/RustWebRender-MIT.txt`.
Never copy GPL-3 code into FlameWM; GPL-3 themes are data only.
