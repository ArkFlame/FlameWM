# Icon and Cursor Themes: Spec

Status: research spec, read-only pass. No Cargo, no builds, no runtime runs were executed. All runtime claims below are open items.
Workspace version: `scripts/version` = `0.1.7`. Repo licence: `LICENSE` is LGPL-2.0 text, `Cargo.toml:9` = `LGPL-2.0-only`.

## 0. Summary

- Breeze is not the default today. Main sets `MacTahoe` (GPL-3.0) as the icon default. Uncommitted UX work in `/tmp/claude-1000/wt-ux` moves the default to `breeze`, but is not merged.
- The shipped repo already bundles GPL-3.0 MacTahoe icons and cursors (`packaging/install-manifest.txt:29-31`). This conflicts with the user rule "GPL-3 is ideas-only". It is the top finding.
- No FlameWM licence change is needed to offer or bundle the Breeze and Adwaita theme data. Aggregation as separate data files is sufficient (section 5).
- Cursor default is a separate chain: `MacTahoe-dark-cursors` > Oxygen > `FlameWM-Breeze-Dark`. It is not Breeze-first.

## 1. Current resolution (evidence anchors)

### 1.1 Icons

| Step | Owner | Anchor |
| --- | --- | --- |
| Product default string | `flamewm-platform` | `crates/flamewm-platform/src/settings.rs:487` `icon_theme: "MacTahoe"`; key `iconTheme` at `:1090`; field at `:268` |
| Shared default const | `flamewm-integrations-linux` | `crates/flamewm-integrations-linux/src/icons/mod.rs:1148` `DEFAULT_ICON_THEME = "MacTahoe"` |
| Search chain | same | `icons/mod.rs:1154-1171` `theme_chain`: `FLAMEWM_ICON_THEME` env, Flame `iconTheme`, kdeglobals/GTK configured theme, `DEFAULT_ICON_THEME`, `Breeze`; `hicolor` added by inheritance |
| Index + lookup | same | `icons/mod.rs:285` `IconResolver`, `:373` `new`, `:462-486` `from_environment*`, `set_preferred_theme`, `:525` `memory_estimate_bytes` |
| Async service | `flamewm-integrations-linux` | `icon_service.rs` (1400 lines): `ICON_RESOLVE_CONCURRENCY=20` (`:143`), `ICON_QUEUE_CAPACITY=256` (`:145`), `RESULT_CACHE_ENTRIES=2048` (`:147`), `RESULT_CACHE_BYTES=512*1024*4` (2 MiB, `:148`), `request_theme_change` (`:294`) |
| Shell loader | `flamewm-shell` | `crates/flamewm-shell/src/icon_loader.rs` (there is no `icon_loader` in integrations): `IconLoader::spawn` `:169`, `set_theme` `:248`, `resolve_icon_raster` `:32`, `icon_name_candidates` `:64`, `GENERIC_APP_ICON` `:92`, brand-fallback rejection `:398` |
| Shell fallback literal | `flamewm-shell` | `crates/flamewm-shell/src/launch_feedback.rs:173` `.unwrap_or("breeze")` (already Breeze, inconsistent with main's `MacTahoe`) |
| Shell settings read | `flamewm-shell` | `src/settings_projection/settings.rs:5-11` `icon_theme_name` (falls back to `ProductSettings::default`); `src/app/icons.rs:9`; `src/app/settings.rs:23` |
| XSettings publish | `flamewm-xsettings` | `src/lib.rs:115,161` `Net/IconThemeName`; `:191,195` `Gtk/CursorThemeName`, `Gtk/CursorThemeSize`; `blob.rs:207` `Net/ThemeName` |
| Picker | `flamewm-settings` | `src/theme_picker.rs:41-45` `LEGACY_ICON_THEMES` (`*:-HighContrast`, `breeze-dark`, `HighContrast`); `:152` empty value maps to `MacTahoe`; `:181` `BUNDLED_ICON_THEMES = ["MacTahoe"]`; `:185` `with_bundled_icon_themes` pins MacTahoe to top; `:264` registry build |
| Settings UI page | `ui/settings.html:34` `#page-theme` (accent, icon and cursor pickers) |
| Desktop apply | `crates/flamewm-desktop/src/settings_values.rs` (UX worktree adds `resolved_icon_theme`) |

### 1.2 Cursors

| Step | Owner | Anchor |
| --- | --- | --- |
| Settings key | `flamewm-platform` | `settings.rs:402` `cursor_theme: String`, default `""` at `:578`, key `cursorTheme` at `:1125`; validation `validate/product_validate.rs:132-138` (max 64 chars, no path separators) |
| Ids and family | `flamewm-session-core` | `src/lib.rs:22` `CURSOR_THEME_ID = "FlameWM-Breeze-Dark"`; `:24` `MACTAHOE_CURSOR_THEME_ID = "MacTahoe-dark-cursors"`; `:36` `cursor_theme_family` (MacTahoe, Oxygen only) |
| Resolver | same | `:58-116` `resolve_cursor_theme`. Order: requested id if installed (has `cursors/`); `MacTahoe-dark-cursors` if `cursors/left_ptr` exists; other MacTahoe variants; Oxygen variant; else `FlameWM-Breeze-Dark`. Breeze is only the last resort |
| Reload gate | same | `:121` `cursor_theme_reload`; `flamewm-shell/src/cursor_theme.rs:22-43` `CursorThemeSync`; applied via `flamewm-render-x11/src/external_decoration.rs:543` `apply_cursor_theme` > `apply_cursor_appearance` `:553` (per-owner registry) |
| Env build | `flamewm-session-core` | `:465` `cursor_environment`, `:222` `build_launch_plan_with_cursor_library_path` |
| X11 startup | `flamewm-wm-x11` | `src/runtime.rs:623-627` `persisted_cursor_theme`; `wm/construct.rs:60` root cursor; `frame/resources.rs:53,64` region-to-cursor map |
| Wayland | `flamewm-wm-wayland` | `src/cursor.rs:27` `DEFAULT_SIZE=24`; `:59-82` `Cursors` loads `XCURSOR_THEME` (saved `cursorTheme` wins through `cursor_env`, `:293-317`) then `default`; `:130-150` `named()` tries icon, then Default, then built-in `FALLBACK_ARROW`; `:170` `load()` uses `icon.name()` plus `alt_names()`; `:198` `decode_frame`. Cargo: `xcursor = "0.3"` (`Cargo.toml:48`) |

Notes:
- Cursor themes are reloaded only on id change (`CursorThemeSync`), and Wayland decodes cursors lazily per icon. Both are already cheap; keep that.
- XSettings is fed from settings (`flamewm-xsettings/src/lib.rs:161`); the source of the input value was not traced. Open item.
- Cursor default and icon default are decided in different crates. Changing one does not change the other.

### 1.3 UX worktree state (`/tmp/claude-1000/wt-ux`, not modified)

- Detached HEAD `478cc386`, a rewritten copy of main's `2c40c344` (same message `docs(arch): add architecture map ...`, different SHA). Main `05a92e17` is not an ancestor of `478cc386`. Rebase is needed before any merge.
- Uncommitted (`git status`): `icons/mod.rs` (`DEFAULT_ICON_THEME` to `"breeze"`), `flamewm-platform/src/settings.rs` (default uses `icon_theme::DEFAULT_ICON_THEME`), untracked `flamewm-platform/src/settings/icon_theme.rs` (`resolve_icon_theme`: `breeze` vs `breeze-dark` follows appearance; empty means Breeze; other names pass through), `flamewm-desktop/src/settings_values.rs` (`resolved_icon_theme`), `flamewm-desktop/src/tests/icon_theme_tests.rs`, plus unrelated taskbar and dock edits in the same tree.
- Not touched by UX work: `theme_picker.rs` (still pins MacTahoe at `:181`), `session-core` cursor chain, `packaging/install-manifest.txt`.
- Its `icon_theme.rs` unit tests were not run (no cargo, per assignment).

## 2. Installed theme inventory (this machine)

System `/usr/share/icons`:

| Theme | Inherits | Cursors | Size | Notes |
| --- | --- | --- | --- | --- |
| `breeze` | hicolor | none | 35M | icons, `index.theme` |
| `breeze-dark` | breeze, hicolor | none | 43M | dark variant |
| `breeze_cursors` | none | 115 | 16M | 15/15 names |
| `Breeze_Light` | none | 115 | 16M | 15/15 names |
| `FlameWM-Breeze-Dark` | none | 115 | 15M | bundled by FlameWM, 15/15 |
| `Adwaita` | AdwaitaLegacy, hicolor | 63 | 16M | 15/15 names |
| `default` | Adwaita | none | 4K | index only |
| `MacTahoe` | hicolor, breeze | none | 62M | GPL-3.0, FlameWM-packaged default |
| `MacTahoe-dark-cursors` | none | 124 | 8.2M | GPL-3.0, 15/15 |
| `KDE_Classic` | none | 76 | 200K | 10/15, missing `default grab grabbing ew-resize ns-resize` |
| `Oxygen_Black/Blue/White/Yellow/Zion` | none | 80 each | 3.8M each | `Oxygen_White` 9/15, missing `default grab grabbing ew-resize ns-resize crosshair` |
| `hicolor` | none | none | 6.9M | fallback |

User dirs: `~/.local/share/icons`: `Fluent-yellow` (48M, hicolor), `Fluent-yellow-dark` (29M), `PlagueSur` (172M, inherits WhiteSur-dark, breeze-dark, Adwaita, hicolor), `distrobox`, `hicolor`. `~/.icons`: empty.

Vendored themes under `.vendor/` (read-only evidence):

| Source | Themes | Licence file | Verdict |
| --- | --- | --- | --- |
| `MacTahoe-icon-theme-main` | MacTahoe icons and cursors, dist-dark | `COPYING` GPL-3.0 | ideas-only |
| `MacTahoe-gtk-theme-main` | GTK | `COPYING` GPL-3.0 | ideas-only |
| `LordOS-main` | `assets/gtk/LordOS{,-dark}` (GTK index only found) | `LICENSE` MIT | compatible; no icon data inspected |
| `LingmoOS-main` | Crule icons, lingmo cursors | `LICENSE` GPL-3.0 | ideas-only |
| `FileWheel-main` | not a theme | `LICENSE` MIT | n/a |
| `starling-main` | not a theme (Apache-2.0) | `LICENSE` Apache-2.0 | n/a |
| `cursors-icons-themes-kdestore` | 15 archives (see section 5.1) | in-archive, partly checked | mostly GPL family, unverified |
| `cosmic-epoch/cosmic-icons` | icon index only | no `LICENSE` found by name | unverified |
| `icewm-master` `Infadel2/cursors` | IceWM cursors | not checked | not in scope |

Repo-bundled assets (already shipped):

- `assets/cursors` (15M): `FlameWM-Breeze-Dark`, KDE Breeze Classic cursors, byte-copied upstream. `COPYING-ICONS`: LGPL v3.
- `assets/web/breeze` (140 files): Breeze UI glyphs for shell and lock. `COPYING-ICONS`: LGPL v3.
- `assets/themes/icons/MacTahoe` (61M): `COPYING` GPL-3.0. Shipped default. Violates the user rule.
- `assets/themes/cursors/MacTahoe-dark-cursors` (8.2M): `LICENSE` GPL-3.0. Shipped. Violates the user rule.
- `LICENSES/` holds only `IceWM-LGPL-2.0.txt` and `RustWebRender-MIT.txt`. There is no LGPL-3.0 text, no `THIRD_PARTY_NOTICES`. Gap.

## 3. Coverage data (measured)

Cursor names (15): `default pointer text wait progress grab grabbing move ew-resize ns-resize nesw-resize nwse-resize crosshair not-allowed help`. Checked by file existence in `cursors/`, without alias resolution:

- 15/15: `breeze_cursors`, `Breeze_Light`, `Adwaita`, `MacTahoe-dark-cursors`, `assets/cursors` (`FlameWM-Breeze-Dark`).
- 9/15: `Oxygen_White` (missing `default grab grabbing ew-resize ns-resize crosshair`).
- 10/15: `KDE_Classic` (missing `default grab grabbing ew-resize ns-resize`).

Icon sample (20 names, found under `breeze` and `hicolor` only): 19 present. `firefox` is missing from both. It needs a fallback or a miss report, not a crash.

Important: the Oxygen and KDE gaps may be covered by xcursor alias names (`alt_names`). The test must check aliases, not raw files. Open item.

## 4. Offer set and bundling

### 4.1 Offered by default

- Icons: `breeze` (light) and `breeze-dark`, chosen by appearance (the UX `resolve_icon_theme` rule). Adwaita as the second choice.
- Cursors: `breeze_cursors` or `FlameWM-Breeze-Dark` (bundled), `Breeze_Light` when light is chosen. Adwaita second.

### 4.2 Offered if installed (detect-only, never bundled)

- Icons: `MacTahoe`, `PlagueSur`, `Fluent-yellow`, `Fluent-yellow-dark`, any `Adwaita` variant, any user dir theme with `index.theme`.
- Cursors: `MacTahoe-dark-cursors`, `Oxygen_*`, `KDE_Classic`, any with a `left_ptr` in `cursors/`.
- Label cursors that are incomplete (fewer than 15 names) in the picker, with a warning.

### 4.3 Not offered

- Papirus (GPL-3.0 upstream), Bibata (GPL-3.0 upstream), kdestore Bibata and Qogir and Vimix and Sweet and Capitaine and oreo and volantes and Tela and WhiteSur and Fluent and McMojave archives (see 5.3). Ideas-only.
- LingmoOS, MacTahoe-gtk, all `.vendor` GPL-3 content: reference only.

### 4.4 Bundle vs detect

- Default path: detect-installed. Do not bundle new icon data. Add package dependencies instead (see D5): Debian `breeze-icon-theme`, Arch `breeze-icons`, `adwaita-icon-theme`. This keeps the size budget unchanged.
- Shipped today: keep `assets/cursors` (`FlameWM-Breeze-Dark`, LGPL-3, precedent) and `assets/web/breeze` (shell glyphs).
- Remove from the bundle: `assets/themes/icons/MacTahoe` and `assets/themes/cursors/MacTahoe-dark-cursors` (about 69M). Needs a user decision because it changes the shipped default on fresh installs. Recommended: remove from the bundle, keep detect-only.
- Size budget: no new bundled theme in phase 1. Any later bundled subset must stay under 2 MiB per theme and must be LGPL-2.1 or LGPL-3 or CC-BY-SA with its licence text in `LICENSES/`.

### 4.5 Lazy load, cache, memory

- Icons: index is built off-thread by `ThemeRebuilder` (`icon_service.rs`, `icons/mod.rs:373-436`). Only the selected chain is indexed. Keep one index per generation. Result cache is bounded (`RESULT_CACHE_ENTRIES=2048`, 2 MiB).
- Cursors: decode only on demand (`Cursors::named`). Keep. Reload only on id change (`cursor_theme_reload`).
- Measure before/after with `IconResolver::memory_estimate_bytes` and `cache_len`. Gate in D3.

### 4.6 Fallback chain (target)

1. Explicit user theme (`iconTheme`).
2. Breeze family by appearance (`breeze` or `breeze-dark`).
3. Adwaita.
4. Configured desktop theme (kdeglobals or GTK). Only when the user chose "system".
5. `hicolor` (from inheritance).
6. Bundled Breeze shell glyphs (`assets/web/breeze`) for shell surfaces.
7. `application-x-executable` generic icon.

Cursor chain (target): explicit id if installed > `FlameWM-Breeze-Dark` or `breeze_cursors` > `Adwaita` > `Oxygen` only if 15/15 with aliases > `MacTahoe-dark-cursors` only if installed and selected > built-in arrow. Changing this changes the cursor on this machine (MacTahoe is installed here). Needs user decision.

### 4.7 Missing-icon handling

- Miss returns `IconError`; loader falls back to bundled glyph (`icon_loader.rs` test `task_icon_miss_with_empty_theme_still_yields_bundled_glyph`, `:458`). Keep.
- Brand fallback must not count as a real hit (`is_brand_fallback`, `icon_loader.rs:398`). Keep.
- Miss counts go to the verification report (section 6.1), never to a user-visible error.

## 5. Licensing

### 5.1 Verdicts

| Theme or asset | Licence (evidence) | Verdict for FlameWM (LGPL-2.0-only) |
| --- | --- | --- |
| Breeze icons (`breeze`, `breeze-dark`) | LGPL-3+ (`/usr/share/doc/breeze-icon-theme/copyright`) | Compatible as separate data files, aggregated with notice. Do not link or compile. Prefer package dependency. |
| Breeze cursors (`FlameWM-Breeze-Dark`, `breeze_cursors`, `Breeze_Light`) | LGPL v3 (`assets/cursors/COPYING-ICONS`; `breeze-cursor-theme` copyright) | Same as above. Already shipped in `assets/cursors`. |
| Adwaita | Dual: LGPL-3 or CC-BY-SA-3.0; CC-BY-SA-4.0 (`/usr/share/doc/adwaita-icon-theme/copyright`) | Compatible. When bundled, state the CC-BY-SA alternative. Prefer detect. |
| Oxygen, KDE_Classic (installed) | Not verified locally | Detect-only until checked. |
| MacTahoe icons and cursors (repo `assets/themes`) | GPL-3.0 (`COPYING`, `LICENSE`) | Not allowed by the user rule. Currently bundled: remove. |
| MacTahoe-gtk, LingmoOS, Papirus, Bibata | GPL-3.0 | Ideas-only. |
| LordOS | MIT | Compatible if icon data is ever used. |
| kdestore `candy-icons` | MIT (in-archive `LICENSE`) | Compatible if used. |
| kdestore `Reversal` | MIT (in-archive `COPYING`) | Compatible if used. |
| Other kdestore archives (Fluent, McMojave, Tela, WhiteSur, Qogir, Vimix, Bibata-Modern-Ice, capitaine, Sweet, oreo, volantes) | No licence member found by name; upstream expected GPL family | Unverified. Ideas-only. |

### 5.2 Is aggregation enough? Yes.

- FlameWM ships theme data as separate files under `usr/share/icons/...` or `assets/...`. Those files are read at runtime through the XDG icon and cursor lookup. They are not compiled into or linked with the Rust code.
- Each data set keeps its own licence text (its `COPYING`) and a notice in `THIRD_PARTY_NOTICES`. FlameWM's own code licence does not change.
- The existing repo already does this with LGPL-3 data (`assets/cursors`, `assets/web/breeze`), so no new legal pattern is introduced.
- Caveat: LGPL-2.0-only code cannot incorporate LGPL-3 code. That is only a problem if LGPL-3 code (not data) is linked. No theme needs that. Rust crate licences were not audited here (`docs/DEPENDENCIES.md`); open item.
- Caveat: aggregation does not make GPL-3 content acceptable under the user rule. Remove MacTahoe from the bundle regardless of legal result.

### 5.3 Recommendation: no relicense

- Relicense is not required to bundle or detect Breeze, Adwaita, or any listed theme. Keep `Cargo.toml:9` and `LICENSE`.
- Only if the user later wants LGPL-3 code linked into FlameWM would a relicense be needed. The minimal option then would be LGPL-3.0-or-later for the whole product. Files that would change: `Cargo.toml:9` (and every crate `license` field), `LICENSE` (replace with LGPL-3 text), `LICENSES/` (add LGPL-3 text), `packaging/arch/PKGBUILD` license line, `packaging/debian/control.in` and any `copyright` file, README and docs licence statements. LGPL-2.0-only has no upgrade clause, so every contributor's code would need consent. That risk is the reason not to do it now.
- Action items (no licence-file edits required): add `LICENSES/LGPL-3.0.txt` (copy of the system LGPL-3 text) and `THIRD_PARTY_NOTICES.md` (theme name, source, licence, path). Done in D5.

### 5.4 Lead decision (user authorised): GPL-3 theme data ships as separate aggregate

- No relicense is needed. GPL-3 and LGPL-3 theme data (MacTahoe icons and cursors) ship as separate data directories, each with its own licence text and a `THIRD_PARTY_NOTICES.md` row. FlameWM code stays LGPL-2.0-only.
- Papirus and Bibata are optional packages, not bundled. Bibata is 27 MB extracted, above the base-package budget.
- Breeze stays the default icon and cursor theme. Installed GPL-3 themes remain selectable by name.
- Package names verified: Debian and Ubuntu `breeze-icon-theme` and `papirus-icon-theme` exist (`apt-cache policy`). Arch names `breeze-icons` and `papirus-icon-theme` are unverified (no pacman on this host).

## 6. Verification plan

### 6.1 Icon matrix (per theme)

- Test: `crates/flamewm-integrations-linux/src/icons/theme_matrix_tests.rs` (new). For each theme in `{breeze, breeze-dark, Adwaita, MacTahoe, PlagueSur, Fluent-yellow}`, install-detect; if absent, emit `SKIP` (not PASS).
- Fixed list of 40 freedesktop names: `preferences-system folder user-trash utilities-terminal system-file-manager accessories-text-editor document-open edit-copy edit-paste document-save go-home view-refresh battery-full network-wireless audio-volume-high application-x-executable text-x-generic image-x-generic video-x-generic audio-x-generic application-pdf firefox` plus 19 more (`dialog-error dialog-information dialog-warning emblem-default folder-open go-up list-add list-remove media-playback-start media-skip-forward preferences-desktop system-search system-shutdown user-home utilities-system-monitor web-browser window-close zoom-in`).
- Report: hits, misses, source dir, theme chain used. Miss list is a report, not a failure, for `firefox`-style gaps.
- Drive through `theme_chain(Some(theme))` (`icons/mod.rs:1154`) and `IconResolver::from_environment_with_theme` (`:468`). No Xvfb needed.

### 6.2 Cursor matrix (per theme)

- Test: `crates/flamewm-session-core/src/lib.rs` tests (`:863`, `:944`), using `install_cursor_theme` (`:1021`) and temp roots. Add a per-theme table.
- Check 15 standard names, including aliases (`CursorIcon::alt_names` from smithay, and xcursor alias table). Report raw-file misses and alias-resolved misses separately.
- Check fallback: `resolve_cursor_theme(requested, path)` returns `requested` when installed, and the target chain (section 4.6) otherwise.

### 6.3 Screenshot check (isolated Xvfb)

- Pick a free display from `:101` upward. Verify the lock file `/tmp/.X<N>-lock` is absent first. Start `Xvfb :<N> -screen 0 1280x800x24` and record its own PID only. Kill only that PID.
- Run `flamewm-x11-canary` or the minimal session with `XCURSOR_THEME=<theme>` and `XCURSOR_SIZE=24`. Capture root with `import -window root` (or `xwd`) and compare against a Breeze-default baseline.
- Cursor image: read via `XFixesGetCursorImage` or log the resolved path. A screenshot does not show the pointer shape reliably. Open item.
- Pattern: `scripts/screenshot-acceptance` already drives Xvfb via ctypes (`:20`, `:24`). Reuse, do not fork.
- Status: runtime evidence required (`flamewm-runtime-evidence`), not produced here.

### 6.4 Gates

- `scripts/gate` does not exist. The repo has `scripts/lint` (mirror of CI) and `scripts/release-gate`. Use those. Confirm the gate crate list with `scripts/release-gate` before running (open item).
- Crates to gate per deliverable: `flamewm-integrations-linux`, `flamewm-session-core`, `flamewm-platform`, `flamewm-settings`, `flamewm-shell`, `flamewm-desktop`, `flamewm-xsettings`, `flamewm-wm-wayland` (cursor), `flamewm-render-x11`, `flamewm-wm-x11`.
- Cargo: `CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2`, one Cargo owner at a time (`.opencode/HARNESS.md`).

## 7. Implementation breakdown

Independent deliverables. D1 and D2 change defaults and must be serialized with D4 (picker). D5 is docs and packaging only.

### D1. Breeze as the default icon theme (finish UX work)

- Rebase `/tmp/claude-1000/wt-ux` onto main (or cherry-pick the four icon-related files only).
- `crates/flamewm-integrations-linux/src/icons/mod.rs:1148` `DEFAULT_ICON_THEME = "breeze"`. Update doc comment at `:1146`.
- `crates/flamewm-integrations-linux/src/icons/mod.rs:1170-1171` chain: keep `DEFAULT_ICON_THEME` then `Breeze`, then add `Adwaita` after.
- `crates/flamewm-platform/src/settings.rs:487` use `icon_theme::DEFAULT_ICON_THEME`. `settings/icon_theme.rs` (untracked, 44 lines) is reused as-is.
- `crates/flamewm-settings/src/theme_picker.rs:152` empty icon value maps to `breeze`, `:181` `BUNDLED_ICON_THEMES = ["breeze"]`, `:185-198` pin Breeze, not MacTahoe.
- `crates/flamewm-shell/src/launch_feedback.rs:173` already `breeze`. Keep.
- Tests: `crates/flamewm-desktop/src/tests/icon_theme_tests.rs` (exists), `flamewm-platform` settings default test, `theme_picker` pin test.

### D2. Remove GPL-3 bundled MacTahoe and reorder cursor default

- `packaging/install-manifest.txt:29-31`: remove MacTahoe icon and cursor lines. Delete `assets/themes/icons/MacTahoe` and `assets/themes/cursors/MacTahoe-dark-cursors` from the tree (about 69M). User decision required.
- `crates/flamewm-session-core/src/lib.rs:58-116` new order per section 4.6. Keep `CURSOR_THEME_ID` at `:22` as the bundled Breeze cursor. `MACTAHOE_CURSOR_THEME_ID` `:24` stays a detect-only id.
- Update tests `:881`, `:929`, `:1042` (they assert `CURSOR_THEME_ID` in fallback paths; keep valid, add one for `breeze_cursors` present).
- Tests: `cursor_theme_core_and_library_fallback_are_explicit` (`:944`) and `bundled_cursor_authority_defaults_and_overrides` (`:863`).
- Wayland: `crates/flamewm-wm-wayland/src/cursor.rs:69-82` already uses `XCURSOR_THEME`; no change, only verify.

### D3. Theme matrix tests and memory check

- New `crates/flamewm-integrations-linux/src/icons/theme_matrix_tests.rs` (section 6.1). Register from `icons/mod.rs` test module.
- New cursor table tests in `flamewm-session-core` (section 6.2).
- Memory note: record `IconResolver::memory_estimate_bytes` and `cache_len` before/after switching default to Breeze. Use `flamewm-performance-proof` route for any perf claim.
- Gate: `flamewm-integrations-linux`, `flamewm-session-core`.

### D4. Picker: offer installed themes, warn incomplete cursors

- `crates/flamewm-settings/src/theme_picker.rs:210-235` `with_installed_cursor_themes`: order Breeze first, then Adwaita, then MacTahoe and Oxygen (detect-only), then others. Add 15/15 completeness check; label incomplete ones.
- `theme_picker.rs:41-45` keep legacy labels. Add `breeze` label.
- `ui/settings.html:34` page: no structural change; confirm labels via screenshot (6.3).
- Gate: `flamewm-settings`.

### D5. Packaging, notices and licence text

- `packaging/debian/control.in`: add `breeze-icon-theme`, `adwaita-icon-theme` to Recommends (not Depends, to keep install size).
- `packaging/arch/PKGBUILD`: add `breeze-icons`, `adwaita-icon-theme` to optdepends.
- `LICENSES/LGPL-3.0.txt`: new, copy of system LGPL-3 text.
- `THIRD_PARTY_NOTICES.md`: new. Rows: Breeze icons (LGPL-3+), Breeze cursors (LGPL-3, `assets/cursors`), Breeze UI glyphs (LGPL-3, `assets/web/breeze`), Adwaita (LGPL-3 or CC-BY-SA, if bundled). Remove MacTahoe row after D2.
- Note: `packaging/` is tracked; UX worktree already touches `PKGBUILD` and `control.in`. Coordinate.

### D6. XSettings consistency

- `crates/flamewm-xsettings/src/lib.rs:161` `Net/IconThemeName` and `:191` `Gtk/CursorThemeName` must publish the resolved value (Breeze family by appearance), not the raw setting.
- Trace the `input.icon_theme` source (open item) before editing.
- Gate: `flamewm-xsettings`.

## 8. Open items and risks

1. Blocker for default change: removing MacTahoe changes the shipped default and the cursor on this machine. User decision required.
2. Breeze-family path: `breeze-icon-theme` is not installed on all distros. D5 Recommends is not a guarantee. Bundled glyphs cover shell surfaces only; app icons fall back to `hicolor`.
3. Oxygen and KDE_Classic gaps may be alias-covered. Verify before labelling incomplete.
4. Breeze-dark vs `resolve_icon_theme` appearance: `system` mode resolves to light (UX test `:34`); confirm desired behaviour.
5. XSettings input source not traced (`flamewm-xsettings`).
6. `scripts/gate` absent. Use `scripts/lint` and `scripts/release-gate`.
7. Unverified: Oxygen and KDE_Classic licences, kdestore archives without licence members, cosmic-icons licence, Rust crate licences.
8. No runtime evidence exists for any theme on screen. Screenshot and cursor checks (6.3) are required before any "works well" claim.

## 9. Recommended order

1. D2 (licence blocker and cursor default), after user decision.
2. D1 (rebase UX work onto main; Breeze default).
3. D3 (matrix tests; gives the "works well" evidence).
4. D4 (picker).
5. D5 (packaging and notices, in parallel with D4).
6. D6 (XSettings), then the 6.3 screenshot run.

Checks run for this spec: read-only file, git, and package-doc inspection, plus the coverage scan re-run under bash after a zsh word-splitting bug. No Cargo, no build, no test, no runtime run. Exit codes were not recorded per command; treat the measured counts in section 3 as observations, not gate results.
