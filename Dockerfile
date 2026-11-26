# Multi-stage production Dockerfile for kook-tavern-dm (GNU AGPL-3.0)
FROM rust:1-slim-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p kook-tavern-dm

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --home-dir /app --shell /usr/sbin/nologin tavern

WORKDIR /app
COPY --from=builder /build/target/release/kook-tavern-dm /usr/local/bin/kook-tavern-dm
COPY --chown=tavern:tavern prompts ./prompts
COPY --chown=tavern:tavern scenarios ./scenarios
COPY --chown=tavern:tavern worldbooks ./worldbooks

RUN mkdir -p /app/data /app/exports /app/scenarios/community \
    && chown -R tavern:tavern /app/data /app/exports /app/scenarios/community \
    && chmod 750 /app/scenarios/community

USER tavern
EXPOSE 8787

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8787/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/kook-tavern-dm"]
CMD ["kook"]
