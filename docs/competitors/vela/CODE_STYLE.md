# Vela code style

Scope: `.vendor/vela-main/` (version 0.2.0). Read-only reference. Evidence: direct reads of the cited
lines, plus `grep` and `wc`. No formatter or linter was run.

## 1. Tooling and enforced rules

| Rule | Enforced by | Anchor |
|---|---|---|
| No `.clang-format`, `.editorconfig`, or pre-commit config in the repo root | `ls -a` of the repo root | root listing |
| English for code, comments, commit messages, UI text. Some comments in Italian, translations welcome | `CONTRIBUTING.md:11` | `CONTRIBUTING.md:11` |
| cppcheck, `--std=c17 --enable=warning --error-exitcode=1`, on pure C modules | CI | `.github/workflows/static-analysis.yml:87-90` |
| clang-tidy, on pure C modules | CI | `.github/workflows/static-analysis.yml:100-103` |
| shellcheck `-S warning` on package scripts | CI | `static-analysis.yml:72` |
| Python doc, motion, and shared-value checks, with unit tests | CI scripts | `scripts/check-docs.py`, `check-qml-motion.py`, `check-shared-values.py`; `scripts/tests/test_*.py` |
| DCO sign-off | GitHub app | `.github/dco.yml` |

Formatting is by convention. Clang-format is not configured.

## 2. File header

Every source file opens with an SPDX pair. The copyright year is 2026.

```c
// SPDX-FileCopyrightText: 2026 Brando Giuffrida
// SPDX-License-Identifier: GPL-3.0-or-later
```

Seen in `compositor/src/focus.c:1-2`, `compositor/src/view.h` (header block), `compositor/src/motion.h` (header block).
Python files use `# SPDX-...` with the same two lines (`scripts/check-shared-values.py`).

## 3. Naming

| Kind | Pattern | Example | Anchor |
|---|---|---|---|
| Public C function | `vela_<module>_<verb>` | `vela_config_get`, `vela_config_flag`, `vela_config_read` | `compositor/src/config.c:17,174,202,212` |
| Static C function | no prefix, `static` | `set_entry`, `start_preview_timeout`, `view_overlaps_output` | `config.c:144`, `compositor/src/focus.c:27,71` |
| Struct | `struct vela_<name>` | `struct vela_view`, `struct vela_server`, `struct vela_config`, `struct vela_curve` | `compositor/src/view.h`, `config.c`, `motion.h:12` |
| Macro constant | `VELA_<MODULE>_<NAME>` | `VELA_MOTION_FAST_MS`, `VELA_DECELERATE_CURVE` | `compositor/src/motion.h` |
| Header guard | `VELA_<NAME>_H` | `VELA_VIEW_H`, `VELA_MOTION_H` | `view.h:7`, `motion.h:7` |
| Struct fields | snake_case | `struct vela_config_entry`, `capacity`, `entries` | `config.c:161` |
| QML singleton | `Theme.qml`, `Theme.scale`, `Theme.decelerate` | | `common/controls/Theme.qml` |
| QML controls | PascalCase files | `Button.qml`, `Switch.qml`, `ListItem.qml` | `common/controls/` |
| Python | snake_case, `test_*.py` for modules | `check_shared_values`, `test_check_docs.py` | `scripts/tests/` |
| Test files | `tst_*.qml` for QML, `*_test.cpp` for gtest | `tst_switch.qml`, `backlight_choice_test.cpp` | `common/tests/`, `compositor/tests/` |

Mixed names are rare. Names follow the module. Function names embed the verb (`_get`, `_read`, `_finish`, `_flag`).

## 4. Layout and module size

- Files split by one concern each. `compositor/src/` has 69 `.c` and 68 `.h` files.
- Largest C files (lines): `view.c` 1,730, `xwayland.c` 1,129, `output.c` 959, `input.c` 889, `lock.c` 874, `command.c` 874, `interact.c` 776, `capture_watch.c` 712, `brightness.c` 681.
- Module size is not capped. The biggest files still have a single job (view, output, input). Splitting is done by topic (`output_config.c`, `output_manager.c`, `output_preview.c`).
- Small pure modules are common: `lock_retry_policy.h`, `device_loss_policy.h`, `night_light_policy.h`, `cursor_policy.h`. They carry the decision logic with no I/O.

## 5. Comments

- Header comments explain purpose and invariants, before the first `#include` or the first declaration (`view.h`, `motion.h`, `scene/scene.h:1-20`).
- Inline comments explain why, not what. Example: `focus.c` explains why a preview is shown in the current frame.
- Non-obvious rules get a block with a reference to the doc, such as `docs/visual-coherence.md §6.4` (`motion.h:9-14`, `check-shared-values.py:5-7`).
- Doc links point to `docs/*.md` section numbers. The design docs are the authority, so code and docs refer to each other.
- Comments are in English in most files. Some are in Italian; the contributing guide allows this.

## 6. Error handling

- C: returns `bool` for success/failure (`vela_config_path` returns bool, `config.c:17`). Errors are reported with `fprintf`/`wlr_log` (file-level grep).
- Bounds are enforced: a fixed buffer with `size_t size` argument, for example `vela_config_path(name, out, size)` (`config.c:17`).
- Memory: `realloc` with growth and explicit `vela_config_finish` to free (`config.c:161,193`).
- Input clamping rather than failure: a theme value out of range is clamped (`docs/configuration.md:97`).
- Theme chains are capped and cycle-safe: `inherits` stops at 16 files and a parent that inherits back is dropped (`docs/configuration.md:95`).
- Python: `raise ValueError(...)` with the file and key name (`check-shared-values.py`, `compositor_values`).

## 7. Concurrency and events

- Event-driven, single compositor loop (wlroots `wl_event_loop`). No threads found in `compositor/src/` (grep: no `pthread_create` in compositor sources, file-level check).
- Timers and animations use frame times, with `motion.h` as the single source of durations (`motion.h:1-25`).

## 8. Shell, QML, and C++

- QML files are small, one control per file. `common/controls/` has about 20 controls.
- C++ in `shell/`, `settings/`, `explorer/`: Qt-style classes, `.h` and `.cpp` pairs (`settings/src/displays.cpp`, `displays.h`).
- QML tests: `common/tests/tst_*.qml`, one per control.

## 9. Testing style

- Each module has a sibling `tests/` directory: `compositor/tests/*_test.cpp`, `settings/tests/`, `common/tests/`, `tests/functional/`.
- Functional tests: one Python file per feature, registered by CMake glob (`CMakeLists.txt:88-95`). Each module runs in its own CTest entry with its own timeout.
- Tests for pure policies use the header-only modules (section 4).

## 10. Quality notes

| Area | Quality | Evidence |
|---|---|---|
| Header discipline | High. Every header has a purpose comment and a guard | `view.h`, `motion.h` (read) |
| Naming | Consistent `vela_` prefix and module prefix | `config.c`, `motion.h` (read) |
| Comments | Explain invariants, link docs. Some comments are long paragraphs | `focus.c`, `scene/scene.h` (read) |
| Module size | Large in places (`view.c` 1,730 lines) | `wc -l` |
| Formatter | None configured. Style depends on the author | repo root listing |
| Static analysis | cppcheck and clang-tidy on pure C only, not all C | `static-analysis.yml:87-103` |
| Error handling | Bool returns and bounded buffers. No error enum | `config.c` (read) |

## 11. Takeaways for FlameWM

These are conventions to note, not to copy. FlameWM already has its own rules in `AGENTS.md` and
`docs/ARCHITECTURE.md`.

- Pure policy modules separated from I/O (`*_policy.h`). FlameWM's `*-core` crates already do this.
- Bounded inheritance and clamped values for theme files. Relevant to `flamewm-skin`.
- Shared constants with a drift checker. Relevant to motion or timing values duplicated across languages.

## Skipped or not verified

- Clang-format rules were not inferred. The repo has no config.
- Error logging calls were not counted in full (file-level grep only).
- No formatter, linter, or CI job was run.
