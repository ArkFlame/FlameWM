# Unit p28-ribbons-tray-fail
STATUS: DONE

Crates flamewm-ribbons and flamewm-tray: tray host absent, malformed StatusNotifier item, icon missing: skip the item with one report, never abort the shell.

## Result
STARTED

All findings are from static source inspection. No git, cargo, builds, tests, python or sleep loops were run; every production edit below is UNBUILT pending the orchestrator's build and visual check.

Reference-first check: `.vendor/LingmoOS-main/core/xembed-sni-proxy/sniproxy.cpp:76-79` reports one failed SNI registration and continues without aborting; `.vendor/LingmoOS-main/core32/dataengines/statusnotifieritem/statusnotifieritemsource.cpp:99` reports one invalid notifier id; that source falls back from an absent `IconPixmap` to `IconName` and finally to a null icon (lines 292-308) instead of failing the shell.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-shell/src/tray_host.rs:184-201 | An absent watcher/host already degrades: one `eprintln!`, `None`, shell runs without a tray; the reactor source is only installed when a host exists (main.rs:1092-1106). | none — verified, no edit needed | None. |
| crates/flamewm-tray/src/watcher.rs:218-229 | A malformed `RegisterStatusNotifierItem` (non-string argument, or a registration `parse_registration` rejects) was silently returned as `Unhandled`. | EDIT (UNBUILT): one `eprintln!` at each rejection, then the existing skip. Reply and registry behavior unchanged. | One stderr line per rejected registration; a hostile peer spamming registrations can fill the log. |
| crates/flamewm-tray/src/watcher.rs:257-263 | A valid item refused because `MAX_ITEMS` was full was silently skipped. | EDIT (UNBUILT): one `eprintln!` naming the item, then the existing skip. | One stderr line per refused item at capacity. |
| crates/flamewm-tray/src/watcher.rs:268-284 | A malformed `RegisterStatusNotifierHost` request, or a host refused at `MAX_HOSTS`, was silently skipped. | EDIT (UNBUILT): one `eprintln!` at each rejection, then the existing skip. | One stderr line per refused host registration. |
| crates/flamewm-tray/src/watcher.rs:4,73,294-307,377-382 | An item with neither `IconName` nor a valid `IconPixmap` was projected with the generic glyph but never reported. | EDIT (UNBUILT): added `iconless_reported: BTreeSet<ItemKey>` and one deduplicated `eprintln!` on the first iconless props reply; the set is pruned with the registry on owner loss. Projection, glyph and pixels unchanged. | One stderr line per iconless item; set size is bounded by registered items. |

PASS — Investigation and all three in-scope production changes are complete: malformed items, refused hosts and iconless items are now skipped or degraded with exactly one report line and no abort, while the already-correct absent-host path was verified and left untouched. No gate was loosened and no visual output changed. All edits are UNBUILT: the brief forbade git, cargo, builds and tests, so the orchestrator must compile the shared checkout and run the flamewm-tray tests before release.
