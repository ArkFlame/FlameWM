# u21-shell-widgets

Second pass over notification_center, osd, quick_controls and launch_feedback. The earlier units
(u06, u07) already removed the per-row history scan, the snapshot copies in the advice tick and the
per-frame `format!` ids of the dots and the dock. The largest slice left in this area was not the
projection itself but the work every shell wake repeated for these widgets.

- notification host: the per-turn `active()` call cloned every active notification view, including
  each toast's icon RGBA buffer, on every reactor turn. The provider already queues one typed UI
  update per view change, so the turn now reprojects only when that queue drained non-empty and
  reuses the one fetched active set for both the toast and the centre. A passive turn clones nothing.
- notification centre: while the centre was open, every turn rebuilt `provider.active()` just to
  compare ids. The projection now receives the active set from the caller (fetched only on a queued
  update) and otherwise compares against the ids of the last projection, so a history-only change
  (mute, dismiss) keeps its projection without cloning toast views.
- notification centre: text nodes were written with `text` and a cloned `String`, which allocates
  and marks damage even when the value is unchanged. They use the borrowed `text_ref` write, so a
  Tab focus move now changes only the focus nodes and repaints nothing else.
- osd: `tick_osd` rebuilt a full audio row (owned id and label strings) on every shell turn for two
  fields. A new `default_sink_level` reads `(volume_percent, muted)` straight from the default sink
  endpoint, allocation-free; `default_endpoint_row` keeps sharing the same lookup.
- quick_controls: `apply_system_snapshot` deep-compared the whole system snapshot (audio endpoints,
  access points, media players) before accepting it. A changed provider revision now takes the
  content as changed and skips the compare; only an unbumped push pays for the equality check.
- quick_controls: `coalesce_commands` rebuilt the queue on every helper turn. With zero or one
  command nothing can collapse, so the queue keeps its buffer instead of draining into a new one.
- launch_feedback: the pending-dot `XShape` mask was re-sampled on every new launch. It is cached by
  badge size, so a relaunch at the same scale reuses the spans.

Known next candidate in this area (not done here): `quick_controls/host/projection.rs` builds a
throwaway `ShellSnapshot` for each popup reprojection, which deep-clones `SystemSnapshot`. The popup
projectors only read `system`, so the clean fix is to take `&SystemSnapshot` in
`project_audio_full`, `project_network_full` and `project_network_details` and pass
`&snapshot.system` from the host.
