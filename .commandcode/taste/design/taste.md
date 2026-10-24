# Design

- Window shadows should be cast only to the right and below (light comes from the upper-left) — no left-side or upward shadow. Confidence: 0.65
- Prefers simple, high-performance visuals over elaborate per-pixel/per-corner geometry: when a fancier effect (e.g. shadows hugging rounded corners) turns out buggy, drop it for a plain 45°-offset "Windows 7 / cast hard square" style shadow instead. Confidence: 0.7
- Edges of visual elements should taper/angle smoothly (e.g. 45° chamfers) rather than ending in a hard blunt cut-off. Confidence: 0.65
