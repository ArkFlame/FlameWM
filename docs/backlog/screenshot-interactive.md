# wl-portal: interactive Screenshot slice

## Done

- `crates/flamewm-screenshot/src/region.rs`: pure region rules (clamp to output, confirm with min side 3, keyboard nudge). The X11 overlay (`ui.rs`) confirms drags through it. Unit tests only.
- Crop of the RGBA frame to the rect already exists (`Image::crop`, `capture.rs`) and is unit-tested.
- Portal `interactive=true` is still refused with response 2 (`portal.rs`), unchanged.

## Remaining

- Portal surface: no overlay is wired to `org.freedesktop.portal.Screenshot` with `interactive=true`. Precedent: KDE Spectacle region mode and COSMIC screenshot both show a region overlay before returning a URI; FlameWM would reuse `ui::select_region` on the portal path.
- Keyboard nudge is not yet bound in `ui.rs`; `region::nudge` has tests only.
- Runtime behaviour of the overlay under a private Xvfb is unverified in this slice.
