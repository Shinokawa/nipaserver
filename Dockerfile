# syntax=docker/dockerfile:1.7

FROM node:26-bookworm-slim AS web-builder
WORKDIR /src/webui/app
COPY webui/app/package.json webui/app/package-lock.json ./
RUN npm ci
COPY webui/app/ ./
RUN npm run check && npm run build

FROM rust:1.88-bookworm AS rust-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY README.md LICENSE ./
COPY crates/ ./crates/
COPY nipa-agent/ ./nipa-agent/
RUN cargo build --release --locked -p nipa-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 nipa \
    && useradd --uid 10001 --gid nipa --home-dir /app --create-home nipa

WORKDIR /app
COPY --from=rust-builder /src/target/release/nipa-server /usr/local/bin/nipa-server
COPY --from=web-builder /src/webui/app/dist/ /app/webui/app/dist/

RUN mkdir -p /data && chown -R nipa:nipa /app /data
USER nipa

ENV NIPA_BIND=0.0.0.0 \
    NIPA_PORT=11810 \
    NIPA_DATA_DIR=/data \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 11810

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:11810/api/v1/system/info >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/nipa-server"]
