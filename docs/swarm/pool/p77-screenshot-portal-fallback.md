# Unit p77-screenshot-portal-fallback
STATUS: DONE

Crate flamewm-screenshot: no portal, no wlr-screencopy, no X: ordered fallback chain, one report with what works, never panic. Test the pure chain chooser.

## Result
(crates/flamewm-screenshot/src/portal.rs:117, no typed fallback-chain model existed, added Backend and Availability value types with stable as_str names, UNBUILT)
(crates/flamewm-screenshot/src/portal.rs:148, no pure chooser existed, added choose_chain returning portal then screencopy then x11 only when available with no IO and no panic, UNBUILT)
(crates/flamewm-screenshot/src/portal.rs:160, availability was implicit in the capture path, added probe_availability using env presence with portal_client=false because BUS_NAME is our own impl-portal backend, UNBUILT)
(crates/flamewm-screenshot/src/portal.rs:170, failures were only logged ad hoc, added report_attempts rendering exactly one semicolon-joined line of backend statuses, UNBUILT)
(crates/flamewm-screenshot/src/portal.rs:183, capture_full_output was a 2-step implicit chain with no report, rewrote it to walk the ordered chain record every outcome including screencopy Err and emit one consolidated report, UNBUILT)
(crates/flamewm-screenshot/src/portal.rs:428, chain chooser had no coverage, added nine unit tests for all-available each-unavailable only-x11 none available ordering purity backend names and the single-line report, UNBUILT)
(docs/swarm/pool/p77-screenshot-portal-fallback.md:1, unit was OPEN, finalized status and result rows, UNBUILT)

PASS - the ordered portal/screencopy/x11 chain, pure chooser, single report and unit tests are all in place with no unwrap/expect added; no build or test command was run per unit rule, so every row is UNBUILT and the result is unverified by compilation.
