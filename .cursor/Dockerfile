FROM rust:1-bookworm
USER root
RUN apt-get update \
 && apt-get install -y --no-install-recommends git ca-certificates pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*
