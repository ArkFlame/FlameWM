# Unit p22-gpu-helper-watchdog
STATUS: DONE

crates/flamewm-render-x11/src/accel: a hung GPU helper must be detected by a deadline and the renderer must fall back to CPU raster, with a failure record; audit existing deadline logic against fault_tests.rs and fix gaps.

## Result
STARTED
