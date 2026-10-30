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
