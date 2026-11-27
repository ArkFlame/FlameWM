# Unit p25-clipboard-fail
STATUS: PARTIAL

Crate flamewm-clipboard: audit startup and runtime failure paths (no display, no selection owner, oversize payload, dead peer). Each must degrade (feature off, one report line) never abort or spin; add one fault test where cheap.

## Result
STARTED

| file:line | finding | change | risk | state |
| --- | --- | --- | --- | --- |
| crates/flamewm-clipboard/src/owner.rs:31-43 | Failure after readiness (dead X peer, oversize INCR size conversion, dropped connection) sent Err into a channel whose receiver the caller had already consumed, so the owner thread ended with no report line. | Report once on stderr when the Err send fails (readiness already delivered), and drop(ready_rx) right after recv() so a late failure cannot be silently buffered. | One stderr line per late failure; pre-readiness failure still returns to the caller, so no duplicate line. | UNBUILT |
| crates/flamewm-clipboard/tests/two_instances.rs:114-129 | No fault coverage for the no-selection-owner case on a fresh display. | Added absent_selection_owner_reads_as_none: get_text, get_primary_text and get_files must be Ok(None) on a fresh Xvfb display. | Test skips without Xvfb; assumes the fresh display has no owner. | UNBUILT |

Reference: .vendor/smithay-master/src/xwayland/xwm/selection.rs:133-157 and mod.rs:1992-1997 show a failed transfer (SelectionNotify with NONE) dropped and an empty final INCR chunk ending an outgoing transfer, never a retry loop; FlameWM already mirrors that in send_next_chunk and read_selection. x11rb packet_reader.rs:75-82 returns UnexpectedEof on a dead socket, so next_event cannot spin.

PARTIAL Work is complete to the brief's scope; builds and tests were explicitly forbidden, so both edits are UNBUILT and unverified. The other three named paths already degrade: the no-display keeper prints one line and ends, oversize text is rejected by ClipHistory::push and INCR-chunked by the owner, and a dead peer surfaces as an error from wait_for_event/poll_for_event rather than an abort or spin.
