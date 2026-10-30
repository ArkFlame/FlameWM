# Assistant: Hermes backends

Evidence from read-only source inspection and probes on 2026-10-30. Hermes v0.22.5.

## Two backends behind one URL

| Mode | Source | `GET /health` | Turns | Tools/skills |
|---|---|---|---|---|
| Agent API server | `~/.hermes/hermes-agent/gateway/platforms/api_server.py` | `{"status":"ok","platform":"hermes-agent",...}` | `POST /v1/runs` (events via `/v1/runs/{id}/events`) | Yes |
| Nous proxy | `~/.hermes/hermes-agent/hermes_cli/proxy/server.py` | `{"status":"ok","upstream":"Nous Portal","authenticated":true}` (no `platform`) | `POST /v1/chat/completions`, streamed | No (model only) |

- Agent mode is enabled with `API_SERVER_ENABLED=true`, `API_SERVER_KEY` (16+ chars), `API_SERVER_PORT` (default 8642) in `~/.hermes/.env`.
- The proxy allows only `/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/models`. Other paths return JSON `404 path_not_allowed`.
- The proxy ignores the client bearer and attaches real credentials. Default port 8645, started by `hermes proxy start`.

## Probes against the user's real proxy (port 8645)

- `GET /health`: 200, `authenticated: true`.
- `GET /v1/models` with `Bearer x`: 200, 382 ids, including `:free` ids such as `poolside/laguna-s-2.1:free`.
- `GET /v1/runs`, `GET /v1/capabilities`: 404 `path_not_allowed`.
- `POST /v1/chat/completions` (`max_tokens` 4), non-stream and `stream:true`: 200. The stream ends with `data: [DONE]`.

## Model semantics

- `/v1/models` `data[].id` lists selectable models. The chosen id is sent as `model` in the chat or run payload.
- Hermes' own default is `model.default` in `~/.hermes/config.yaml`. The app never writes it.

## Not verified

- The real agent API server (8642) was not run or probed.
- No `/v1/runs` turn was run against the real Hermes.
- The `hermes proxy` start and stop lifecycle was not exercised.

## Connect flow (agent first)

- Discovery probes 8642 then 8645 and prefers the agent API server over the proxy.
- Hermes installed and API server off or keyless: one confirm, "Enable the Hermes API server?". It backs up `~/.hermes/.env`, sets `API_SERVER_ENABLED=true` and a generated key, and restarts a running gateway in the same step.
- Installed means the binary file exists. A failed `--version` probe never means "not installed". Candidates: `~/.local/bin/hermes`, `~/.hermes/hermes-agent/.hermes/bin/hermes`, `hermes` on PATH.
- Proxy mode is labelled "Model only, no tools".

## Agent run events (`POST /v1/runs`, `GET /v1/runs/{id}/events`)

Source: `gateway/platforms/api_server_runs.py`. Event names: `run.started`, `message.delta` (`delta`), `tool.started` (`tool`, `preview`), `tool.completed` (`tool`, `error`), `approval.request` (`choices`, `request_id`), `run.completed`, `run.failed` (`error`), `run.cancelled`, `run.interrupted`. Approval resolves with `POST /v1/runs/{id}/approval` and `choice` once, session, always or deny. The window shows "Approve once" and "Deny" for each approval. The worker never auto-denies. A 120 s timeout in the window sends deny. Tested with a concurrent fake server (`approval_round_trip_answers_the_waiting_run`). Not tested against a real gated tool call.

## Live evidence (scratch home, real `hermes gateway run`)

- `HERMES_HOME` set to a scratch dir, `API_SERVER_PORT=18642`, no provider credentials.
- `GET /health`: 200 `{"status":"ok","platform":"hermes-agent","version":"0.22.5"}`.
- `POST /v1/runs` (fresh `session_id`, `"Say ok."`): 202 `started`. Events: `run.failed` with "not connected to any AI provider". The run is not a success.
- `cargo test -p flamewm-assistant-core live_agent_run -- --ignored`, with the scratch URL and key: exit 0, the error reaches the client.
- Xephyr (scratch display, scratch HOME, scratch gateway): the window showed "Connected" and the prompt returned the `run.failed` reason once in the chat, with "Press Enter to retry." in the notice.
- Not verified: a successful agent turn (needs a provider), real tool events, a real approval, the enable-confirm screen on a live machine (the probe finds the user's proxy on 8645, so it cannot show here), and the real `~/.hermes` gateway.
