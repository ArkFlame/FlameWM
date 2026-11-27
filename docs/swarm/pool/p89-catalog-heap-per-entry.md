# Unit p89-catalog-heap-per-entry
STATUS: PARTIAL

ApplicationCatalog (crates/flamewm-applications/src/app_catalog.rs, desktop_entry.rs) costs about 25 KB of heap per parsed .desktop entry (225 entries = 5.6 MB in flamewm-shell after warm_application_catalog_in_background). Read the DesktopEntry/DesktopApplication structs and the parse path. List every field kept per entry (raw text, localized Name/Comment/Keywords maps, Exec, Actions, MimeType lists, duplicated Strings, HashMap per entry). Keep only what start menu, launcher search and dock actually read; intern or drop the rest; shrink_to_fit the Vecs/Strings after parse; free the parse buffer. Output of start menu and search must stay identical. At most 3 edits, UNBUILT.

## Result
STARTED
STATUS NOTE: edits UNBUILT.

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| crates/flamewm-applications/src/desktop_entry.rs:259 (list_value) | Categories/Keywords Vecs from split+collect keep spare capacity | shrink_to_fit before return | none, same contents |
| crates/flamewm-applications/src/app_catalog.rs:28 (by_id field), :82 (rebuild_indexes), :127 (find) | by_id BTreeMap<String,usize> duplicated every entry id (one String plus map node per entry) | removed the map; find() does a linear scan over applications (ids unique via `seen`, same first match) | launch-time lookup is O(n) over ~225 entries, negligible |
| crates/flamewm-applications/src/tests.rs:514 | struct literal used removed by_id field | deleted the field initializer | test-only |

Not changed, kept on purpose: DesktopEntry.exec (read by expand_exec_with_path at launch), DesktopApplication fields (all read by search at app_catalog.rs search, or by launch.rs/opener.rs icon paths), application.id (asserted in tests, duplicates entry id but type lives outside this unit). Not measured: the 25 KB/entry figure is not explained by these structs; the removed map is roughly 225 Strings plus BTreeMap nodes, a few KB total. Remaining heap is likely in the warm-up path, not the parse structs. Needs a heap measurement (scoped, one component) before claiming the 5.6 MB drop.

PARTIAL: removed the by_id index and spare Vec capacity, which cuts a few KB of heap per catalog without changing start menu or search output. The 25 KB per-entry target is not reached by these edits; the struct fields are all still read. Edits are UNBUILT and not measured; next step is a scoped heap probe of ApplicationCatalog::discover before and after.
