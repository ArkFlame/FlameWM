# u170 Wayland lock: clock text is lighter than X11 (open)

Seen (3x crops of the clock, X11 lock above, software `paint_document` below; the lower frame was dumped with no time so it shows `--:--`): X11 draws the clock in a bold weight with a soft dark drop shadow; the software frame draws regular weight, no visible shadow. The same difference was visible earlier in the nested Wayland frame (clock, date, username, labels).

Suspects: `glyph_source` in `crates/flamewm-lock/src/present.rs` ignores the node's font weight, and `PaintCommand::TextShadow` is lowered but its blur radius/offset may be dropped by `snap_commands`/`lower_commands`. Check what weight the document asks for the clock node and what `glyph_source` loads.

Acceptance: 3x crop of the clock with a real time shows bold digits and a soft shadow like the X11 frame; unit test on the weight passed to the glyph source.

## Findings
- Confirmed cause: `PaintCommand::Text` carries `weight: u16`, but `glyph_source` (lock `present.rs`) calls `glyphs::text_mask(text, size, x, y)` which has no weight parameter, and `flamewm-render-core/src/glyphs.rs` loads a single Regular face (`CANDIDATES`: IBMPlexSans-Regular, NotoSans-Regular, DejaVuSans, ...) with a per-size run cache. Every software-painted string is therefore Regular.
- Fix shape: add `weight` to `text_mask`, pick a Bold/SemiBold file per weight (`IBMPlexSans-Bold.ttf`, `NotoSans-Bold.ttf`, `DejaVuSans-Bold.ttf`, ...) with the same search dirs, key the run cache by (weight class, size), fall back to Regular when no bold file exists, pass `weight` from `Prepared`/`glyph_source`. No bold Plex file was found in the system font listing on this machine, so check `fc-list` first; a Noto/DejaVu bold fallback may be what is available. Also check `TextShadow` blur/offset is honoured.
- Layout widths: the X11 path measures with the real weight; make sure the software layout measures with the same face or bold text will overflow its box.
