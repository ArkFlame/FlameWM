# read-D2: `UiDocumentView::runtime_document`

## Decision

Keep the method for now; do not remove it without an explicit public-API deletion decision.

## Evidence

- `rg -n "runtime_document\\(" crates` finds only the definition in `crates/flamewm-ui-document/src/template.rs:94`; no in-workspace callers use this accessor.
- `UiDocumentView` is publicly re-exported from `crates/flamewm-ui-document/src/lib.rs`, and `runtime_document` is a `pub` inherent method. The view is also part of public callback signatures in `flamewm-ui-x11` and downstream crates can access the crate API.
- Removing it is therefore a source-breaking change for external users even though the repository has zero call sites. A downstream-usage audit is not available from this checkout; local search cannot establish that the API is unused externally.

## Conclusion

This is dead within the workspace, but not proven dead as a public API. Retain it unless an explicit compatibility/deprecation decision authorizes removal. No source change made.
