# FlameWM 0.1.0 — Pass 23: installed-system stability / remaining production gaps

## Scope and evidence

Source baseline: Pass 22. The reporter installed `flamewm_0.1.0_amd64.deb` successfully in an X11/SDDM session and observed invisible titlebar controls, second-panel weather drift, physical monitor reassignment, launcher filename labels, lost browser wallet access, missing MIME choices, nonworking desktop ghost, and missing shortcuts. We have **not** received logs from the crashed/flipped session or a package from the real system, nor can this trimmed archive compile native Rust or test physical RandR/X11. Source-level repairs below are not release certification.

## Implemented repairs

1. **Installed WM chrome asset root**: titlebar control glyph loader now uses the canonical runtime data root shared with the backdrop renderer (`/usr/share/flamewm` when installed, workspace source when developing). The WM's indexed icon resolver uses the same root. If an installed SVG is missing or invalid, bounded raster-only line icons are drawn instead of silently dropping minimize/maximize/restore/close. Control role, shape, hit geometry and material styling remain skin-owned.
2. **Weather in every panel**: the weather tick projects the same current feed to every native panel document and requests redraw for each. Failure of any panel preserves the projection retry latch. No second polling provider is introduced.
3. **RandR primary physical output**: both real display adapters query `GetOutputPrimary` rather than marking every real output `primary=false`. An unsupported/no-primary server uses a stable connector-sorted active output fallback. This mitigates monitor identity flipping from resource enumeration changes; it does *not* prove the reported prolonged-use topology failure has been eliminated.
4. **Desktop `.desktop` labels**: derive displayed name from the desktop-file `Name` field, with optional locale variants (`LC_MESSAGES`/`LANG`), even when OnlyShowIn/TryExec means the launcher is not currently executable. Fallback remains the filename. The existing application's launcher parser still validates execution separately. Drag label inherits the same display name.
5. **Shortcut registry and WM dispatch**: global bindings `Super+E`, `Ctrl+Alt+E` open home via `xdg-open` (configured directory handler), and `Super+T`, `Ctrl+Alt+T` start `konsole`, falling back to `x-terminal-emulator`. Added as four independent canonical shortcut actions so X passive-grab state, conflict detection and persistence use the existing authority. Spawn errors are logged.
6. **Read-only installed-production diagnostics**: `python3 -I tools/flamewm_production_doctor.py` prints session environment names, connected monitor geometry/primary, presence of packaged control SVGs, MIME defaults, KDE Wallet bus *service name* visibility, and existence of file-explorer / terminal commands. Never prints secrets or wallet contents.

## Still outstanding — do not claim completion

- **Native draggable monitor arrangement:** the authoritative DisplayPort supports mode/scale but not CRTC repositioning, mirrored groups, atomic rearrangement/rollback, or per-output persisted layouts. A draggable settings UI alone would fake success. Implement as one backend transaction covering active RandR CRTCs, coordinate normalization, root framebuffer limits, scale, primary selection, persistence, safety timeout/rollback, and immediate repaint and strut reconciliation, then wire the semantic monitor cards. Retire any geometry bypass.
- **Desktop drag preview:** Pass 20 fixed retained layout invalidation, but the reporter says the ghost is still invisible. This pass does not assert resolution. Reproduce in native dual-monitor X11 with a pointer-move trace of source callback, retained node visibility, actual paint/damage and X11 composite frame; fix the first failing owner before changing more render code. Do not keep layering speculative ghost tweaks.
- **KDE Wallet / browser passwords:** credentials likely belong to the user's original KDE Wallet or Chromium encrypted password backend. Do not migrate, reencrypt, unlock automatically, rewrite browser profiles, or change browser `--password-store` globally without a verified provider mapping and consent. Confirm wallet service activation, the session's KDE integration context, and browser backend choice, then integrate PAM/DBus lifecycle without copying secrets. The diagnostic intentionally shows metadata only.
- **File associations and Open With:** desktop and Dolphin use XDG MIME defaults; inspect current `xdg-mime query default text/plain`, `inode/directory`, `application/pdf`, and actual `mimeapps.list` precedence in the FlameWM session before changing handlers. The current `xdg-open` dispatch is preserved; an OS-level default-app chooser and KDE MIME integration require a real provider implementation, not a hard-coded Kate assignment.
- **Intermittent right-click/workspace outage:** the RandR primary discrepancy was source-confirmed and mitigated. Actual event loss, session subprocess restarts, and X11 visibility/focus have not been proven. Reproduce with contemporaneous `wm.log`, `shell.log`, `desktop.log`, `session.log`, `xrandr --verbose`, `xprop -root _NET_CURRENT_DESKTOP`, and timeline of RANDR changes.

## Native acceptance matrix (required on user's installed machine)

- Build: `cargo check --workspace --all-targets`; `CARGO_BUILD_JOBS=2 cargo build --release --workspace`; `./scripts/build-deb`. Treat failures as blocking and never reuse prior `.deb`.
- Packaging: `dpkg-deb -f .target/dist/flamewm_0.1.0_amd64.deb Architecture` must print `amd64`, `(cd .target/dist && sha256sum -c SHA256SUMS)`, and `python3 -I tools/flamewm_deb_acceptance.py --deb .target/dist/flamewm_0.1.0_amd64.deb`.
- Two actual physical outputs: identity stable over 30 minutes, repeated monitor plug/unplug, right-click, workspace switch, Dolphin file launching, and window resize. Check both taskbars' weather and all window-control glyphs from installed `/usr/share/flamewm/assets/web/breeze` without source checkout mounted.
- Desktop: `.desktop` `Name` locale vs filename; press and move the icon with ghost below the pointer, 50% opacity, pointer-track at 60Hz without afterimages or disappearing source icon; drop/cancel correct.
- Hotkeys: both forms of file/terminal launch work with NumLock/CapsLock and do not trigger twice or steal application text input.
- KWallet: test a *non-sensitive* browser profile with existing wallet credentials visible in both Plasma and FlameWM, without writing credentials or starting duplicate wallet daemons.

## Production release gate

Status: **NOT_RELEASE_CERTIFIED**. Source tests and archive integrity are necessary, not sufficient. Do not label the full set of user requests complete until monitor arrangement, drag ghost, wallet/MIME, and the intermittent session outage have native functional verification.
