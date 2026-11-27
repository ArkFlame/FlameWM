# u170 Wayland lock: clock text is lighter than X11 (open)

Seen (3x crops of the clock, X11 lock above, software `paint_document` below; the lower frame was dumped with no time so it shows `--:--`): X11 draws the clock in a bold weight with a soft dark drop shadow; the software frame draws regular weight, no visible shadow. The same difference was visible earlier in the nested Wayland frame (clock, date, username, labels).

Suspects: `glyph_source` in `crates/flamewm-lock/src/present.rs` ignores the node's font weight, and `PaintCommand::TextShadow` is lowered but its blur radius/offset may be dropped by `snap_commands`/`lower_commands`. Check what weight the document asks for the clock node and what `glyph_source` loads.

Acceptance: 3x crop of the clock with a real time shows bold digits and a soft shadow like the X11 frame; unit test on the weight passed to the glyph source.
