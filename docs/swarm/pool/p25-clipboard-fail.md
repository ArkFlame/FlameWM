# Unit p25-clipboard-fail
STATUS: OPEN

Crate flamewm-clipboard: audit startup and runtime failure paths (no display, no selection owner, oversize payload, dead peer). Each must degrade (feature off, one report line) never abort or spin; add one fault test where cheap.
