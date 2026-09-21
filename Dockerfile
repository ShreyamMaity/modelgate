# Multi-stage build to a small static-ish image.
# NOTE: not yet built locally (no Docker on the dev machine) - CI builds it.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev gcc
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src src
COPY assets assets
RUN cargo build --release --locked

FROM alpine:3
RUN apk add --no-cache ca-certificates \
 && adduser -D -u 10001 app \
 && mkdir /data && chown app /data
COPY --from=build /src/target/release/modelgate /usr/local/bin/modelgate
USER app
ENV LISTEN=0.0.0.0:8080 CONFIG=/data/chains.json
VOLUME /data
EXPOSE 8080
HEALTHCHECK --interval=10s --timeout=3s --retries=3 \
  CMD wget -q --spider http://127.0.0.1:8080/health || exit 1
ENTRYPOINT ["modelgate"]
