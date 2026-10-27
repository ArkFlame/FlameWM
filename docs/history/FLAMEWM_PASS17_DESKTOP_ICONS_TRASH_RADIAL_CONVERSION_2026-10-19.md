# FlameWM 0.1.0 — Pass 17 desktop interactions and format conversion

Source baseline: FlameWM Pass 16 `FlameWM_0.1.0_pass16_shared_animation_mask_resize_flicker_fix_2026-10-19.tar.gz`.

## Changes implemented in source

1. File-extension MIME icon keys replace the common `document-edit` fallback for regular desktop files. Directories, launcher desktop entries, symlinks and Trash retain their distinct existing icon handling. If the installed icon pack lacks a particular key, the canonical resolver may fall back to its generic raster; verification in the full installed theme is required.
2. The retained 82×82 desktop drag ghost now uses the real icon override and matching two-line label at 50% opacity; cursor drag offset derives from actual tile position, not first motion delta. The count badge appears only for multiple selected entries.
3. `_NET_ACTIVE_WINDOW` requests targeting minimized clients now go through `restore_minimized` instead of focus-only; placement state, including maximized mode, is retained by the WM. This requires a native X11 minimized-maximized roundtrip test.
4. Default window-open animation is None and disabled in product settings and native WM fallback. Explicitly saved user preferences are not overridden.
5. The Delete key is mapped to a desktop `keyboard.input` event. For selected entries, it opens the Move to Trash confirmation. The context menu uses the same confirmation. Deletion now calls the existing Freedesktop Trash authority and never falls back to permanent deletion; errors are reported, not hidden.
6. The eight-petal wheel is a bundled self-contained SVG under `ui/desktop/convert-wheel.svg` with eight reusable semantic buttons and a center file-size hub. The desktop RWR build copies this asset to its staged `assets/` directory, and validates required live text node IDs.
7. Format providers: ImageMagick for standard image conversions; `pdftotext` for PDF→TXT; Pandoc for Markdown→TXT/PDF/DOCX, DOCX→TXT/MD/PDF, TXT→PDF. TXT→MD is a plain-text copy, not a Markdown parser. Choices requiring programs are offered only when the corresponding executable exists. Pandoc PDF output can additionally require a PDF engine (e.g. TeX). HEIC/AVIF depend on installed ImageMagick delegate support.
8. The conversion worker uses the existing no-overwrite temporary-file publication protocol. A 50%-opacity target-format preview appears at the next free grid cell and says “Converting…”. When the worker commits the output, the preview is removed, the real file is assigned that cell, and the layout is persisted. Conversion errors dismiss the preview and are logged.

## Not yet complete

- The screenshot is a **visual reference**: the authored eight-petal SVG approximates its wheel proportions/peach palette with FlameWM-native UI, but pixel-perfect parity has not been measured in a native runtime.
- A determinate bottom-to-top preview fill is **not implemented**. No shared trustworthy, monotonically increasing percentage exists for ImageMagick, Pandoc and `pdftotext`. The preview reports a real in-progress operation and completion, never a fabricated percentage. A future provider progress callback and separate clipping layer would be required.
- Preview is one at a time. Concurrent conversions can finish after a newer preview has replaced theirs; the ordinary desktop directory watcher still detects published files, but earlier conversions have no predicted placement guarantee. If the grid has no free cell, the predicted cell policy falls back to the source cell; a future pass can suppress the preview in that case.
- Build/compile, integrated X11 mouse/keyboard presentation, icon-theme raster lookup, Trash on mounted volumes, window restore, and converter runtime must be verified against the user's full working repository. This authoring container has no `rustc`/`cargo`; the supplied source export lacks the full asset tree required by the build scripts. **No native release certification is claimed.**

## Verification evidence

- `python3 -I tools/ui_contract_guard.py`: 0 errors, 24 inherited warnings.
- `python3 -I -m unittest discover -s tools -p 'test_desktop_conversion*.py' -q`: 18 pass.
- `python3 -I -m unittest discover -s tools -p 'test_package_source_archive.py' -q`: 4 pass.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_xtest_*.py' -q`: 53 pass.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_desktop_*acceptance.py' -q`: 24 pass.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_graphical_flicker_pass16.py' -q`: 6 pass.
- `python3 -I -m unittest discover -s tools -p 'test_flamewm_desktop_pass17_contract.py' -q`: 7 pass. The new tests caught and prevented a pre-declaration `kind` use.

## Native continuation

Run `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets` first, then relevant `cargo test -p flamewm-desktop-core -p flamewm-desktop -p flamewm-wm-x11`, then `./xephyr` from the **full checkout with bundled assets**. Explicitly test: all file types, drag ghost with one/multiple selection, Delete/Trash confirmation cancel and commit, maximize→minimize→activate, optional conversion provider availability, wheel clipping and button hit testing, preview entry placement, worker timeout/failure cleanup, and preexisting workspace interactions. Preserve the native results and file paths, and do not claim passing gates from static analysis alone.
