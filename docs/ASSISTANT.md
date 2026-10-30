# Assistant: providers

The assistant window chats with two OpenAI-compatible providers. Both use one client,
`crates/flamewm-assistant-core/src/openai.rs`, which runs HTTP through `curl` and reads SSE.

## Providers

| Provider | Endpoint | Key | Detection |
|---|---|---|---|
| ARK AI (default) | `https://ai.arkflame.com/v1` | `assistant.apiKey` in Settings, or the public default key | Always listed. Models from `GET /models`, fallback `lite`, `flash`, `pro` |
| Hermes | `http://127.0.0.1:8645/v1` | Dummy bearer, the proxy ignores it | Probed when the window opens and when the model menu opens |

- The Hermes probe is `GET /health`, then `GET /v1/models`, each limited to 2 s. It counts as a
  model proxy only when `status` is `ok`, `platform` is absent, `upstream` is set, and
  `authenticated` is not `false`. HTML, a non-JSON model list or an empty list means not running.
- The Hermes group appears in the model menu only while the probe succeeds.
- Test hooks: `FLAMEWM_ASSISTANT_ARK_BASE` and `FLAMEWM_ASSISTANT_PROXY_URL`. Each accepts only
  `http://127.0.0.1` or `http://localhost`. Other hosts are ignored.
- Only `https`, or `http` on 127.0.0.1 or localhost, is accepted, so a key never travels in cleartext.
- The bearer goes to curl on stdin (`-K -`) and never appears in argv or logs.

## Model menu and selection

- One menu lists ARK AI models, then Hermes models under its own header. The check mark marks the
  current provider and model. Search matches ids in every group and hides groups with no match.
- Paging runs over the flattened rows. Header rows count toward the page size.
- The choice is saved in `state.json` as `{"provider":"ark"|"hermes","model":...}`. A file without
  `provider` is a legacy ARK state and maps to ARK `flash`.

## Turns and fallback

- Each turn goes to the selected provider. A Hermes turn probes the proxy first. When the proxy is
  gone, the turn uses ARK `flash` and the notice says so.
- Footer: "Powered by ARK AI" with ARK selected, "Via Hermes proxy · model only" with Hermes.
- Errors are short and user-facing. Raw HTML or JSON bodies are never shown.

## Verification

- Unit tests use fake ARK-like and proxy-like servers (`test_support::serve`). They cover the
  shared SSE path, bearer headers, probe rules, grouped menu rows and fallback.
- Live: one streamed `Hello` to `https://ai.arkflame.com/v1/chat/completions` with model `flash`
  returned HTTP 200, `text/event-stream`, 12 `data:` frames.
- Not verified here: the real Hermes proxy was not used by tests or screenshots. Screenshots on a
  scratch display are not yet taken.

## Taskbar button and popover

- `assistant.taskbarIcon` (default on) shows a white brain button in the taskbar, next to the
  tray items. It shows only while `assistant.enabled` is also on. Settings > Assistant >
  "Show assistant in taskbar" flips it.
- The button (`tray-assistant`, `ui/shell/panel.html`, icon `assets/web/breeze/assistant-brain.svg`)
  sends `assistant.open`. The shell resolves the button rect and runs
  `flamewm-assistant --popover X Y WIDTH HEIGHT` with device pixels.
- Choice: the popover reuses the assistant binary, not the shell's in-process popup machinery. The
  shell's popups are rendered from build-time artifacts; hosting the chat there would duplicate the
  assistant UI. The popover is an override-redirect window (`UiWindowRole::PopupMenu`, `_NET_WM_WINDOW_TYPE_POPUP_MENU`),
  so the WM draws no frame or titlebar.
- Placement: centred on the button, above it for a button in the lower half of the root and below
  it otherwise, kept on the root (`popover::origin`).
- Dismissal: a root press outside the popover and the button, or Escape, exits it. A press on the
  button is left to the shell: its release launches `flamewm-assistant --popover` again, and
  `instance::claim` stops the open window, the same toggle as Super+C.
- Super+C and Settings > Open assistant still open the normal window.
- Limits: the popover does not take keyboard focus by itself, so Escape works only when the pointer
  is over it or the X focus is on it. Presses are compared in logical pixels, which match device
  pixels at the default scale only.
