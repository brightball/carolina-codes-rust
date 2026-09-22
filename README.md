# carolina-codes-rust

Read-only [axum](https://github.com/tokio-rs/axum) + `tokio-postgres` API for the Carolina Code Conference polyglot site.

Queries PostgreSQL **v1 views** only (`v1_years`, `v1_speakers`, `v1_talks`, `v1_sponsors`, `v1_year_sponsors`, `v1_sponsorships`). Registers with Elixir once on boot.

```bash
make test        # cargo test
make clippy      # cargo clippy -D warnings
make audit       # cargo audit (RustSec lockfile scan)
make secrets     # gitleaks detect --source .
make fmt         # cargo fmt --check
make check       # all of the above
make hooks       # install local pre-commit hooks
```

Pre-commit runs the same five checks (`rustfmt`, `clippy`, `local tests`, `cargo audit`, `gitleaks`). Install once with `make hooks` (needs `pre-commit` on PATH). Emergency skip: `SKIP=fmt,clippy,local-tests,audit,gitleaks git commit`.

Catalog tests create a private database on the Postgres server from `DATABASE_URL` (default `127.0.0.1:5432`), seed `v1_*` views there, and drop that database afterward. They do not modify any other database. `cargo test` fails if that server is unreachable. Gitea Actions prepares the workspace once, then runs each check as its own job (`.gitea/workflows/precommit.yml`); the test job provides Postgres 16. The process binds `/health` before Postgres warmup or CMS registration. Fly idle machines suspend (256mb) instead of stopping from zero.

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
