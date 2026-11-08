# KDE Plasma vs FlameWM: design consistency comparison

Snapshot: 2026-11-08. Method: source-only. No KDE screenshots were captured, and no KDE session was started.

## Evidence used

| Source | Viewed | Not viewed |
| --- | --- | --- |
| KDE | No images. Source only: `plasma-workspace/kcms/colors/`, `kwin/src/plugins/windowview/`, `kwin/src/plugins/overview/`, `plasma-workspace/krunner/qml/` | Breeze theme (not cloned), rendered Plasma panel, KRunner, Overview, Klipper popup, System Settings pages |
| FlameWM | `assets/screenshots/` (existing set, see `docs/competitors/vela/DESIGN_COMPARISON.md`) | No new capture |

## 1. Scorecard

No scores are given. Scores in `vela/DESIGN_COMPARISON.md` rest on screenshots. KDE has no screenshots in this pass, so a score would be an estimate and is not recorded.

| Surface | KDE | FlameWM | Note |
| --- | --- | --- | --- |
| Panel and taskbar | `unverified` | see Vela file | Plasma panel layout not viewed. |
| Overview and present | `unverified` | n/c | Source present (`windowview`, `overview`), visuals not viewed. |
| KRunner | `unverified` | n/c | QML in `krunner/qml/`, not rendered. |
| Settings | `unverified` | n/c | KCMs in `plasma-desktop/kcms/`, not rendered. |
| Clipboard popup | `unverified` | n/c | `klipper/` QML and widgets, not rendered. |

## 2. Recommendation

Do not use KDE as a visual reference for FlameWM. Its look is part of Breeze, and FlameWM's visual language is protected by project rules. Use KDE for behaviour ideas only (see `LEARNINGS.md`). A design comparison needs KDE screenshots from a clean session first; that is follow-up work, not part of this pass.
