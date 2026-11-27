# Unit p42-gpu-software-fallback
STATUS: OPEN

Crates flamewm-render-x11 and flamewm-wm-wayland: GPU init failure (no EGL, no GBM, driver crash): fall back to software/CPU raster with one report. Audit and fix any path that aborts instead.
