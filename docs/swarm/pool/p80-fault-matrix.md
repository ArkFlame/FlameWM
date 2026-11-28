# Unit p80-fault-matrix
STATUS: OPEN

Read docs/FAULT_TESTS.md and rg for fault/fallback tests in crates/*/src (rg 'fn .*(fallback|missing|corrupt|degrad|absent)'). Produce a matrix in docs/FAULT_TESTS.md: each fallback in the goal (backend DRM/nested/X11/minimal, GPU hw/sw/cpu, missing lib/font/icon/theme/portal, corrupt settings, panic restart) -> test path or GAP. Docs only; list top 5 gaps as next units at the end.

## Result
STARTED
