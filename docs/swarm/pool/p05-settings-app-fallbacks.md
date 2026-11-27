# Unit p05-settings-app-fallbacks
STATUS: PARTIAL

crates/flamewm-settings: missing font, icon theme or unreadable settings file must degrade, never abort at startup; audit startup path for unwrap/expect and fix the worst.

## Result
STARTED

Production edits: 0 of 3 allowed (no production panic site exists to fix). Precedent: `.vendor/kwin-master/src/virtualdesktops.cpp:693` `group.readEntry("Number", 1)` — missing config key supplies a default instead of failing startup; this crate already follows that pattern.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-settings/src/main.rs:1-13` | Entry parses args with `match`/`if let Err` → `eprintln!` + `exit(2)`/`exit(1)`; no `unwrap`/`expect`/`panic!`. | None — UNBUILT | None |
| `crates/flamewm-settings/src/runtime.rs:21-28` | `run()` uses `?` and reports `refresh` errors inline via `report_error`; no production unwrap. | None — UNBUILT | None |
| `crates/flamewm-settings/src/xdg_cache.rs:47,75-76` | Unreadable/absent settings facade uses `if let Ok(...)`, `unwrap_or_default()` → empty defaults, no abort. | None — UNBUILT | None |
| `crates/flamewm-settings/src/theme_picker.rs:189-198,224-245` | Unset icon theme falls back to bundled `breeze`; missing previews return `None` → static placeholder; no unwrap. | None — UNBUILT | None |
| `crates/flamewm-settings/src/view.rs:21-47` | Backdrop reads font/wallpaper keys with `Some(SettingValue::Text) => ... _ => String::new()` and `unwrap_or(100)`; missing values degrade. | None — UNBUILT | None |
| `crates/flamewm-settings/src/{startup_page.rs:132,startup_extras.rs:110,runtime.rs:201,view.rs:51,default_apps_page.rs:134,launch.rs:106,theme_picker.rs:467}` | Every `unwrap`/`expect`/`panic!` in the crate is inside a `#[cfg(test)]` module or test-only module; production code has none. | None — UNBUILT | None |
| `Cargo.toml:70` | Release profile `panic = "abort"`: any future production panic would abort the process. Gate left unchanged per "never loosen gates". | None — UNBUILT | Low: amplifies future panics only; none exist on this path today. |

PARTIAL — Audit of the `flamewm-settings` startup path is complete, but no production edit was warranted: the three named degradation cases are already handled without panic (error messages + nonzero exit in `main`, `?`/inline error reporting in `runtime`, `if let Ok`/`unwrap_or_default` for settings files, bundled-`breeze` fallback and `None` placeholder for icon themes, safe key reads for fonts). All `unwrap`/`expect`/`panic!` hits are test-only, so "fix the worst" had no production target. Zero edits were made and nothing was built or tested (no build/test/git per brief; all rows UNBUILT); the orchestrator should compile once after the batch. Residual risk is limited to the release `panic = "abort"` profile, which would turn any *future* production panic into an abort but does not affect current behavior.
