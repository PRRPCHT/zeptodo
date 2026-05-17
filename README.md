# Zeptodo

An ultra-minimalist, self-hostable mono-user to-do web app, REST API-enabled. Basically the digital pendant to a sheet of paper and a pen.

## Stack

- Rust + Axum + SQLx (SQLite)
- Askama templates + Tailwind v4 + DaisyUI v5
- HTMX + Alpine.js

## First boot

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

## Quality checks

All three commands must pass before any sprint is considered done:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```
