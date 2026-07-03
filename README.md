# Zeptodo

An ultra-minimalist, self-hostable mono-user to-do web app, REST API-enabled. Basically the digital pendant to a sheet of paper and a pen.

## Stack

- Rust + Axum + SQLx (SQLite)
- Askama templates + Tailwind v4 + DaisyUI v5
- HTMX + Alpine.js

## Local development

### 1. Install JavaScript build dependencies

Node and npm are only required at build time (to compile the CSS bundle). The runtime container is Rust-only.

```bash
npm install
```

### 2. Build the CSS bundle

```bash
npm run build:css
```

This produces `static/app.css` from `static/css/input.css` using the Tailwind v4 CLI with DaisyUI v5 loaded as a plugin (CSS-first config; no `tailwind.config.js`).

Use `npm run watch:css` during development for incremental rebuilds.

### 3. Configure the environment

Create a `.env` file (loaded by `dotenvy` in dev; real environment variables always win):

```env
BIND_ADDR=127.0.0.1:8080
DATABASE_URL=sqlite://data/zeptodo.db
BASE_URL=http://localhost:8080
SESSION_SECRET=replace-with-32-bytes-of-random-data-please
USERNAME=admin
PASSWORD=changeme
TIMEZONE=UTC
# LOG_DIR=./logs
```

### 5. Run the server

```bash
cargo run
```

Then visit `http://localhost:8080/` and enjoy!

When `LOG_DIR` is set, daily-rotated JSON logs land under that directory with a 7-day retention. Stdout JSON logging is always enabled.

## Docker deployment

A multi-stage `Dockerfile` is provided. The image is published to GHCR for
`linux/amd64` and is the recommended way to self-host Zeptodo.

### Run with docker-compose (recommended)

```bash
# 1. Create a host data directory (the SQLite file and logs land here)
mkdir -p ./data

# 2. Edit docker-compose.yml: set SESSION_SECRET, USERNAME, PASSWORD, BASE_URL, TIMEZONE
#    Generate a session secret with: openssl rand -base64 48

# 3. Start the service
docker compose up -d

# 4. Tail logs
docker compose logs -f
```

The default compose file pulls `ghcr.io/prrpcht/zeptodo:latest`. To build the
image locally instead, comment out the `image:` line and uncomment the `build:`
block in `docker-compose.yml`.

### Run with plain docker

```bash
docker run -d \
  --name zeptodo \
  -p 8080:8080 \
  -v "$(pwd)/data:/data" \
  -e SESSION_SECRET="$(openssl rand -base64 48)" \
  -e USERNAME=admin \
  -e PASSWORD=changeme \
  -e BASE_URL=http://localhost:8080 \
  -e TIMEZONE=UTC \
  ghcr.io/prrpcht/zeptodo:latest
```

The container runs as a non-root user (uid/gid 1000). Make sure the host
`./data` directory is writable by that user, or set the ownership to match:

```bash
sudo chown -R 1000:1000 ./data
```

### Environment variables

| Variable | Default in image | Purpose |
|---|---|---|
| `BIND_ADDR` | `0.0.0.0:8080` | host:port to bind inside the container |
| `BASE_URL` | `http://localhost:8080` | Public URL of the instance |
| `DATABASE_URL` | `sqlite:///data/zeptodo.db` | SQLite file path |
| `SESSION_SECRET` | (none, required) | 32+ random bytes for session cookie signing. Generate with `openssl rand -base64 48`. |
| `USERNAME` | (none, required at first boot) | Login username |
| `PASSWORD` | (none, required at first boot) | Login password (plaintext in env, hashed before storage) |
| `TIMEZONE` | `UTC` | IANA timezone name (e.g. `Europe/Paris`) |
| `BEHIND_PROXY` | `false` | Set to `true` only when a trusted reverse proxy sits in front. Then rate limiting trusts `X-Forwarded-For` / `X-Real-Ip` / `Forwarded`. Left `false` (the default), rate limiting keys on the direct socket address, which cannot be spoofed. Enabling this while exposed directly lets clients bypass the login and global rate limits by forging the header. |
| `LOG_DIR` | `/data/logs` | Daily-rotated JSON logs with 7-day retention. Stdout is always on. |

### Credentials rotation

`USERNAME`, `PASSWORD`, and `TIMEZONE` are reconciled against the stored row
on every startup. Empty or absent values mean "no change". To rotate:

```bash
# 1. Update the value in docker-compose.yml (or pass via -e)
# 2. Restart so the new value is written:
docker compose up -d

# 3. Clear the plaintext from the environment and restart again so it does
#    not linger in the process environment:
#    (edit docker-compose.yml, comment out the line or set it to empty)
docker compose up -d
```

### Upgrade path

Migrations are embedded in the binary and run automatically on startup. To
upgrade, pull the new image tag and restart:

```bash
docker compose pull
docker compose up -d
```

The on-disk database is preserved through the `/data` volume.

## Quality checks

All three commands must pass before any task is considered done:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
