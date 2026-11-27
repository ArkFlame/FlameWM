# Unit p06-explorer-startup-fail
STATUS: PARTIAL

crates/flamewm-explorer: startup with an unreadable home, missing mime database or missing icon theme must degrade, not panic; audit and fix the worst (max 3 edits).

## Result
STARTED

Production edits: 0 of 3 allowed. No production panic site exists on the named startup paths; every unwrap/expect/panic hit in the crate is either an unwrap_or* fallback or inside a #[cfg(test)] module.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-explorer/src/main.rs:355-374 | main reports run() errors through eprintln! plus exit(1); home_dir() has an is_dir probe with / fallback, and canonicalize failures propagate as Err, never a panic. | None - UNBUILT | None; an unreadable home still starts at / or reports one error line. |
| crates/flamewm-explorer/src/lib.rs:534-546,595-631 | The home probe is cached only when is_dir() succeeds; sidebar reads user-dirs.dirs, bookmarks and mountinfo with read_to_string(...).unwrap_or_default() or if let Ok, so an unreadable home yields defaults. | None - UNBUILT | None; no visual change on the normal path. |
| crates/flamewm-explorer/src/mime.rs:94-115,124-210,212-284 | Missing mimeapps.list layers fall back to unwrap_or_default(); xdg-mime absence, non-zero status or empty MIME output returns Err; discover maps catalog failure to Err shown once in the chooser. No bare unwrap/expect in production code. | None - UNBUILT | None; the chooser already degrades to text-only rows. |
| crates/flamewm-explorer/src/appicon.rs:76-99,104-192,197-212 | Missing icon roots or themes are probed with is_file() one path at a time; resolve_icon returns None and the row keeps its embedded file glyph. The unwrap hits at 225/268/245 are test-only. | None - UNBUILT | None; visuals stay pixel-identical. |
| crates/flamewm-explorer/src/theme.rs:39-130; crates/flamewm-explorer/src/app/projection.rs:949-1005 | The theme starts from SettingsSnapshot::new(0) with clamped values; chooser projection hides icon slots on a failed bind and shows a loading line before discovery. The only expect("project explorer theme") is in #[cfg(test)]. | None - UNBUILT | None. |
| crates/flamewm-explorer/src/{app/*.rs,jobs.rs,places.rs,prefs.rs,thumbs.rs,trash.rs,workers.rs,view.rs} | Sweep of every production file found only unwrap_or* / PoisonError::into_inner fallbacks; all bare unwrap/expect/panic hits are inside #[cfg(test)] modules. | None - UNBUILT | None. |
| .vendor/xfdesktop-master/src/xfdesktop-file-utils.c:420-459; .vendor/xfdesktop-master/src/xfdesktop-special-file-icon.c:233-248 | Reference-first: XFCE falls back to a bundled pixmap, then g_themed_icon_new_with_default_fallbacks, when a theme/icon is absent, matching the explorer's embedded-glyph fallback. | None - UNBUILT | Reference only; .vendor/** is not a dependency or runtime input. |

PARTIAL - audit of the explorer startup path is complete. All three named degradation cases (unreadable home, missing MIME database, missing icon theme) already fail soft through unwrap_or_default, Err plus status text, and None-to-embedded-icon, with no production unwrap/expect/panic reachable from startup. Zero production edits were warranted, so nothing was changed; every row above is UNBUILT and no build, test or git command was run under this unit's rules. The orchestrator must compile once after the batch; residual risk is limited to third-party xdg-mime and IconResolver behavior outside this crate.
