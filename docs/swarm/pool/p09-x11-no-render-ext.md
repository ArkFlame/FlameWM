# Unit p09-x11-no-render-ext
STATUS: PARTIAL

crates/flamewm-render-x11 and wm-x11: when RENDER, COMPOSITE, DAMAGE or XFIXES extensions are missing, fall back to the plain core-protocol path with one failure record; list each extension query and what happens now; fix gaps.

## Result

### Extension queries and current behavior (investigation)
| extension | query site | when missing, today |
| --- | --- | --- |
| RENDER | `crates/flamewm-render-x11/src/xrender.rs:104` — `DynamicLibrary::open("libXrender")` + symbol loads; no protocol `XRenderQueryExtension` | `crates/flamewm-render-x11/src/app/mod.rs:552-582`: `xrender=None`, `choose_provider` returns `GraphicsProvider::XCore`, alpha becomes `OpaqueFallback`; one record `FLAMEWM_RENDER_ALPHA_BACKEND opaque-fallback reason=...`. Core fallback already works; no gap. |
| COMPOSITE | `crates/flamewm-wm-x11/src/wm/mod.rs:821-830` — `composite_query_version` (x11rb) | `crates/flamewm-wm-x11/src/wm/construct.rs:186` sets `redirect_frames=false` silently. Gap: no failure record. |
| DAMAGE | `crates/flamewm-render-x11/src/app/live_backdrop.rs:59-67` — `XDamageQueryExtension` | `crates/flamewm-render-x11/src/app/live_backdrop.rs:295-299`: `.ok()?` returns `None`, desktop layer kept. Gap: no failure record. |
| XFIXES | no production query in either crate; `crates/flamewm-session-core/src/diagnostics.rs:139-142` lists it Info, "not used by the WM sources" | Never audited at startup. Gap: absence is invisible. |

### Edits (all UNBUILT)
| file:line | finding | change | risk | status |
| --- | --- | --- | --- | --- |
| `crates/flamewm-wm-x11/src/wm/construct.rs:31-60` | no shared definition of the four optional rendering extensions | added `CORE_FALLBACK_EXTENSIONS` table and pure `core_fallback_fault` reducer built on `missing_extension_fault` | compile-only risk; no behavior change until called | UNBUILT |
| `crates/flamewm-wm-x11/src/wm/construct.rs:132-144` | COMPOSITE/DAMAGE/XFIXES degraded silently | `become_wm` now queries all four via core-protocol `conn.query_extension` (no new x11rb features) and emits exactly one `flamewm: <EXT> absent; blast radius: ...` record per missing extension; existing core fallbacks untouched | four one-time startup round-trips; one record per absent extension (not one combined line) | UNBUILT |
| `crates/flamewm-wm-x11/src/wm/construct.rs:36-54` | blast-radius wording drifted from authoritative diagnostics | aligned strings with `flamewm-session-core::X_EXTENSIONS`, including accurate XFIXES "not used by the WM sources" | log text only; visuals unchanged | UNBUILT |
| `crates/flamewm-wm-x11/src/wm/startup_fault_tests.rs:4-7,108-133` | new pure helper untested | added two pure tests: audit covers exactly RENDER/COMPOSITE/DAMAGE/XFIXES, and absent->one fault / present->none | test imports compile-only risk | UNBUILT |

Reference-first: checked `.vendor/kwin-master/src` for an extension-presence fallback precedent; no direct match, so the existing `missing_extension_fault` + one-record convention was reused. No build, cargo, git, python or runtime check was run per worker rules.

PARTIAL — investigation and three scoped production edits plus tests are complete; each absent rendering extension now produces one startup failure record while the pre-existing plain core-protocol fallbacks remain unchanged. All edits are UNBUILT and unverified because worker rules forbid cargo; the coordinator must run one build/test pass to confirm the new `conn.query_extension` loop and test imports compile.
