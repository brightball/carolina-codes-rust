# carolina-codes-rust

Read-only v1 polyglot API in Rust (axum). The Phoenix site (`Carolina.Polyglot`) keeps at most one language API warm and reads speakers and sponsors from it. This repository is that finished service, and it is its own git remote. Do not assume a sibling checkout of the CMS or of any other language exists. Do not fold this tree into the CMS remote (`github.com/brightball/carolina-codes`).

The HTTP contract is the CMS `priv/api/openapi.yaml` and `priv/api/AGENTS.md`. Do not implement Ash JSON:API (`application/vnd.api+json`). Responses are ordinary JSON.

Before changing architecture, read `MEMORY.md` (stack, commands, and constraints) and `DECISIONS.md` (accepted choices). When a durable choice changes, add or supersede a record in `DECISIONS.md` in the same change. Git history is the changelog.

## Purpose

1. Query PostgreSQL **v1 views** only. Never query Ash resource tables.
2. Expose the v1 routes below.
3. **Register once on boot** with the Elixir site (no heartbeat). If `CAROLINA_URL` or the token is empty, skip registration and keep serving. If the POST fails because the CMS is down, log and keep serving.

## Environment

| Variable | Example | Role |
| --- | --- | --- |
| `DATABASE_URL` | `postgres://postgres:postgres@127.0.0.1:5432/carolina_dev` | `v1_*` views in the CMS database |
| `CAROLINA_URL` | `http://127.0.0.1:4000` | Elixir site (optional; register no-ops if down) |
| `POLYGLOT_REGISTER_TOKEN` | `dev` | Bearer token for register |
| `PUBLIC_BASE_URL` | `http://127.0.0.1:4005` | URL the Elixir site will call |
| `PORT` | `4005` | Listen port (the container and Fly set `8080`) |

The binary defaults `DATABASE_URL` to the example above and `PORT` to `4005` when those variables are unset.

## SQL views (query these)

`v1_years`, `v1_speakers`, `v1_talks`, `v1_sponsors`, `v1_year_sponsors`, `v1_sponsorships`.

The views live in the CMS database. This repository does not ship a catalog schema. Year-scoped speaker rows include `languages` and `topics`. Year-scoped sponsor rows include `tier` and `blurb`. Return `photo_path` and `logo_path` as stored; this process does not serve image bytes.

## Required HTTP routes

Wrap list payloads as `{ "data": [ ... ] }`. Unknown slugs and unknown paths return 404 `{ "error": "not_found" }`. Every response includes `X-Polyglot-Language: Rust` and `X-Polyglot-Framework: axum`.

- `GET /health` — liveness `{ "ok": true }`. No SQL. The listener accepts this before Postgres warmup and before CMS registration.
- `GET /` — identity (`language`, `language_version`, `api_version`, `framework`, `created_year`, `schema_version`, `endpoints`)
- `GET /v1/years`
- `GET /v1/speakers` and `GET /v1/speakers?year=2026`
- `GET /v1/speakers/{slug}` and `GET /v1/speakers/{year}/{slug}`
- `GET /v1/sponsors` and `GET /v1/sponsors?year=2026`
- `GET /v1/sponsors/{slug}` and `GET /v1/sponsors/{year}/{slug}`

## Register on boot (once)

`POST {CAROLINA_URL}/internal/api-endpoints/register`

```
Authorization: Bearer {POLYGLOT_REGISTER_TOKEN}
Content-Type: application/json
```

Body fields: `language`, `language_version`, `api_version`, `framework`, `created_year`, `base_url` (`PUBLIC_BASE_URL`, or `http://127.0.0.1:{PORT}` when that is unset), `schema_version` (1), `endpoints` (the same method, path, and query list that `GET /` returns).

Do not heartbeat. An empty `CAROLINA_URL` or token skips registration and does not log. If the HTTP client cannot be built or the POST fails, log and keep serving. Registration is spawned after the listener binds, so it does not block accept.

## This repository

Toolchain pin: Rust `1.98.1` in `rust-toolchain.toml` and `rust:1.98.1-bookworm` in the `Dockerfile`. HTTP framework: axum.

| Path | Role |
| --- | --- |
| `rust-toolchain.toml` | Rust toolchain pin |
| `Dockerfile` | Release build, then a runtime image without the toolchain |
| `src/main.rs` | axum service |
| `Cargo.toml` | Dependencies, including axum |
| `Makefile` | `make check` and `make hooks` |
| `fly.toml` | Fly service |
| `MEMORY.md` | Stack, commands, constraints |
| `DECISIONS.md` | Accepted architecture records |
| `tests/boot_health.rs` | `/health` answers before Postgres accepts a session |

Route tests create a private database of `v1_*` views on the Postgres 16 server in `DATABASE_URL` and drop it afterward. They do not modify other databases. Start Postgres 16 before `cargo test`. `cargo test` fails if that server is unreachable.

`make check` (and `pre-commit run --all-files`) runs rustfmt, clippy `-D warnings`, `cargo test`, `cargo audit`, and gitleaks. Install hooks with `make hooks`. Emergency skip: `SKIP=fmt,clippy,local-tests,audit,gitleaks git commit`.

## Checklist

- All v1 paths return 200 with JSON shaped like the CMS contract (404 on an unknown slug)
- `?year=` speaker rows include `languages` and `topics`; year sponsor rows include `tier`
- Register runs once at process start and does not stop the server when the CMS is down
- No writes; no Ash table names; no Ash JSON:API
- `GET /health` does not touch the database and is served before Postgres warmup
