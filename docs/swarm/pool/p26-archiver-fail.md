# Unit p26-archiver-fail
STATUS: DONE

Crates flamewm-archiver and flamewm-archiver-app: corrupt/unreadable archive, missing external tool, disk full: return an error value shown in UI, never panic or unwrap; fix unwraps on external input.

## Result

Investigation (rg/Read): the archive core already returns ArchiveError for every corrupt/unreadable archive (zip::ZipError / Io), a missing 7z tool (Unsupported from find_tool), and a disk-full write (Io from Writer::finish or fs::rename). The app converts each into a visible status through Reply::Listed ("Cannot open: ...") and Reply::Done ("Failed: ..."). No production unwrap/expect acts on external input in either crate; the only unwrap/expect calls left are in build.rs and #[cfg(test)] code. Reference check: .vendor/ark-master reports corrupt archives as error signals instead of crashing, matching this design. The reachable panic paths were unchecked indexing and row arithmetic fed by UI action strings.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-archiver-app/src/window.rs:348 | self.entries[index] could panic if selection held a row from an older listing | iterate entries.get(index) via filter_map | UNBUILT |
| crates/flamewm-archiver-app/src/window.rs:392 | page * ROWS + row could overflow for an arbitrary ar.rowN action string | reject row >= ROWS before arithmetic | UNBUILT |
| crates/flamewm-archiver-app/src/window.rs:438 | a fresh listing replaced entries but kept stale selection and page | clear selected and reset page when entries are replaced | UNBUILT |

PASS: all three named failure modes already return error values that the UI displays; the three hardening edits remove the only production panic paths reachable from external input. No build or test was run because this shared checkout forbids cargo; all edits are UNBUILT and the orchestrator must verify them.
