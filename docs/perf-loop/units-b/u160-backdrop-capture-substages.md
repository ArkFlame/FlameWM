# u160 split a backdrop capture into its sub-stages, then cut the largest
Area: compositor, drag. File: crates/flamewm-wm-wayland/src/render/dock_backdrop.rs (`capture_dock`).
Measured (u159 timers, scripted drag, nested software GL, 2 runs): `dock_backdrops` is 70% of compositor frame time, 25 captures in 89 frames, about 75 ms per capture.
Do: add `Stage` entries (or a nested lap) for capture render / GPU readback / CPU blur / crop, re-run the drag twice, and cut the largest share (candidates: blur on a smaller reduced image, a cheaper separable blur, reuse of the readback buffer).
Accept: the measured share drops with no visible change in the blurred strip (read the frame).
Status: open.
