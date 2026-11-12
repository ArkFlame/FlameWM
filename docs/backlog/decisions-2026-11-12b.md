# Backlog decisions, 2026-11-12 (b)

Resolves the eight rows still marked "decision needed" in `docs/BACKLOG.md`:
look-radius-large, read-U8, cmp-quick, read-U4, cmp-dock-overflow, perf-bgaudit,
pol-session, wrss-scratch. Format follows `docs/backlog/decisions-2026-11-12.md`.

Reference base: `.vendor/**` is read-only and lives in the main checkout
(`/run/media/linsaftw/SDD/GitHub/ArkFlame/flamewm-webrender/.vendor`). Only
`cosmic-epoch`, `vela-main`, `xfwm4-master`, `xfdesktop-master`, `mate-desktop-master`,
`icewm-master`, `starling-main` and `smithay-master` are present. KDE Plasma is not
in `.vendor`, so no KDE precedent is cited here. Licence rule: re-implement the
decision in FlameWM; never copy vendor code.

No code changed in this pass. Each row below is a decision or a close. None of
them needs a code slice now, so no gate ran.

## D-radius-L (look-radius-large): close, keep literal radii

- Premise: 34 `border-radius` declarations at 14-18, 24, 26, 28px in `ui`
  (`docs/backlog/group-a-audit.md`, look-radius-large).
- Precedent: COSMIC radius steps `xs 4, s 8, m 16, l 32, xl 160` px, read from
  `.vendor/cosmic-epoch/cosmic-initial-setup/res/themes/mocha-dark.ron:210-215`
  (verified in this pass).
- Decision: close the row. Keep the literal 14-28px values. The D-radius mapping
  rule (nearest COSMIC step within 1.5px) does not match: 14 and 18 are 2px from 16,
  and 24, 26, 28 are 4-8px from 32. No new token step is added.
- Guard: a radius >= half the shorter side is never mapped (`look.md:21-23`).
- Slice: none. Proof: none needed while values are unchanged.

## D-read-U4 (read-U4): close as won't-fix

- Premise: `drain` and `nonblocking_pair` exist in `crates/flamewm-finder/src/lib.rs:270,284`
  and `crates/flamewm-assistant/src/lib.rs:981,995`. They are wake-pipe production
  code, not test helpers (`docs/BACKLOG.md` row read-U4 already says so).
- Precedent: `AGENTS.md` names `flamewm-reactor` as the owner of native event sources.
  A shared wake-pipe helper would belong there, but it would need new dependency
  edges from finder and assistant. The helpers return `UiWakeDrain` from
  `flamewm-ui-core`, so the edge is not free.
- Decision: close as won't-fix. Two small copies in two apps beat a new dep edge
  (same reasoning as D-read-U8 in `decisions-2026-11-12.md`).
- Revisit when: a third wake-pipe user appears, or `flamewm-reactor` already exposes
  a wake source that both apps can take.
- Slice: none.

## D-read-U8 (read-U8): close as won't-fix, split the mis-scoped items

- Premise audit: `docs/backlog/read-u8.md` (`40b6059b`).
  - `release`/`claim` in `crates/flamewm-finder/src/instance.rs` and
    `crates/flamewm-assistant/src/instance.rs` are near-duplicates.
  - The two Drop items (`crates/flamewm-integrations-linux/src/device_watch.rs:115`,
    `crates/flamewm-explorer/src/refresh.rs:114`) are watcher teardown, not
    single-instance helpers.
- Decision: close the row as won't-fix for the pid-file pair. No shared home
  is created, so no new dependency edge. The two Drop items are not single-instance
  work and leave this row. Their dedupe, if wanted, is a separate watcher row.
- Revisit when: a third single-instance user appears, or a shared home crate is
  approved by the owner.
- Slice: none.

## D-cmp-quick (cmp-quick mic row placement): decided, keep as landed

- Current: the microphone row landed in `5091cdaa` (battery popup 160 -> 184 px).
  Placement follows the existing popup row order in `ui/shell/battery.html`.
- Decision: keep the placement. The row is last in the popup, so no existing row
  moves and no hit area changes.
- Vendor precedent: not checked for this placement. Basis is in-repo order only.
  Shortcut: placement is unmeasured at runtime; revisit when a runtime screenshot
  of the popup is captured.
- Slice: none. Proof: none run in this pass (`5091cdaa` was static only).

## D-dock-overflow (cmp-dock-overflow placement): decided, panel owns the cap

- Premise: layout owner is `ui/shell/panel.html`, `flamewm.css`, and
  `crates/flamewm-shell/src/projection/panel.rs`. `app/dock.rs` is the app dock and
  is off-target (`docs/BACKLOG.md` row cmp-dock-overflow).
- Landed: `ebad163d` (`+N` cell) and `d42da107` (cap from panel geometry).
- Decision: the cap and the `+N` cell stay in the panel projection. The app dock
  does not take overflow logic.
- Shortcut: the chrome budget is a fixed constant, not measured. Revisit when a
  runtime width measurement exists.
- Slice: none in this pass.

## D-perf-bgaudit (perf-bgaudit lowering owner): close, no code

- Premise: `lower_current_thread` lives in `flamewm-integrations-linux`. Finder,
  imageviewer and archiver-app do not depend on it (`docs/backlog/perf-bgaudit.md`).
- Measurement: no foreground gain was measured (perf-bgprio A/B, idle and stress-ng
  within noise). Weather child inherits nice 10 from `weather_host`.
- Decision: close as no-op. Do not add a Cargo edge from three app crates to
  `flamewm-integrations-linux` for an unmeasured gain.
- Revisit when: an A/B run shows a foreground gain for one of the four sites.
- Slice: none.

## D-pol-session (pol-session): superseded, gate on (`ecf88310`)

- Earlier decision (superseded): keep the gate off until the prompt UI answers.
  It was made while `a2ba9ce0` held `POLKIT_AGENT_CAN_ANSWER = false`.
- Current (checked in code): `crates/flamewm-shell/src/main.rs:998` has
  `POLKIT_AGENT_CAN_ANSWER = true`, and registration at shell start is under that
  gate (`main.rs:676`). `ecf88310` flipped it because the prompt card now answers:
  typed password reaches the helper and Escape returns `Cancelled`. Proof was on
  private Xvfb and private buses with a fake Authority and fake helper. `66e053f4`
  unregisters on exit.
- Safety net: a prompt with no answer expires as `Failed` after `PROMPT_TIMEOUT`
  = 120 s (`crates/flamewm-integrations-linux/src/polkit_agent.rs:34`, expiry at
  :657; `8f7cbbc4`). This bounds the pkexec wait the old decision guarded against.
  It does not remove the wait.
- Residual risks: (1) the real `polkit-agent-helper-1` path is not exercised;
  (2) typed input and Escape are proven only with the fake helper; (3) real polkitd
  and logind registration are unverified (host system bus not probed); (4) pkexec
  waits up to 120 s when the card does not answer.
- Decision: the gate stays on. Revisit with a real helper run on a private session
  bus (`pol-proof`, D-polkit-proof). If that run shows a hang, set the const to false.
- Slice: none. Proof: fake-helper card proof `ecf88310`; real helper not run.

## D-wrss-scratch (wrss-scratch): closed as not-fixable below bar

- Measured: scratch texture drop -3.0 to -3.5 MB PSS, below the 5 MB bar
  (`docs/backlog/wayland-rss.md:43`, `group-b-audit.md`).
- Conflict: `wayland-rss.md:62` says the 3.38 MB `image_core` buffer is "likely the
  same buffer (not confirmed)"; the row says it is not. Decision: treat the claim as
  unconfirmed and do not use it for the status.
- Decision: status is closed as not-fixable below the bar. Revisit if a larger idle
  target is set.
- Slice: none. Proof: runtime not re-measured in this pass (UNVERIFIED).
