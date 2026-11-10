# Look pass backlog (visual and behavioural consistency)

Evidence: deterministic captures in `/tmp/claude-1000/look/` (`c1-before`, `c1-after`, `s2-after`).
Harness: `/tmp/claude-1000/look/shot2.sh` (private Xvfb `:96`, 1920x1080, no WM, windows at 0,0,
dbus session, capture after two identical frames). Base build: clean `32f28041` in `look-base-fast`.

## Landed

- `bc8098cd` style(ui): one canonical `--muted` in `ui/base/base.css`. Removed copies in settings, finder,
  imageviewer. Visual check: settings secondary text colour `#aeb9cc` -> `#aeb4bb` only (crop reviewed).
  Re-pin: `settings_hotkeys_document_is_unchanged_by_utility_migration` 0xdf93_9260_dd67_2338 -> 0xc575_3a91_ac46_57f8.
  Gate: `EXIT_GUARD=0 EXIT_FMT=0 EXIT_CLIPPY=0 EXIT_TEST=0 GATE_DONE all=0` (pre-rebase tree).

## Dropped (not verifiable or wrong)

- Radius mapping on settings (`ui/settings.css`). Two changes were wrong: `.swatch-dot` (9px box) and
  `.preset-swatch` / `.wheel-swatch` (26px box) use 13px / 5px radii that make them circles or near-circles, and
  the mapping turned them into rounded squares. The captured settings pages showed zero pixel change, so the
  change could not be verified. Rule for any redo: a radius >= half the shorter side is a circle and must not be
  mapped.
- Radius commits for assistant/osd, task-manager, finder/explorer/archiver/imageviewer and shell non-dock were not
  made. Reasons: archiver capture is blank in both base and head; assistant capture is black in the deterministic
  run; task-manager never reaches two identical frames (live process data); shell panel/start/notifications are not
  in the deterministic harness; the affected rules are not visible on the captured pages.

## Found, not fixed

Design decisions (radii outside 1.5px of a scale step, kept on purpose for large surfaces):
1. 14, 15, 16, 17, 18, 24, 26, 28px on cards/panels: `ui/settings.css` (14, 16), `ui/assistant/assistant.css`
   (10, 14, 16, 17, 18, 24), `ui/assistant/osd.css` (14, 28), `ui/desktop/desktop.css` (15, 18, 26), `ui/flamewm.css`
   (15, 24), `ui/shell/flamewm.css` (18), `ui/shell/audio.html` (18, 24), `ui/shell/network.html` (16, 24),
   `ui/shell/notification-center.html` (16, 24), `ui/shell/osd.html` (18), `ui/shell/calendar.html` (18),
   `ui/shell/flamewm.css:293` (18).
2. 1px: `ui/flamewm.css:147`.
3. Two radius scales in `ui/base/tokens.css`: `--radius-*` (3/6/9/11) and `--r-*` (4/8/12). Choose one.
   Mapping that stays within 1.5px: 2, 2.5 -> radius-sm; 5, 7 -> radius-md; 10 -> radius-lg; 13 -> r-lg.

Colour and token drift:
4. `--muted` still differs in `ui/assistant/assistant.css` (`#d2d2d8`) and `ui/assistant/osd.css`
   (`#a1a1a6`), `ui/archiver/archiver.css` (`#a1a1a6`) and showcase `ui/flamewm.css` (`#aeb9cc`). Unify once the
   assistant and archiver captures work. Deliberate: `ui/terminal/terminal.css` (`#8a8a8a`),
   `ui/explorer/explorer.css` (`#f1f2f3`).

Harness gaps (block verification, not visual defects):
5. Archiver window renders blank under the harness. Assistant renders black. Task-manager never settles.
6. Shell (panel, start, notifications, dock excluded), desktop and lock were not in the deterministic run; the
   earlier non-deterministic capture (`before/`) is not comparable.
7. `ui/shell/launch-feedback.html` and `ui/task-manager/task-manager.html` and `ui/assistant/osd.html` lacked a token
   sheet; the radius pass needs `<link ../base/tokens.css>` in each, and `crates/flamewm-desktop/build.rs` needs
   `ui/base/tokens.css` prepended (desktop compiles its own sheet list).

Dock (not touched, owned by the translucency worker): `.dock-dot` radius 2px in `ui/shell/flamewm.css`.

## Unverified

- Hover and focus consistency, and controls that do nothing: not exercised.
- Goldens for `ui/` radius changes: not re-pinned (no radius commit landed).

## Measured: `0de670a1` vs parent `7ac41256` (assistant OSD, assistant, archiver)

Scope: `ui/assistant/osd.html` (links `../base/base.css`), `osd.css`, `assistant.css`, `archiver.css`; `--muted` removed locally.
Harness: private Xvfb 1920x1080 per tree (`:121` head, `:124` parent), no WM, dbus session, two identical frames.
OSD host run as `flamewm-assistant --osd` with stdin `listening`. Assistant run plain. Archiver run with a
sample `.tar.gz` argument (earlier blank capture was a missing-argument usage error, not a render fault).
Screenshots: `/tmp/claude-1000/osdshots/{parent,head-osd,head-r2}-{osd,assistant,archiver}.png`.

- OSD: frame 1920x1080 both. Pixel diff 227 px, bbox (932,581)-(989,589) = the "Esc cancels" line only.
  No layout shift, no focus ring, window size unchanged (260x140 WIDTH/HEIGHT). Muted text brightest pixel
  `#9f9fa4` (parent, from `#a1a1a6`) -> `#acb1b8` (head, from `#aeb4bb`), anti-aliased. `base.css:3` defines `--muted:#aeb4bb`.
- Assistant: 953 px differ, bbox (748,190)-(1008,919): secondary text rows only (subtitle, placeholder, footer). Not glyph-checked.
- Archiver: 846 px differ, bbox (91,77)-(619,227): muted labels only. Window size and layout unchanged.
- Verdict: colour-only change on muted text, no regression. No code fix made, nothing pushed except this note.
- Not checked: hover/focus states, shell OSD (`ui/shell/osd.html`, volume/brightness, not touched by `0de670a1`).
