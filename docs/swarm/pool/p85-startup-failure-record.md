# Unit p85-startup-failure-record
STATUS: OPEN

Crates flamewm-session and flamewm-session-core: every startup stage has a named stage, error code, cause, blast radius (what works / what degraded) written to the session log and a failure record file; list stages lacking one and add the top 3. Pure formatting function with a test.
