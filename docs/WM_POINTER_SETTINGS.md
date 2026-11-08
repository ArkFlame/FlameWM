# Window pointer settings (X11 window manager)

Read by `flamewm-wm-x11` once at start from the environment (`WmConfig::from_env`). Unset or invalid
values use the default, so a typo never disables pointer handling. Restart the session to change them.

| Variable | Values | Default | Effect |
| --- | --- | --- | --- |
| `FLAMEWM_TITLE_DOUBLE_CLICK` | `maximize`, `off` (`none` is accepted as an alias) | `maximize` | Second titlebar click inside 500 ms and 4 px. `maximize` toggles maximize (historical behaviour). `off` does nothing. Single click still focuses and drags. |

Implementation: `crates/flamewm-wm-x11/src/frame/title_action.rs` (parsing, default) and the reducer
branch in `crates/flamewm-wm-x11/src/frame/controller/reducer.rs` (`title_click_release`).

## Not yet supported

- `shade` as a titlebar action: the WM has no shade implementation (`_NET_WM_STATE_SHADED` is advertised
  only). Adding it needs a new placement mode, so it is deferred.
- A settings-UI key for these variables: the typed stable-settings table lives in
  `crates/flamewm-platform/src/settings.rs` (over 50 KB, owned by the settings worker). A key there
  would be a separate change.
