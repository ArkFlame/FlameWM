# Unit p70-x11-extension-missing
STATUS: PARTIAL

Crate flamewm-wm-x11: missing X extensions (RANDR, COMPOSITE, XInput2, SHAPE, XKB), lost X connection, second WM already running: degrade per extension, one report with blast radius, no panic. Add a pure-decision test per case. Max 3 edits.
## Result
- (crates/flamewm-wm-x11/src/wm/construct.rs:100-156, finding: only RENDER/COMPOSITE/DAMAGE/XFIXES were audited and faults were printed as separate lines; change: added INPUT_FALLBACK_EXTENSIONS for XInputExtension/SHAPE/XKB, StartupReport aggregator with one render() carrying blast radii, and pure connection_lost_fault; risk: new query_extension calls on every startup, no panic path, UNBUILT)
- (crates/flamewm-wm-x11/src/wm/construct.rs:158-232, finding: become_wm emitted one eprintln per fault; change: collects RandR, core, input and second-WM faults into one StartupReport and prints it once, second WM still fatal via ReplyError; risk: report now includes several connection-related lines if the first X request fails, UNBUILT)
- (crates/flamewm-wm-x11/src/wm/mod.rs:380-385, finding: lost X connection was converted to io error with no operator report; change: any_conn_err pushes connection_lost_fault and emits the same single report before returning the error; risk: no panic, error type unchanged, UNBUILT)
- (crates/flamewm-wm-x11/src/wm/startup_fault_tests.rs:136-199, finding: no pure-decision coverage for input extensions, connection loss, or report consolidation; change: added per-case tests for XInput2/SHAPE/XKB, audit-once, fatal connection loss and one-message rendering; risk: tests written but not executed, UNBUILT)

PARTIAL: the requested pure decisions are implemented and wired so startup produces one report with blast radii for RANDR, COMPOSITE, XInputExtension, SHAPE and XKB, lost X connection, and a second WM, with no production panic. Reference precedent: .vendor/xfwm4-master/src/display.c:218-306 queries each extension independently and degrades with a warning; .vendor/xfwm4-master/src/screen.c:101 reports a second WM on BadAccess. All four edits are UNBUILT because this worker is forbidden from running cargo; no build or test was executed. Remaining gap: randr_resource_fault in lib.rs still emits its own runtime record outside the startup handshake, so a mid-session RandR resources failure is not folded into the startup report.

