# Unit p12-hotkey-grab-fail
STATUS: DONE

crates/flamewm-platform hotkeys / wm-x11 key grabs: when a key combination is already grabbed by another client, degrade that binding with one record, never abort startup; audit and fix.

## Result

STARTED

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-x11-desktop/src/shortcuts.rs:373-375` | `commit` doc said any failed required grab aborts and restores the previous set | doc now states an already-held grab is logged once and that binding is skipped even when not optional; other required failures still roll back | UNBUILT |
| `crates/flamewm-x11-desktop/src/shortcuts.rs:395` | `Err(error) if entry.optional` sent a required `Conflict` (BadAccess / "already grabbed by another client") down the rollback-and-return path, failing the whole grab set and aborting startup | guard is now `entry.optional || error.code == ErrorCode::Conflict`, so a conflict degrades only that binding; the existing `skipped` set drops all its NumLock/CapsLock variants and one log record is emitted | UNBUILT |
| `crates/flamewm-x11-desktop/src/shortcuts.rs:397` | log said "optional shortcut" although the path now covers required bindings | message is the neutral "shortcut for {} was not grabbed: {}" | UNBUILT |
| `crates/flamewm-x11-desktop/src/shortcuts.rs:764` | test asserted a required conflict rolls back and surfaces `ErrorCode::Conflict` | test now asserts the conflicted binding is skipped while the other binding still commits | UNBUILT |
| `crates/flamewm-x11-desktop/src/shortcuts.rs:890` | test asserted a required conflict releases every optional grab on rollback | test now asserts the required conflict degrades while the optional binding stays grabbed | UNBUILT |

PASS — the audit found one real defect: `ShortcutGrabs::commit` treated an already-grabbed key as a fatal error for required bindings, rolling back the entire set and failing `commit_startup_shortcuts`; now any `Conflict` degrades that single binding with one log record and the rest of the effective hotkey set still grabs, matching the existing optional-binding behavior and the degraded-startup precedent in `flamewm-wm-x11/src/wm/construct.rs`. Non-conflict required failures still roll back and return their error, so no gate was loosened; no visuals or generated output were touched. Every edit is UNBUILT: this shared-checkout brief forbids `git`, `cargo`, builds and test runs, so compile and test verification (`cargo test -p flamewm-x11-desktop`, `cargo test -p flamewm-platform`) was not run and is left to the orchestrator.
