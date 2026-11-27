# Unit p01-lock-wallpaper-drop
STATUS: PARTIAL

Lock screen (crates/flamewm-lock, not run.rs/document.rs/view.rs/pam.rs/wayland.rs: re-read before edit): free the retained wallpaper raster (hotlist row 3, 8-9 MiB) once unlocked or when the lock surface is destroyed; verify nothing keeps it alive in statics.

## Result

| file:line | finding | change | risk |
| --- | --- | --- | --- |
| `crates/flamewm-lock/src/document.rs:194-224` | Built-in wallpaper decode was held for the whole process in `OnceLock<Option<RgbaImage>>`; the ~17 MiB natural-size raster was never freed on unlock (hotlist row 3) | Replaced with module-level `BUILTIN_CACHE: OnceLock<Mutex<Option<Arc<RgbaImage>>>>` and added `release_wallpaper_caches()` to clear it and call `decode_cache::release_shared_raster()`; **UNBUILT** | A later lock show re-decodes the same WebP; pixels are identical. No visual change. |
| `crates/flamewm-lock/src/document.rs:245-248` | `cover_rgba` takes `&RgbaImage`, but the cache now yields `Arc<RgbaImage>` | Pass `&builtin_background()?` so deref coercion keeps the same cover-scale call; **UNBUILT** | None; the presented pixels are unchanged. |
| `crates/flamewm-lock/src/run/tick.rs:79-86` | On successful authentication the lock destroyed its surfaces but left the decoded source rasters resident in statics | Call `crate::document::release_wallpaper_caches()` on the unlock branch after `destroy_surfaces`; **UNBUILT** | If surface destruction errors, the release is skipped, but the process then exits and frees all statics. No visual change. |

Static verification: `rg` over `crates/flamewm-lock/src` finds only the new clearable `BUILTIN_CACHE` and `decode_cache::MEMORY`; both are now released on unlock. The Wayland blurred-background cache is a local `Rc<RefCell>` dropped when `wayland::run` returns, not a static. `.vendor/sddm-develop/src` has no wallpaper-cache precedent (only an image credit in a theme README), so no upstream clone was needed. No `git`, `cargo`, build, test, Python or sleep-loop command was run.

PARTIAL — both production edits are applied and every retained wallpaper raster now has an explicit release on unlock, but no build or test was permitted in this shared-checkout unit, so all edits are marked UNBUILT and the orchestrator must compile the `flamewm-lock` and `flamewm-image-core` crates and run their tests before treating this slice as verified.
