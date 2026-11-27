# Unit p89-catalog-heap-per-entry
STATUS: OPEN

ApplicationCatalog (crates/flamewm-applications/src/app_catalog.rs, desktop_entry.rs) costs about 25 KB of heap per parsed .desktop entry (225 entries = 5.6 MB in flamewm-shell after warm_application_catalog_in_background). Read the DesktopEntry/DesktopApplication structs and the parse path. List every field kept per entry (raw text, localized Name/Comment/Keywords maps, Exec, Actions, MimeType lists, duplicated Strings, HashMap per entry). Keep only what start menu, launcher search and dock actually read; intern or drop the rest; shrink_to_fit the Vecs/Strings after parse; free the parse buffer. Output of start menu and search must stay identical. At most 3 edits, UNBUILT.
