# u37 — bundled DejaVuSansMono as last-resort font

Status: **UNBUILT**. No `cargo`, `git`, build or test command was run in this unit; the orchestrator
verifies after the batch lands.

The bundled asset and its Bitstream Vera / DejaVu license were copied by an earlier worker. This
unit wires them into `find_font()` only. `include_bytes!` resolves relative to
`crates/flamewm-render-core/src/glyphs.rs`: `../../../assets/fonts/DejaVuSansMono.ttf` reaches the
workspace-root `assets/fonts/` file, so a workspace build finds it. `rg 'assets' Cargo.toml
crates/flamewm-render-core/Cargo.toml` returns no matches; Cargo's default include set keeps all
non-ignored files, and `.gitignore` does not exclude `assets/fonts/`. System-font discovery and the
`Some(None)` cache are unchanged, so machines with a usable font parse nothing extra and allocate
nothing extra.

## Rows — file:line, +335 KB binary, change, risk

| file:line | +335 KB binary | Change | Risk |
| --- | --- | --- | --- |
| `crates/flamewm-render-core/src/glyphs.rs:184-203` | Yes — `include_bytes!` embeds the 343,140 B (≈335 KiB) TTF as a static in every binary linking this crate; bytes are neither parsed nor copied when a system font is found. | Added a nested `static FALLBACK: OnceLock<Option<Font>>`; the closure runs only after all system candidates fail, parses the embedded TTF once, and `find_font()` returns one clone of that cached `Font`. | Low — parse and the one clone are confined to the failure path. The `Option<Font>` type and both callers (`with_run` cache, test `OnceLock`) are unchanged. A `Font` clone duplicates its tables once on a font-less machine; it is dropped with the process. |
| `crates/flamewm-render-core/src/glyphs.rs:190-195` | No additional bytes — reuses the existing `flamewm:` record. | Moved u35's one-time `NO_FONT_LOG` (`Once` + `eprintln!`) into the fallback branch and reworded it to `no system font found; using bundled DejaVuSansMono fallback`, keeping the `blast_radius` tag. | Low — still one stderr line per process, now reporting fallback use instead of skipped text. If the embedded TTF itself fails to parse, no line is emitted; that asset is shipped and license-checked, so this is a corrupted-asset case. |
| `crates/flamewm-render-core/src/glyphs.rs:274-279` | No | Removed the old `with_run` `NO_FONT_LOG` block so the single record above is the only missing-font diagnostic; `FontCache` construction is otherwise untouched. | Low — `with_run` still returns `None` and skips text when no font is usable; only the diagnostic moved. |
| `assets/fonts/DejaVuSansMono.ttf`, `assets/fonts/LICENSE-DejaVu.txt` | Yes — 343,140 B TTF plus license text, already present before this unit. | Read-only input; no edit. | Licensing: DejaVu font license ships alongside the binary asset. Packaging: a standalone `cargo package -p flamewm-render-core` would not carry the out-of-crate asset; workspace builds resolve it. |
| `Cargo.toml`, `crates/flamewm-render-core/Cargo.toml` | No | Checked only; no manifest edit. No `assets` entries, so default Cargo inclusion applies. | If a future release packages this crate alone, an explicit manifest include or in-crate asset copy is needed. Not a workspace-build risk. |
| `docs/swarm/units/u37-fallback-font.md` | No | This record. | None — documentation only. |

## Verification

Not run by this unit (see Status). Static checks only: source paths, `fontdue::Font` `Clone` derive,
asset size and `.gitignore` were inspected; no build, test, `git` or `cargo` command was executed.

PARTIAL — `find_font()` now falls back to the bundled DejaVuSansMono on the failure path only, parsed
once behind a `OnceLock`, with the u35 one-time record reworded to report fallback use. Machines with
a system font keep identical behavior and RAM; the binary grows by the ≈335 KiB asset. The change is
UNBUILT by contract, so the orchestrator must run the crate checks after the batch lands.
