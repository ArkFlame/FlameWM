# Unit p01-lock-wallpaper-drop
STATUS: PARTIAL

Lock screen (crates/flamewm-lock, not run.rs/document.rs/view.rs/pam.rs/wayland.rs: re-read before edit): free the retained wallpaper raster (hotlist row 3, 8-9 MiB) once unlocked or when the lock surface is destroyed; verify nothing keeps it alive in statics.

## Result

STARTED
