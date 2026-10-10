# Project Sources

Engineering/reference knowledge only. Production code must never load from this directory.

Authority order for current FlameWM Rust WebRender work:

1. newest explicit product instruction;
2. active `/crates`, `/ui`, `/assets` source and executable evidence;
3. V8 porting/taskbar contracts;
4. RustWebRender 0.0.6 implementation/source and approved Xephyr visual reference;
5. SourcePort post-mortem methodology for parity-oriented translation work;
6. current FlameWM C/Rust product reference implementations;
7. V5/V4/Product Soul reports where non-conflicting;
8. IceWM/IceWM-Rust/SourcePort source only as protocol/behavior reference — never as FlameWM runtime base.

Key current additions:

- `RUSTWEBRENDER_0.0.6_IMPLEMENTATION_REPORT.md`
- `RUSTWEBRENDER_PROTOTYPE_PORT_0.0.6.md`
- `RUSTWEBRENDER_SUPPORTED_HTML_CSS_0.0.6.md`
- `reports/ICEWM_RUST_PORT_POST_MORTEM_SOURCEPORT_LAW.md`
- `visual/rwr-0.0.6-target-xephyr.png` (historical; not present in this checkout)
- `visual/user-rwr-0.0.6-xephyr.png`
