# u158 notification icon poll must be bounded
Area: shell idle wakeups. File: crates/flamewm-shell/src/main.rs (`poll_icon`, ~535-576).
Finding: while `icon_wait` is true the reactor is re-woken every 15 ms. It clears only when a matching raster arrives or the loader is absent. If the icon loader drops the request or answers for a different size/name, the 66 Hz poll lasts as long as the notification is visible.
Change: give `icon_wait` a deadline (e.g. 1 s from the request at line ~530); on expiry clear it and keep the fallback glyph. Prefer waking from the loader's own channel over polling if `IconLoader` exposes a fd/waker.
Acceptance: unit test on the deadline logic with an injected clock; no feature or visual change when the icon arrives in time.
Status: DONE (commit below). `icon_wait` is now `Option<Instant>`; `poll_icon` clears it after 2 s (`ICON_WAIT_LIMIT`) and keeps the fallback glyph. The lost-answer path remains unconfirmed, the bound makes it harmless.
