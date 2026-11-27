# Unit p62-panic-hook-forced
STATUS: OPEN

Crates flamewm-session-core diagnostics and each binary main: verify every subsystem thread spawn is wrapped by the panic guard and restarts or degrades, never aborts the session. List unwrapped spawns (rg 'thread::spawn|Builder::new'), wrap the top 5. Max 3 edits.
