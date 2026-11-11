# Group A backlog audit (look-radius, cmp-keyring, cmp-bt, net-vpn)

Base: origin/main `6e589404` (worktree `/tmp/opencode/wt-groupa`, detached). Static source inspection only
(`grep`, `sed`, `wc`); no build, gate or runtime run was made for this audit. No code changed, so no code gate applies.
Rows are not edited here; `docs/BACKLOG.md` is reconciled by the orchestrator.

## Verdicts

| row | size | premise verdict | disposition |
| --- | --- | --- | --- |
| look-radius | M | verified | blocked: scale choice is a design decision |
| look-radius-large | S | verified | blocked: keep-or-map is a design decision |
| cmp-keyring | S | partly verified; cited line is wrong | blocked: security-sensitive, live Secret Service unverified |
| cmp-bt | L | verified | blocked: L size and security-sensitive `bt-agent` |
| net-vpn (follow-ups) | L | verified (slices 1-4 present) | blocked: no defined follow-up slice; live NM not allowed on host |

## look-radius (M): blocked, decision needed

Premise checked:
- `ui/base/tokens.css:4` defines `--radius-sm:3px; --radius-md:6px; --radius-lg:9px; --radius-xl:11px`.
- `ui/base/tokens.css:7` defines `--r-sm:4px; --r-md:8px; --r-lg:12px; --r-pill:999px`.
- Both scales are live. Usage counts over `ui` and `crates`: `var(--radius-*)` 33 lg, 22 md, 26 sm, 27 xl (108 total);
  `var(--r-*)` 9 lg, 19 md, 15 pill, 18 sm (61 total).
- `ui/base/tokens.css:2` says legacy `--radius-*` names are retired, yet `:4` still defines and 108 sites still use them.
  The file comment and the definitions disagree.
- 32 literal `border-radius: 3|6|9|11px` declarations remain in `ui` (`docs/backlog/look.md:35-41`).

Why it cannot proceed: which scale is canonical, and whether the 3/6/9/11px literals map to it, is a design choice
(`look.md` item 3 says "Choose one"). Any choice changes pixels on 169 token sites. Visual proof needs the look
harness, which is blocked for archiver, assistant and task-manager (`look.md:45-48`).

D-radius (decided by reference, 2026-11-11): canonical scale = COSMIC corner radii, the step set
`radius_xs 4px / radius_s 8px / radius_m 16px / radius_l 32px / radius_xl 160px` (plus 0 for none), read from
`.vendor/cosmic-epoch/cosmic-initial-setup/res/themes/mocha-dark.ron:210-215` (same values in every bundled theme).
Mapping rule for legacy literals: nearest COSMIC-compatible step within 1.5px per look.md (3 -> `--r-sm` 4px,
9 -> `--r-md` 8px, 11 -> `--r-lg` 12px). 6px sits 2px from both 4 and 8, so it stays literal. The retired
`--radius-*` comment in `ui/base/tokens.css:2` is not authoritative: `--radius-*` names stay until each use is
mapped. Status: not landed in this pass. The remap rewrites 32 declarations across ui pages and changes pixels, and
the look harness has no runtime capture for them, so it needs a visual run first (shortcut: static decision only;
revisit when a look-harness capture exists for the affected pages).

## look-radius-large (S): blocked, decision needed

Premise checked: 34 `border-radius` declarations at 14, 15, 16, 17, 18, 24, 26 or 28px in `ui` (grep count).
`look.md:29-34` records these as design decisions, not defects, and the look harness did not capture the affected
rules (`look.md:21-25`).

Decision needed: keep these large-surface radii as-is (close the row), or define a second token step for cards and
panels. No code change without that answer; a radius >= half the shorter side must never be mapped (`look.md:21-23`).

## cmp-keyring (S): blocked, security-sensitive

Premise checked:
- `crates/flamewm-applications/src/browser_secrets.rs:174-176` forces `XDG_CURRENT_DESKTOP=KDE`,
  `XDG_SESSION_DESKTOP=KDE`, `KDE_SESSION_VERSION` for browser child processes, with a comment that the session
  stays FlameWM. This matches the row.
- The row cites `crates/flamewm-applications/src/lib.rs:539`. That line is Exec field-code expansion
  (`%f/%u/%F/%U` handling), not the desktop override. The override location in `lib.rs` was not confirmed.
- Error text at `browser_secrets.rs:~170` says "Unlock the existing KWallet wallet", so the KDE identity directly
  selects KWallet over GNOME keyring for browser profile secrets.

Why it cannot proceed: the change alters which secret store a browser uses for its profile. Verifying GNOME
keyring or Secret Service behaviour needs a live bus, which the host rules forbid; a private mock Secret Service
bus has not been defined. Static audit `aba76518` already found no narrow fix.

D-keyring (decided by reference, 2026-11-11): keep the KDE identity for browser children until a mock Secret
Service test exists. Reference: the code and its error text already choose KWallet deliberately, and dropping the
identity would make an existing profile's secret store change, which the wait_for_wallet guard exists to prevent.
Test precedent for the mock bus: none in `.vendor` was adopted; the next slice must add that mock first. Row stays
closed-as-kept until then.

## cmp-bt (L): blocked, security-sensitive

Premise checked: `docs/backlog/bluetooth.md:3-34` matches the row. Device control, list and reply plumbing and the
`bt-ui` trust row are in source (`docs/backlog/bluetooth.md:26`). `bt-agent` (`org.bluez.Agent1` for PIN and
passkey) is not started (`bluetooth.md:30`) and is marked security-sensitive there.

Why it cannot proceed: size L exceeds the one-slice cap for this job, and `bt-agent` handles pairing secrets.
`bt-ui` is unverified against real BlueZ and needs a device or a mock BlueZ bus.

Decision needed: approve an owner-scoped slice plan for `bt-agent` (pairing agent registration, PIN/passkey reply
path) and a mock BlueZ bus for its tests. Without that, no slice is started here.

## net-vpn follow-ups (L): blocked, no defined slice

Premise checked: `docs/backlog/network.md:26-29` lists slices 1-4. Source confirms them:
- `crates/flamewm-integrations-linux/src/network_manager/vpn.rs`, `actions.rs`, `mod.rs` (`GetConnectionByUuid`,
  `ActivateConnection`, `DeactivateConnection`, `ActiveConnections` probes).
- `crates/flamewm-api/src/system.rs`, `crates/flamewm-platform/src/system.rs`, `crates/flamewm-control-wire/src/{action,reply,tests}.rs`
  (`VpnConnection` action and wire round trip).
- `crates/flamewm-wm-wayland/src/control/service.rs` and `crates/flamewm-integrations-linux/src/lib.rs` (X11 and Wayland arms).

The doc's own "Unverified" section (`network.md:33-34`) says nothing is runtime-proven: no live NetworkManager, no
VPN profile, no D-Bus round trip.

Why it cannot proceed: the remaining work is live verification, which needs a NetworkManager on a private bus
(host NM is forbidden). The doc defers follow-ups to "a dedicated slice plan" and no such plan exists, so there
is no unblocked S/M slice to land.

Decision needed: (1) define the follow-up slice (for example a mock NetworkManager on a private bus that drives
activate and deactivate end to end); (2) approve that mock bus. Until then no code commit.

## Checks run for this audit

- Worktree create: `git worktree add --detach /tmp/opencode/wt-groupa origin/main` (HEAD `6e589404`).
- Premise greps listed above (static only).
- No gate run: no code changed. Architecture guard run after this doc edit (see commit).

## Remaining risk

Every row is blocked. No code landed, so no runtime or unit claim is made for any group A row.
