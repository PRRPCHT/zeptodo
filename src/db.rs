use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use std::str::FromStr;

/// Build a SQLite connection pool and run all pending migrations.
///
/// ### Description
/// The pool is configured to create the database file if missing and to
/// enable WAL journaling and foreign keys, both required by the schema
/// introduced in later sprints.
///
/// ### Arguments
/// - `database_url`: SQLx-compatible SQLite URL (e.g. `sqlite://zeptodo.db`).
///
/// ### Returns
/// - `Ok(SqlitePool)`: Pool ready to serve queries with migrations applied.
/// - `Err`: Connection or migration failed.
pub async fn init(database_url: &str) -> Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid DATABASE_URL: {database_url}"))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .log_statements(tracing::log::LevelFilter::Debug);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .context("failed to open SQLite pool")?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    Ok(pool)
}
