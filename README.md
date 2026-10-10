# FlameWM Rust WebRender 0.0.4

FlameWM Rust WebRender is the clean-room Rust/X11 branch of FlameWM.

`0.0.4` continues the clean-room product line established in 0.0.2. There is no active IceWM engine, no `icewm-rust` dependency and no `/engine` directory. IceWM, FlameWM C, earlier FlameWM Rust, SourcePort and older renderer releases live only under `.vendor/` as source/reference corpora.

The active product is:

```text
FlameWM-owned Rust X11 WM (`crates/flamewm-wm`)
        +
FlameWM Platform/product crates
        +
FlameWM Render compile-time HTML/CSS renderer
        +
V8 FlameWM presentation and native Rust interactions
```

## Architectural rule

HTML/CSS is an **authoring/build language**, not a browser runtime.

```text
ui/index.html + ui/flamewm.css + raster assets
                  |
                  | build time only
                  v
      flamewm-render-compiler
                  |
                  v
               RWRB/2
                  |
                  v
           RuntimeDocument
       layout / paint / hit-test
                  |
                  v
      native Rust interaction state
                  |
          +-------+-------+
          |               |
   flamewm-render-x11  flamewm-wm
   Xlib/Xft renderer      x11rb WM
```

There is no Chromium, Electron, WebKitGTK, JavaScript VM, Tauri, WGPU or browser networking stack in the shell.

## 0.0.4 product identity

- Removed `/engine` and every active IceWM/IceWM-Rust dependency.
- Added an independent FlameWM X11 WM in `crates/flamewm-wm`.
- Corrected copied Cargo metadata bug that produced `rust-version = "0.0.1"`; FlameWM product workspace defaults to Rust 1.85 / edition 2024; promoted renderer crates stay on proven edition 2021 surface.
- Corrected `doctor` fontconfig detection using the dynamic loader instead of a fragile `ldconfig | grep -q` pipeline.
- Promoted renderer **0.0.6** source as active FlameWM Render implementation, including:
  - release-optimized interactive path;
  - ordered `MotionNotify` coalescing;
  - `XSetGraphicsExposures(False)` for image-copy GCs;
  - cached image transparency masks;
  - rounded border strokes;
  - press/motion/release action phases;
  - retained Xft/font/image caches.
- Promoted the 0.0.6 V8 showcase presentation and raster mappings as the active visual baseline. This is the branch that produced the near-reference Xephyr screenshot supplied for this iteration.
- Added deterministic visual comparison through `./verify-visual`.
- Added the porting post-mortem to project sources and made its core lesson an engineering rule: preserve exact owner/source identity for parity surfaces before refactoring.
- Kept all historical implementations in `.vendor/` only; production code is under `/crates`.

## Run

Ubuntu/Debian development dependencies:

```bash
./scripts/install-deps-ubuntu.sh
```

Then:

```bash
./scripts/doctor
./scripts/check
./scripts/xephyr
```

Visual differential:

```bash
./scripts/verify-visual
```

The interactive contract is a **release build**. Debug renderer latency is not accepted as product performance evidence.

## Source status

`0.0.4` is a real clean-room branch foundation plus a substantially ported V8 shell. It does **not** claim final FlameWM parity. The independent WM has the first functional X11/ICCCM/EWMH/window/workspace/snap foundation, while several complete-product surfaces remain in `docs/TODO.md`.

The packaging environment used to assemble this archive does not provide Rust/Cargo/Xephyr, so compiler/runtime PASS is not fabricated here. `./check`, `./xephyr` and `./verify-visual` are mandatory target-machine gates.

## Performance target

- <= 40 MiB combined owned PSS target, measured rather than assumed;
- effectively idle when no user/system event occurs;
- no polling frame loop;
- no stale pointer-motion replay during direct manipulation;
- event-driven audio/network/media integration;
- no duplicate window/workspace authority.

See `docs/PERFORMANCE.md` and `docs/PORTING_METHOD.md`.
