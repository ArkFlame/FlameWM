# Fault-injection test index

**Status: script checks verified; cargo suites UNBUILT.** The index script and this document were
written by static reading, then checked at script level with `bash -n scripts/run-fault-tests`
(SYNTAX_OK) and `bash scripts/run-fault-tests --list` (six cases printed, exit 0). No `git`,
`cargo`, build, test, Python or sleep loop was run. The orchestrator owns compilation, test
execution and the commit after the batch lands.

## Inventory

Discovery started from `rg -l 'fault' crates --glob '*tests*.rs'`. That bare substring also
matches `default`, so it returns 88 mostly unrelated files; the list below is curated to the
fault-injection modules added this session (standalone `fault`, `fault_tests`, or a documented
fault-injection intent). `backend_fallback_tests.rs` (u36c) and `load_tests.rs` (u34) are included
even though their file names have no `fault`; both declare a documented fault-injection intent.

| Module (file:line of `mod` declaration) | Package | Cargo filter | Fault injected | Unit |
| --- | --- | --- | --- | --- |
| `crates/flamewm-render-x11/src/accel/mod.rs:3` -> `src/accel/fault_tests.rs` | `flamewm-render-x11` | `fault_tests` | missing helper binary, helper exit at start, mid-session timeout/death; hardware -> software -> CPU chain | u09c |
| `crates/flamewm-platform/src/settings.rs:258` -> `src/settings/load_tests.rs` | `flamewm-platform` | `load_tests` | corrupt settings file quarantined, missing file, legacy keys, atomic persist/load round-trip | u34 |
| `crates/flamewm-wm-x11/src/wm/mod.rs:593` -> `src/wm/startup_fault_tests.rs` | `flamewm-wm-x11` | `startup_fault_tests` | no extensions, no RandR, second WM on root `SubstructureRedirect` | u45 |
| `crates/flamewm-wm-wayland/src/main.rs:121` -> `src/backend_fallback_tests.rs` | `flamewm-wm-wayland` | `backend_fallback_tests` | native DRM init failure, nested winit init failure, failure-record append | u36c |
| `scripts/test-wrapper-faults:14` (shell script, no cargo) | n/a | n/a | missing-library `ldd` preflight vs all-resolved | u15b |
| `scripts/x11-startup-faults` (shell script, no cargo) | n/a | n/a | DISPLAY unset and unreachable (`connect`), second WM on private Xvfb (`become-wm`), healthy WM 3 s timeout; asserts exit and `FLAMEWM-X11-STARTUP-FAIL stage=` with `blast_radius` | w01 |

The Wayland suite is invoked twice, with default features and with `--features drm`, because u36c
requires both feature states to be checked.

## Running

```sh
bash scripts/run-fault-tests --list   # inventory only; no build, test or git
bash scripts/run-fault-tests          # orchestrator only: runs cargo test per module, then the wrapper test
bash scripts/test-wrapper-faults      # standalone shell fault test
```

The runner sets `CARGO_BUILD_JOBS=2` and `RUST_TEST_THREADS=2`, calls `cargo test -p <package> --
<filter>` sequentially (one Cargo owner at a time), and finishes with
`bash scripts/test-wrapper-faults`. It exits 0 only when every case passes and prints a
`SUMMARY fault suites=<n> failed=<n>` tail. A missing module file is a hard failure in both
`--list` and run mode.

`scripts/test-wrapper-faults` already existed from u15b; it was read, not edited. Its two cases
shadow `ldd` with a fake executable in a `mktemp -d` sandbox and source the wrapper's preflight
functions without running the wrapper's main tail. No session, compositor, real `ldd`, network or
build is involved.

## Evidence and precedent

Routes loaded: `flamewm-source-first` (source, declarations and call paths before editing) and
`no-adhoc-scripts` (no Python or temporary scripts; shell inventory in a reusable script).
No vendor-specific harness precedent was needed: the local conventions in
`scripts/test-session-fallbacks` and `scripts/test-wrapper-faults` (documented usage, bounded
exit codes, `mktemp -d` isolation) were followed. `.vendor/**` was not used as a dependency or
runtime input.

Verified here: `bash -n scripts/run-fault-tests` printed SYNTAX_OK and
`bash scripts/run-fault-tests --list` printed all six inventory cases and exited 0 without running
any build, cargo test or git. Verified by static read-back: the four `mod` declarations at
`accel/mod.rs:3`, `settings.rs:258`, `wm/mod.rs:593` and `main.rs:121`, the crate `name` entries,
the `drm` feature at `crates/flamewm-wm-wayland/Cargo.toml:78`, and the bare `rg` count of 88 files
(matching `default`). Unverified: every `cargo test` invocation and the `--features drm` variant;
no Cargo runner was allowed in this unit.

## Risks

Cargo filters are substring matches on test names: renaming a module requires updating the
`CASES` table and this document. The `--features drm` case needs the DRM development libraries on
the orchestrator host; if unavailable, that case fails while the other four can still pass. The
wrapper test asserts preflight behavior in-process only and does not drive the wrapper supervisor;
`scripts/test-session-fallbacks` covers that path.

## Fallback -> test matrix (p80)

Built from `rg 'fn .*(fallback|missing|corrupt|degrad|absent)' crates/*/src` plus a targeted read of
each named test. A test path is `file:line` (test function); fixture runners use the filters in
`scripts/run-fault-tests:31`. `GAP` means no test injects that condition. Nothing in this matrix was
built or executed. `.vendor/**` holds kwin, smithay and xfwm4 references; no vendor precedent is
needed for a docs-only inventory of already-existing local tests, and no vendor code is a dependency
or runtime input.

| Goal fallback | Test path | Coverage |
| --- | --- | --- |
| Backend: DRM (native) | `crates/flamewm-wm-wayland/src/backend_fallback_tests.rs:14` `native_ok_keeps_the_native_backend`; `:48` `native_without_drm_feature_falls_through_to_nested`; runner `cargo test -p flamewm-wm-wayland -- backend_fallback_tests` (also `--features drm`) | Covered: pure init-outcome mapping, feature gate |
| Backend: nested (winit) | `crates/flamewm-wm-wayland/src/backend_fallback_tests.rs:22` `native_fails_nested_ok_keeps_the_nested_backend`; `:34` `native_fails_nested_fails_goes_headless` | Covered: native -> nested step |
| Backend: X11 (capture chain) | `crates/flamewm-screenshot/src/portal.rs:456` `choose_chain_keeps_only_x11_when_others_unavailable`; `:469` `choose_chain_order_is_portal_then_screencopy_then_x11` | Covered as pure chain; no live X11 connection |
| Backend: minimal / headless | `crates/flamewm-wm-wayland/src/backend_fallback_tests.rs:34` (`NestedFailed` -> `Headless`; `main.rs:213-214` "socket alive with no rendering backend") | Partial: mapping only, no runner-state test |
| GPU: hw | `crates/flamewm-render-x11/src/accel/fault_tests.rs:105` `helper_exiting_nonzero_at_start_fails_hardware_then_keeps_software_device`; `:71`, `:142` | Covered via helper-binary injection |
| GPU: sw | `crates/flamewm-render-x11/src/accel/fault_tests.rs:105` (software device kept); `:71` (software tier exhausted -> CPU) | Covered |
| GPU: cpu | `crates/flamewm-render-x11/src/accel/fault_tests.rs:71` `missing_helper_binary_falls_through_both_tiers_to_cpu`; `:142` `helper_timeout_or_death_mid_session_falls_back_to_cpu` | Covered |
| Missing lib | `scripts/test-wrapper-faults:14` (fake `ldd` in `mktemp -d`); `crates/flamewm-session-core/src/diagnostics.rs:590` `fallback_plan_keeps_role_environment_and_stays_alive`; `:605` `shell_fallback_has_no_dialog` | Partial: preflight in-process only; supervisor not driven |
| Missing font | `crates/flamewm-render-core/src/glyphs.rs:559` `bundled_font_fallback_parses_and_inks_a_glyph`; `crates/flamewm-render-x11/src/xft/default_measure_tests.rs:9` | Partial: bundled asset tested directly; `find_font` (`glyphs.rs:145`) never injected with absent candidates |
| Missing icon | `crates/flamewm-shell/src/icon_loader.rs:647` `task_icon_miss_with_empty_theme_still_yields_bundled_glyph` | Covered |
| Missing theme | `crates/flamewm-session-core/src/lib.rs:998` `cursor_theme_core_and_library_fallback_are_explicit`; `:1092` `cursor_theme_resolution_order_and_fallback_chain`; `:1212` `cursor_alias_fallback_covers_standard_names_only_through_targets`; `appearance.rs:512`, `:586` | Covered: theme name/path resolution, no invented theme |
| Missing portal | `crates/flamewm-integrations-linux/src/portal_file_chooser.rs:468` `missing_portal_maps_to_one_clear_message_and_keeps_other_errors`; `crates/flamewm-screenshot/src/portal.rs:440` `choose_chain_skips_unavailable_portal` | Covered as pure map; no live D-Bus injection |
| Corrupt settings | `crates/flamewm-platform/src/settings/load_tests.rs:53` `corrupt_json_is_quarantined_once_and_defaults_are_used`; `:151` truncated; `:176` binary junk; `:196` huge | Covered: quarantine + defaults |
| Panic restart | `crates/flamewm-session-core/src/diagnostics/failure.rs:384` `guard_captures_panic_as_a_failure_record`; `:408` `crash_loop_stops_after_the_window_limit`; `:430`; `src/lib.rs:1267`, `:1285` | Partial: record and bounded-policy tests only; no supervised restart |

### Top 5 gaps - next units

1. p81-missing-font-path: inject zero system font candidates and assert `find_font` (`glyphs.rs:145`) selects the bundled fallback; `glyphs.rs:559` only parses the asset.
2. p82-panic-restart-e2e: drive `install_panic_hook` (`failure.rs:226`) through a supervised child panic and assert the bounded restart plan actually restarts, not just records.
3. p83-wrapper-missing-lib-supervisor: extend `scripts/test-wrapper-faults` beyond the in-process `ldd` shadow to run the wrapper main tail inside a `mktemp -d` sandbox.
4. p84-headless-backend-runner: assert `NextBackend::Headless` (`main.rs:213-214`) keeps the socket alive after `NestedFailed` with no rendering backend, not only the pure mapping.
5. p85-portal-absent-bus: inject an absent xdg-desktop-portal over a fake bus and assert both chains degrade live (`portal_file_chooser.rs:468`, `portal.rs:440` are pure maps).

## Wave 2 additions (w13)

Rows below were cited by `rg` against the current shared checkout; every test name and `file:line` was
read from source, not inferred. This is an index of already-existing local tests only, so no
`.vendor/**` precedent is needed and no vendor code is a dependency or runtime input. All rows are
UNBUILT: no `git`, `cargo`, build, test, Python or sleep loop was run in this unit; the orchestrator
owns compilation and the post-batch run.

| Group | Test path (`file:line` — real test name) | Fault injected / coverage |
| --- | --- | --- |
| Config quarantine — core | `crates/flamewm-api/src/safe_config.rs:226` `quarantine_keeps_only_newest_copies` | Retention bounds (`KEEP_QUARANTINED`) on corrupt copies. |
| Config quarantine — explorer prefs | `crates/flamewm-explorer/src/prefs.rs:247` `corrupt_prefs_load_defaults_are_quarantined_and_survive_save`; `:269` `missing_prefs_are_silent_defaults_and_create_no_quarantine` | Corrupt `explorer.toml` quarantined, defaults used, bytes survive save; missing file silent. |
| Config quarantine — terminal store | `crates/flamewm-terminal/src/store.rs:587` `unparsable_terminal_toml_is_quarantined_and_defaults_used`; `:603` `oversized_terminal_toml_is_quarantined_and_defaults_used`; `:621` `missing_terminal_toml_is_silent_defaults` | Unparsable/oversized `terminal.toml` quarantined; missing file silent. |
| Config quarantine — workspaces | `crates/flamewm-platform/src/services/workspaces.rs:302` `corrupt_count_file_is_quarantined_and_bytes_survive_save`; `:331` `missing_count_file_is_silent_none` | Corrupt `workspaces.conf` count quarantined before save; missing file silent. |
| Config quarantine — terminal preferences | `crates/flamewm-terminal/src/app/preferences.rs:172` `corrupt_saved_config_is_quarantined_and_defaults_used` | Corrupt saved config loaded through `store::load_config` quarantined, defaults used. |
| Backend records (G1/G2) | `crates/flamewm-wm-wayland/src/backend_fallback_tests.rs:61` `each_failure_appends_a_record` (G1); `:161` `headless_entry_appends_a_record` (G2) | One record per injected DRM/nested failure; one `headless` record on entering the headless step. |
| Session HOME fallback | `crates/flamewm-wm/src/bin/flamewm-session.rs:247` `mod home_resolution_tests` — `:261` `env_home_wins_and_is_not_a_fallback`; `:276` `unset_or_empty_home_uses_passwd_entry_for_current_uid`; `:296` `fails_when_passwd_has_no_usable_home_and_never_guesses_root`; `:305` `fails_when_status_is_unreadable`; `:313` `passwd_parsing_matches_uid_exactly_and_skips_unusable_homes`; `:336` `real_uid_is_the_first_uid_field` | Unset/empty `$HOME` falls back to the current-uid passwd entry; no root guess, no silent fallback on unusable passwd/unreadable status. |
| Lock — X11 respawn | `crates/flamewm-x11-desktop/src/lock.rs:127` `respawn_until_started<T>` — `:472` `failed_respawns_retry_at_crash_loop_delay_and_ask_loginctl_once`; `:494` `immediate_respawn_skips_sleep_and_loginctl` | Injected spawn/loginctl/sleep closures: crash-loop delay retry and loginctl once; immediate respawn skips both. |
| Lock — fail-safe | `crates/flamewm-lock/src/fail_safe.rs:49` `every_fault_combination_shows_a_lock_and_never_unlocks`; `:75` `each_single_failure_uses_the_solid_colour_without_unlocking`; `:105` `render_fault_keeps_a_cover_and_the_reducer_stays_locked`; `:130` `a_healthy_lock_keeps_the_normal_background_and_stays_locked` | Pure `decide()` table: every render/pam/background fault combination keeps the cover and never unlocks. |
| Lock — PAM dlopen | `crates/flamewm-lock/src/pam.rs:497` `missing_libpam_stays_locked`; `:504` `pam_load_failure_decision_keeps_the_cover_and_loses_only_auth`; `:517` `dlopen_miss_leaves_the_reducer_locked_with_a_cover` | dlopen miss degrades only `lock_auth`; lock surface stays covered, reducer stays locked. |
| Lock — document / wallpaper | `crates/flamewm-lock/src/document.rs:1105` `empty_image_uses_builtin_background_and_undecodable_falls_back_to_colour`; `:1317` `self_test_document_decodes`; `:1352` `undecodable_wallpaper_still_yields_an_opaque_full_size_cover` | Undecodable/missing wallpaper yields an opaque full-size cover; builtin background fallback. |
| X11 startup | `crates/flamewm-wm-x11/src/wm/startup_fault_tests.rs` (`mod` at `crates/flamewm-wm-x11/src/wm/mod.rs:593`) — `:22` `randr_present_with_successful_selection_has_no_fault`; `:27` `randr_absent_degrades_to_root_geometry_with_a_record`; `:72` `second_wm_on_root_selection_is_fatal_with_a_clear_record`; `:89` `missing_extension_degrades_with_a_record`; `:189` `lost_connection_is_fatal_with_a_blast_radius_record`; `:287` `startup_failure_line_names_each_stage`; `:305` `missing_display_is_named_in_the_connect_cause` | Missing RandR/extensions, second WM on root, lost connection, missing display; each degrades with a classified record/fatal line. |
| Resize budget — snap preview | `crates/flamewm-wm-wayland/src/snap_preview.rs:261` `hundred_translation_motions_rasterise_once`; `:282` `one_second_of_outline_resize_stays_within_raster_budget` | One simulated second of 1 kHz outline resize stays within the 70-raster budget with injected clock. |
| Resize budget — X11 configure pacing | `crates/flamewm-wm-x11/src/wm/effects.rs:596` `one_second_of_1khz_resize_stays_budgeted_and_flushes_last_batch` (module at `:576` `resize_configure_pacing_tests`) | One second of 1 kHz resize: configure batches <= 70 and trailing deferred batch flushed after the last event. |
