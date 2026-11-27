# Unit p18-usage-poll-backoff
STATUS: OPEN

AI usage provider polling (rg -l UsageProvider crates): failures and missing credentials must back off and never busy-poll; verify idle wakeups are zero when disabled; fix.
