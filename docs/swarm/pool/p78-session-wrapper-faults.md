# Unit p78-session-wrapper-faults
STATUS: OPEN

scripts/flamewm-session-wrapper and scripts/test-wrapper-faults: read both; list each fallback (no Wayland backend, no X, no GPU, missing binary, bad env) and check the fault script covers it; add missing cases to the script (bash, no python). Do not run it.
