# Performance scout 2026-11-05: shell, reactor, desktop, ui/render hot loops

Scope: flamewm-shell, flamewm-reactor, flamewm-desktop, flamewm-ui-core, flamewm-render-core.
Excluded: wm-wayland input/state/render, terminal/text_ref work.

## Result

No new per-frame or per-event waste was proven in scope. No code changed. No A/B run was
justified, because no candidate reached a credible >=3% or clear wake/alloc reduction.

## Findings (static, by grep and read)

| Area | Evidence | Verdict |
| --- | --- | --- |
| Desktop widget sync | `crates/flamewm-desktop/src/widgets.rs:131` documents an equal-key gate: an equal key means the document already shows this state | Already gated. `sync_widgets` `format!` churn only runs on real state change. |
| Desktop sample/tick timers | `widgets.rs:129` `SAMPLE_INTERVAL=4s`, `widgets.rs:365-380` wake slot set to `min(due, sample_due)` | Single coalesced wake. No extra polling. |
| Desktop monitor/panel IPC | `output_layout.rs:26,288` `GetDisplays` and `GetPanels` with 2 s timeout | One-shot control calls at layout time, not a poll loop. |
| Shell clock heap trim | `app.rs:25` `CLOCK_HEAP_TRIM_INTERVAL=600s`, gated per the comment at `app.rs:1996` | Already gated. |
| Shell wake coalescing | `app.rs:122`, `app.rs:312-318` shared reactor wake slot, `request_tick` on settings signal | Coalesced. |
| render-core paint/layout allocs | `paint.rs` `to_string`/`Vec::new` hits at lines 373-586 are inside `#[cfg(test)]` (starts line 358/407). `layout.rs` text `to_string` hits at 911 and 1201 are inside the test module (starts line 899). `layout.rs:135,183,223` `Vec::new()` do not allocate until push. | Test-only or non-allocating. No production hot-loop win. |
| Shell projection `format!` (45+ hits in `projection.rs`) | Not checked per-frame yet | Open. Needs a call-graph read (which of these run per tick vs per event). |

## Not run

- `scripts/perf/fm.sh idle|marquee|hover` before/after: not run. It needs a nested Xvfb session, and the host load is about 25.
- Same-process A/B kernels: not run. No candidate existed.
- Open item: call-graph audit of `flamewm-shell/src/projection.rs` and `async_projection.rs` `format!` sites to confirm they stay off the idle path.

## Next step

Run `scripts/perf/fm.sh all` on an idle host to confirm idle wakes/s and CPU baseline. Then audit the shell projection `format!` sites against that baseline.
