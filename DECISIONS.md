# Decisions

Accepted choices for this Rust + axum API. Each record is status, context, decision, and consequences. Git history is the changelog; do not restate it here. When a durable choice changes, add a record or mark the old one superseded in the same commit. `AGENTS.md` requires reading this file and `MEMORY.md` before an architecture change.

## Axum

- **Status:** Accepted
- **Context:** The polyglot contract is a small read-only JSON API. It needs path and query extractors plus one middleware layer for the polyglot response headers.
- **Decision:** Use axum `0.8` on tokio. `GET /` reports `framework` as `axum`.
- **Consequences:** Routes are declared in `router` in `src/main.rs`. Headers are applied with `middleware::map_response`. Replacing the HTTP stack needs a new record.

## tokio-postgres and deadpool-postgres against v1 views

- **Status:** Accepted
- **Context:** The CMS publishes read-only `v1_*` views. The public contract is those views and ordinary JSON. Ash resource tables and Ash JSON:API are not the API.
- **Decision:** Use `tokio-postgres` `0.7` with `NoTls` and `deadpool-postgres` `0.14` (`RecyclingMethod::Fast`, pool `max_size(4)`, `prepare_cached`). Catalog SQL queries `v1_*` views only: `v1_years`, `v1_speakers`, `v1_talks`, `v1_sponsors`, `v1_year_sponsors`, and `v1_sponsorships`. Do not query Ash tables.
- **Consequences:** In-process TLS to Postgres is not configured. When `DATABASE_URL` has no connect timeout, the client uses 3 seconds. Statements stay on the pool's statement cache. A new database stack, or any query of an Ash table, needs a new record.

## Register once, no heartbeat

- **Status:** Accepted
- **Context:** `Carolina.Polyglot` keeps at most one language API warm and keep-alives that process. This process must still serve HTTP when the CMS is down.
- **Decision:** Spawn one `POST {CAROLINA_URL}/internal/api-endpoints/register` on boot, with bearer `POLYGLOT_REGISTER_TOKEN`. If `CAROLINA_URL` or the token is empty, return without logging and keep serving. If the client cannot be built or the POST fails, log and keep serving. There is no heartbeat.
- **Consequences:** Registration does not run catalog SQL and does not block accept. Do not add a periodic register loop.

## /health before Postgres

- **Status:** Accepted
- **Context:** The platform probes `GET /health`. A Postgres connect can sit for the full connect timeout when the database is slow or unreachable.
- **Decision:** Bind the listener first, then spawn pool warmup and registration, then call `axum::serve`. `GET /health` returns `{ "ok": true }` and runs no SQL.
- **Consequences:** Liveness does not wait on Postgres or the CMS. Catalog routes still fail until the pool can connect.

## IPv6 listen

- **Status:** Accepted
- **Context:** The service is deployed on a network that reaches the process over IPv6. Local clients use IPv4-mapped loopback on the same socket.
- **Decision:** Listen on `::` through `listen_addr`. Do not bind IPv4-only. For a Fly Postgres hostname, resolve an IPv6 address and set it on the client config before connect.
- **Consequences:** `PORT` selects the port. The binary default is `4005`. The image and Fly set `8080`.

## Release profile and TOKIO_WORKER_THREADS

- **Status:** Accepted
- **Context:** The deployed VM is one shared CPU and 256mb. Binary size and a predictable runtime matter more than multi-core throughput.
- **Decision:** The release profile uses thin LTO, `codegen-units = 1`, `panic = "abort"`, and `strip = true`. The tokio runtime is `current_thread`. Fly sets `TOKIO_WORKER_THREADS=1`. The image builds on `rust:1.98.1-bookworm` with `cargo build --release --locked` and runs on `debian:bookworm-slim` as `nobody`.
- **Consequences:** Do not switch to the multi-thread runtime without a new record. Do not drop `--locked` from the image build. The runtime image does not include rustc or cargo.

## fmt, clippy, test, audit, and gitleaks

- **Status:** Accepted
- **Context:** Local commits and CI have to run the same gate.
- **Decision:** `make check` runs `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets --all-features -- -D warnings`, `cargo test --locked`, `cargo audit`, and `gitleaks detect --source .`. `make hooks` installs pre-commit and points `core.hooksPath` at `.githooks`. CI runs those five checks as separate jobs. The test job provides Postgres 16.
- **Consequences:** A change that fails any check does not land. The emergency skip `SKIP=fmt,clippy,local-tests,audit,gitleaks git commit` is not the normal path.

## Private test database on Postgres 16

- **Status:** Accepted
- **Context:** Route tests need real `v1_*` views. They must not migrate or rewrite any other database on the server.
- **Decision:** Tests `CREATE DATABASE` a private database on the Postgres 16 server in `DATABASE_URL`, seed views there, exercise the shipped handlers, and drop that database afterward.
- **Consequences:** `cargo test` fails closed when that server is unreachable. Do not retarget the suite at a shared catalog it will mutate.
