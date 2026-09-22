# Fail-closed local checks. Same commands as .pre-commit-config.yaml and
# the five parallel Gitea jobs. Emergency skip:
#   SKIP=fmt,clippy,local-tests,audit,gitleaks git commit
CARGO ?= cargo
GITLEAKS ?= gitleaks
export PATH := $(HOME)/.local/share/mise/installs/gitleaks/latest:$(HOME)/.cargo/bin:$(HOME)/.local/bin:$(PATH)

.PHONY: test clippy audit secrets fmt check hooks

test:
	$(CARGO) test --locked

clippy:
	$(CARGO) clippy --locked --all-targets --all-features -- -D warnings

audit:
	$(CARGO) audit

secrets:
	$(GITLEAKS) detect --source . --verbose

fmt:
	$(CARGO) fmt --all -- --check

check: fmt clippy test audit secrets

hooks:
	pre-commit install
	git config core.hooksPath .githooks
