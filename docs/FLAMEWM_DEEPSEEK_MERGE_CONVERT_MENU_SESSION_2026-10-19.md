# FlameWM 0.1.0 — DeepSeek + Cursor Pass 6 merge, workspace menu ownership, desktop image conversion, X11 session packaging

## Baseline/merge provenance

Base: user-supplied `027_19_10_26_deepseek_branch.tar.gz`. Preserve DeepSeek's accepted single `flamewm-api::workspace` two-row topology; keyboard shortcuts, WM EWMH publishing, and variable-width pager remain untouched except for the explicitly merged cursor policy. The agent transcript `Pasted text(8).txt` reports `cargo test --workspace` 2490 pass/0 fail/7 ignored, `cargo check --workspace --all-targets` exit 0, `cargo build --release --workspace` exit 0, and real Xvfb keyboard-grid checks. **Those are prior agent claims on its checkpoint, not fresh verification after this patch.**

Pass 6 merged semantically (no replacement of newer DeepSeek source): `wm/mod.rs`, `wm/pointer.rs`, `wm/tests.rs`. 24px opposite-edge cursor placement; 220ms dwell; 500ms mouse-only horizontal cooldown, with independent 500ms vertical-lane state reserved for when vertical mouse-edge dragging is implemented. The existing DeepSeek grid shortcut behavior is unchanged, and left/right mouse dragging currently retains the pre-existing flat wrap traversal (not yet grid-neighbor traversal). No Git metadata exists in either source archive, so this is a **source-level merge**, not a Git branch merge.

## Context-menu bug

`crates/flamewm-desktop/src/main.rs` previously responded to WorkspacesSnapshotChanged by setting `workspace_visual_dirty` and calling `sync_menu` on the NEW workspace while preserving `state.menu`, selected item, pending delete, and menu node visibility. This makes old context menus reappear on another workspace. The same leakage risk existed in shell's `context_menu` state. Repair: both desktop and shell dismiss context menus on authoritative workspace updates before repaint; desktop menu/confirm/wheel backdrops and hit rectangles are cleared; sticky notes still project per workspace. Regression test `workspace_menu_lifetime_tests` covers all menu types and delete intent. Full native X11 switching should still be tested.

## File Convert wheel

Files with a supported image extension expose **Convert ▸** in their right-click entry menu. Its radial picker offers PNG, WebP, JPG, GIF, BMP, TIFF, ICO, and AVIF, omitting equivalent source format. Source extensions also include JPEG, TIF, HEIC and HEIF when ImageMagick delegates support them. Folders, shortcuts, symlinks and trash never expose conversion. Chosen target immediately starts a detached conversion worker (does not block the UI thread); a successful conversion is atomically published beside the original as `<stem>.<target>` or `<stem> (converted N).<target>` if occupied. Original files are never modified; existing output names and symlinks are never overwritten (`rename_entry_no_replace`, NOREPLACE). Only private `.flamewm-convert-*` temporary files are hidden by the desktop model during inotify rescans, rather than hiding arbitrary dotfiles.

ImageMagick `magick` or `convert` must be installed; the Debian package now **Recommends: imagemagick**. Limits are set on process memory, mapped memory, and disk use. Errors are logged; no in-app toast or cancel/progress UI is provided yet. Format-specific delegates, especially AVIF/HEIC, vary by distro. Converting animated formats or files with multiple frames may be lossy. Input files are untrusted and handled by ImageMagick subject to the system's configured security policy.

## Login-session packaging

The existing `.deb` already installs `/usr/share/xsessions/flamewm.desktop`, with `Exec=/usr/bin/flamewm-session` and TryExec. The entry now additionally publishes `DesktopNames=FlameWM` and packaging unit tests assert the canonical installed path, executable and no fake Wayland entry. **Native Wayland support is NOT implemented.** FlameWM here is an X11 WM and cannot truthfully register a working Wayland compositor session. `docs/INSTALL.md` documents the real support boundary. A future Wayland provider needs its own implementation and native tests before `/usr/share/wayland-sessions` registration.

## Fresh checks run on this modified source

- `python3 -I tools/ui_contract_guard.py` -> 0 errors; 24 pre-existing/unrelated warning lines. Previously one unrecognized new action was repaired.
- `python3 -I -m unittest discover -s tools -p test_flamewm_deb_acceptance.py -q` -> 26 passed.
- `... -p test_flamewm_context_menu_weather_acceptance.py -q` -> 14 passed.
- `... -p test_flamewm_workspace_edge_acceptance.py -q` -> 6 passed (harness units only, NOT real X11 acceptance).
- `... -p test_ui_contract_guard.py -q` -> 8 passed.
- `... -p test_package_source_archive.py -q` -> 4 passed.
- Real ImageMagick test: run `magick -limit memory 256MiB -limit map 512MiB -limit disk 1GiB <2x2 PNG> webp:<existing temporary path>` -> exit 0, 68-byte valid WebP output. This is a provider probe, **not** a Rust native test.

## Unrun / release blockers

- No `rustc`, `cargo`, or usable release binaries in this sandbox; new Rust code is NOT compiled or tested.
- User-provided DeepSeek source archive is a trimmed export missing the entire `assets/` tree (including `assets/wallpapers/fitz-roy-dawn.webp` required in `crates/flamewm-desktop/build.rs`) plus other `.vendor/.agents` resources. This archive preserves that deficiency; running `cargo` on these exports alone will fail before compilation. Reapply onto the full live repository, then compile and run native X11 tests.
- No `.deb` was built or installed. Only the `.deb` packaging source and X11 session checks were updated.
- No Wayland backend exists; not complete.
- The pre-existing `MANIFEST.sha256` lists hundreds of omitted source paths and is retained strictly as historical metadata. This delivery's actual archive SHA-256 is supplied separately.

## Required live checks

1. Safely apply this source-level merge to a fresh full checkout with original assets; preserve existing dirty work.
2. `CARGO_BUILD_JOBS=2 cargo check --workspace --all-targets`; `RUST_TEST_THREADS=2 CARGO_BUILD_JOBS=2 cargo test --workspace`; release build.
3. In isolated Xvfb/Xephyr, right-click desktop/file/sticky/taskbar/workspace on workspace 1; switch to 2 and back; assert no menu, no invisible hitbox, and no pending delete/convert selection. Open menus on 2 independently.
4. Right-click `test.png` -> Convert -> WebP; assert a valid `test.webp` adjacent to `test.png`, source unchanged, then repeat to produce `test (converted 2).webp`. Test side-effect failure, missing converter, source symlink, collision, rapid workspace switch while converting, and 4+ file extensions.
5. During a held titlebar drag, test right/left crossing and wrap after 220ms dwell, 500ms axis cooldown, 24px cursor inset, mouse button preservation; compare with two-row keyboard navigation.
6. `./scripts/package` then `python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.0_amd64.deb` on a test root; verify X11 session listed in the display manager. Do NOT claim Wayland until real compositor support exists.

## Source-export hygiene

The DeepSeek upload contained 70 `__pycache__`/`.pyc` tar members generated by Python. Only these disposable bytecode cache entries were omitted during repack; **all 850 non-cache original entries (including 9 symlinks)** remain present and byte-preserved except the 15 intentional source/docs edits. The new archive adds the conversion Rust module and this verification report.
