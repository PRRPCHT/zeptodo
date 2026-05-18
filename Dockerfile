# syntax=docker/dockerfile:1.7

# Stage 1: build the Tailwind/DaisyUI CSS bundle and copy vendored JS/font assets
FROM node:22-alpine AS css-builder
WORKDIR /build
COPY package.json package-lock.json ./
RUN npm ci
COPY static/css ./static/css
COPY templates ./templates
RUN npm run build:css

# Stage 2: compile the Rust binary against debian-bookworm
FROM rust:1-bookworm AS rust-builder
WORKDIR /build

# Cache the dependency layer by building against a dummy main.rs first
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --bin zeptodo && rm -rf src target/release/deps/zeptodo-* target/release/zeptodo*

# Build the real binary
COPY src ./src
COPY templates ./templates
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release --bin zeptodo

# Stage 3: runtime image (debian-slim, x86-64 only)
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        wget \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 zeptodo \
    && useradd --system --uid 1000 --gid zeptodo --home-dir /app --shell /usr/sbin/nologin zeptodo

WORKDIR /app

COPY --from=rust-builder /build/target/release/zeptodo /usr/local/bin/zeptodo
COPY static ./static
COPY --from=css-builder /build/static/app.css ./static/app.css
COPY --from=css-builder /build/static/files ./static/files
COPY --from=css-builder /build/static/vendor ./static/vendor
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

RUN mkdir -p /data /data/logs && chown -R zeptodo:zeptodo /data /app

ENV BIND_ADDR=0.0.0.0:8080 \
    DATABASE_URL=sqlite:///data/zeptodo.db \
    BASE_URL=http://localhost:8080 \
    TIMEZONE=UTC \
    LOG_DIR=/data/logs

EXPOSE 8080
VOLUME ["/data"]

USER zeptodo

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:8080/login || exit 1

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/zeptodo"]
