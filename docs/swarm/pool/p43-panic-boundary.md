# Unit p43-panic-boundary
STATUS: OPEN

Crates flamewm-shell, flamewm-desktop, flamewm-lock: wrap each subsystem thread entry in the session-core guard so a panic becomes a FailureRecord and a restart, not a dead session. List thread::spawn sites lacking it; fix up to 3.
