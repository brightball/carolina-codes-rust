# carolina-codes-rust

Read-only [axum](https://github.com/tokio-rs/axum) + `tokio-postgres` API for the Carolina Code Conference polyglot site.

Queries PostgreSQL **v1 views** only (`v1_years`, `v1_speakers`, `v1_talks`, `v1_sponsors`, `v1_year_sponsors`, `v1_sponsorships`). Registers with Elixir once on boot.

```bash
cargo run
```

```bash
DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/carolina_dev \
CAROLINA_URL=http://127.0.0.1:4000 \
POLYGLOT_REGISTER_TOKEN=dev \
PUBLIC_BASE_URL=http://127.0.0.1:4005 \
PORT=4005 \
cargo run
```

`GET /` reports `language: "Rust"` and `framework: "axum"`. Responses include `X-Polyglot-Language` and `X-Polyglot-Framework`.
