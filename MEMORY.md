# Memory

Current facts for this Rust + axum service. Accepted choices and why they were made are in `DECISIONS.md`. Read both before changing architecture. When a durable choice changes, update `DECISIONS.md` in the same change. Git history is the changelog.

## Stack

- Rust `1.98.1`, pinned in `rust-toolchain.toml` and in the `Dockerfile` build image `rust:1.98.1-bookworm`. The runtime image is `debian:bookworm-slim` and does not ship the toolchain.
- HTTP: axum `0.8` on tokio. The process uses the `current_thread` runtime. `GET /` reports `language: "Rust"` and `framework: "axum"`.
- SQL: `tokio-postgres` `0.7` (`NoTls`, 3-second connect timeout when the URL has none, `prepare_cached`) and `deadpool-postgres` `0.14` (`RecyclingMethod::Fast`, `max_size(4)`).
- Outbound HTTP: `reqwest` `0.12` with `default-features = false` and features `json` and `rustls-tls`.
- Listen address: IPv6 `::`. `PORT` defaults to `4005` in the binary. The image and Fly set `8080`.
- Release profile: thin LTO, `codegen-units = 1`, `panic = "abort"`, `strip = true`. Fly sets `TOKIO_WORKER_THREADS=1`.
- API version `0.2.0`. `schema_version` 1. `created_year` 2026.

## Commands

```bash
make test        # cargo test --locked
make clippy      # cargo clippy --locked --all-targets --all-features -- -D warnings
make audit       # cargo audit
make secrets     # gitleaks detect --source .
make fmt         # cargo fmt --all -- --check
make check       # rustfmt, clippy, cargo test, cargo audit, gitleaks
make hooks       # pre-commit install and git config core.hooksPath .githooks
```

Emergency skip: `SKIP=fmt,clippy,local-tests,audit,gitleaks git commit`.

`cargo test` needs a reachable Postgres 16 server from `DATABASE_URL`. CI runs the five checks as separate jobs after one prep job. The test job provides Postgres 16.

## Contract

- Query only these views: `v1_years`, `v1_speakers`, `v1_talks`, `v1_sponsors`, `v1_year_sponsors`, `v1_sponsorships`.
- Do not query Ash tables. Do not speak Ash JSON:API. Do not write.
- The HTTP contract is the CMS `priv/api/openapi.yaml` and `priv/api/AGENTS.md`.
- Lists use `{ "data": [ ... ] }`. Missing records use 404 `{ "error": "not_found" }`.
- `GET /health` returns `{ "ok": true }`, runs no SQL, and does not check out a connection.
- Every response sets `X-Polyglot-Language` and `X-Polyglot-Framework`.
- Register once with `POST {CAROLINA_URL}/internal/api-endpoints/register`. No heartbeat. An empty `CAROLINA_URL` or token returns without logging. If the POST fails because the CMS is down, log and keep serving.
- `photo_path` and `logo_path` are returned as stored. This process does not serve image bytes.

## Environment

| Variable | Local example |
| --- | --- |
| `DATABASE_URL` | `postgres://postgres:postgres@127.0.0.1:5432/carolina_dev` |
| `CAROLINA_URL` | `http://127.0.0.1:4000` |
| `POLYGLOT_REGISTER_TOKEN` | `dev` |
| `PUBLIC_BASE_URL` | `http://127.0.0.1:4005` |
| `PORT` | `4005` |

## Constraints

- This repo is its own git remote. Do not fold it into the CMS remote. Do not assume `../elixir` or any other sibling checkout exists.
- Route tests create a private database on the Postgres 16 server in `DATABASE_URL`, seed `v1_*` views there, and drop that database. Do not point tests at a shared catalog they will mutate.
- Bind the listener before Postgres warmup and CMS registration. Both run in spawned tasks. Do not block accept on either one.
- Do not add a register heartbeat.
- Do not bind IPv4-only. Listen on `::`.
- Keep `tokio-postgres` on `NoTls` unless a new decision says otherwise. Do not open catalog SQL from the register task.
- Do not drop `--locked` from release builds in the `Dockerfile`.
- Idle Fly machines suspend at 256mb. Do not set `auto_stop_machines` to `stop` while `min_machines_running` is 0.
- Do not commit secrets, production credentials, tailnet hostnames, or non-public tokens.
