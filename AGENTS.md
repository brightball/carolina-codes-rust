# carolina-codes-rust

Read-only v1 polyglot API. See README.md for install, run, and test commands. `make check` (and `pre-commit run --all-files`) runs rustfmt, clippy `-D warnings`, cargo test, cargo audit, and gitleaks. Install hooks with `make hooks`. Emergency skip: `SKIP=fmt,clippy,local-tests,audit,gitleaks git commit`.

## Cursor Cloud specific instructions

This repository is one sibling git remote in the carolina.codes polyglot fleet. Cloud agents should treat **this repo** as the workspace root. The Phoenix CMS is a different remote (`github.com/brightball/carolina-codes`); do not assume `../elixir` or other sibling directories exist unless those remotes are attached to the same Cloud environment.

Postgres `v1_*` views live in the CMS database. Route tests create a private database of those views on the server in `DATABASE_URL` and do not modify other databases; start Postgres 16 before `cargo test`. For a running API against the CMS views, set:

- `DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/carolina_dev`
- `CAROLINA_URL=http://127.0.0.1:4000` (optional; registration no-ops if CMS is down)
- `POLYGLOT_REGISTER_TOKEN=dev`
- `PUBLIC_BASE_URL` / `PORT` as in the README

Do not query Ash tables. Do not fold this tree into the CMS git remote. Contract: CMS `priv/api/openapi.yaml` + `priv/api/AGENTS.md`.
