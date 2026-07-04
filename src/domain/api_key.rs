use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use subtle::ConstantTimeEq;

/// Number of random bytes used to mint a new API key.
const KEY_BYTES: usize = 32;

/// Length of the visible key prefix stored alongside the hash.
const PREFIX_LEN: usize = 8;

/// Minimum age of `last_used_at` before a repeat use refreshes it.
const LAST_USED_THROTTLE_SECONDS: i64 = 60;

/// A persisted API key row. Plaintext value is never stored.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
pub struct ApiKey {
    pub id: i64,
    pub key_prefix: String,
    pub key_hash: String,
    pub description: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// User-selectable expiration windows for a newly created or edited key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryChoice {
    Days30,
    Days90,
    Days180,
    Year1,
    Never,
}

impl ExpiryChoice {
    /// Return the form-value identifier for this choice.
    ///
    /// ### Returns
    /// - `&'static str`: `"30"`, `"90"`, `"180"`, `"365"`, or `"never"`.
    #[allow(dead_code)]
    pub fn as_str(self) -> &'static str {
        match self {
            ExpiryChoice::Days30 => "30",
            ExpiryChoice::Days90 => "90",
            ExpiryChoice::Days180 => "180",
            ExpiryChoice::Year1 => "365",
            ExpiryChoice::Never => "never",
        }
    }

    /// Parse a string identifier into an [`ExpiryChoice`].
    ///
    /// ### Arguments
    /// - `value`: One of `"30"`, `"90"`, `"180"`, `"365"`, `"never"`.
    ///
    /// ### Returns
    /// - `Ok(ExpiryChoice)`: A recognized value.
    /// - `Err`: The string was not a recognized choice.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "30" => Ok(ExpiryChoice::Days30),
            "90" => Ok(ExpiryChoice::Days90),
            "180" => Ok(ExpiryChoice::Days180),
            "365" => Ok(ExpiryChoice::Year1),
            "never" => Ok(ExpiryChoice::Never),
            other => Err(anyhow!("invalid expiry choice: {other}")),
        }
    }

    /// Resolve this choice into an absolute `expires_at` value.
    ///
    /// ### Arguments
    /// - `now`: The reference instant used as the start of the window.
    ///
    /// ### Returns
    /// - `Some(DateTime<Utc>)`: A bounded expiry instant.
    /// - `None`: The key never expires.
    pub fn to_expires_at(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            ExpiryChoice::Days30 => Some(now + Duration::days(30)),
            ExpiryChoice::Days90 => Some(now + Duration::days(90)),
            ExpiryChoice::Days180 => Some(now + Duration::days(180)),
            ExpiryChoice::Year1 => Some(now + Duration::days(365)),
            ExpiryChoice::Never => None,
        }
    }
}

/// Outcome of [`create`]: the persisted row plus the one-time plaintext token.
#[derive(Debug, Clone)]
pub struct CreatedKey {
    pub record: ApiKey,
    pub plaintext: String,
}

/// Generate 32 random bytes and encode them as base64url without padding.
///
/// ### Returns
/// - `String`: A 43-character URL-safe token suitable for `Authorization: Bearer`.
fn mint_plaintext() -> String {
    let mut bytes = [0u8; KEY_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the SHA-256 hash of a plaintext key, hex-encoded.
///
/// ### Arguments
/// - `plaintext`: The full token as supplied by the client.
///
/// ### Returns
/// - `String`: Lowercase hex of the SHA-256 digest (64 characters).
pub fn hash(plaintext: &str) -> String {
    let digest = Sha256::digest(plaintext.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

const SELECT_COLUMNS: &str =
    "id, key_prefix, key_hash, description, expires_at, last_used_at, created_at";

/// List all API key rows, newest first.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
///
/// ### Returns
/// - `Ok(Vec<ApiKey>)`: Rows ordered by `created_at DESC`.
/// - `Err`: SQLite query failed.
pub async fn list(pool: &SqlitePool) -> Result<Vec<ApiKey>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM api_keys ORDER BY created_at DESC");
    sqlx::query_as::<_, ApiKey>(&sql)
        .fetch_all(pool)
        .await
        .context("listing api keys")
}

/// Fetch a single API key by id.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the row to load.
///
/// ### Returns
/// - `Ok(Some(ApiKey))`: A row exists with this id.
/// - `Ok(None)`: No row matched.
/// - `Err`: SQLite query failed.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<ApiKey>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM api_keys WHERE id = ?");
    sqlx::query_as::<_, ApiKey>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("loading api key")
}

/// Mint a new API key and persist its hash.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `description`: Optional human-readable description.
/// - `expiry`: User-selected expiration window.
///
/// ### Returns
/// - `Ok(CreatedKey)`: Persisted record plus the one-time plaintext token.
/// - `Err`: SQLite query failed or the freshly inserted row could not be read back.
pub async fn create(
    pool: &SqlitePool,
    description: Option<String>,
    expiry: ExpiryChoice,
) -> Result<CreatedKey> {
    let now = Utc::now();
    let plaintext = mint_plaintext();
    let key_hash = hash(&plaintext);
    let key_prefix: String = plaintext.chars().take(PREFIX_LEN).collect();
    let expires_at = expiry.to_expires_at(now);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO api_keys (key_prefix, key_hash, description, expires_at, created_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(&description)
    .bind(expires_at)
    .bind(now)
    .fetch_one(pool)
    .await
    .context("inserting api key")?;

    let record = get(pool, id)
        .await?
        .ok_or_else(|| anyhow!("api key {id} vanished after insert"))?;
    Ok(CreatedKey { record, plaintext })
}

/// Update the expiration window of an existing API key.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the row to update.
/// - `expiry`: New expiration window, computed against the current instant.
///
/// ### Returns
/// - `Ok(Some(ApiKey))`: The row was updated.
/// - `Ok(None)`: No row matched.
/// - `Err`: SQLite query failed.
pub async fn update_expiry(
    pool: &SqlitePool,
    id: i64,
    expiry: ExpiryChoice,
) -> Result<Option<ApiKey>> {
    let now = Utc::now();
    let expires_at = expiry.to_expires_at(now);
    let rows = sqlx::query("UPDATE api_keys SET expires_at = ? WHERE id = ?")
        .bind(expires_at)
        .bind(id)
        .execute(pool)
        .await
        .context("updating api key expiry")?
        .rows_affected();
    if rows == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// Update the description of an existing API key.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the row to update.
/// - `description`: New description. `None` or empty string clears it.
///
/// ### Returns
/// - `Ok(Some(ApiKey))`: The row was updated.
/// - `Ok(None)`: No row matched.
/// - `Err`: SQLite query failed.
pub async fn update_description(
    pool: &SqlitePool,
    id: i64,
    description: Option<String>,
) -> Result<Option<ApiKey>> {
    let rows = sqlx::query("UPDATE api_keys SET description = ? WHERE id = ?")
        .bind(&description)
        .bind(id)
        .execute(pool)
        .await
        .context("updating api key description")?
        .rows_affected();
    if rows == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// Outcome of [`verify`]: the id of the matched key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedKey {
    pub id: i64,
}

/// Resolve a bearer token against the stored keys.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `plaintext`: Bearer token as supplied by the client.
///
/// ### Returns
/// - `Ok(Some(VerifiedKey))`: The token matched a non-expired row.
/// - `Ok(None)`: No match, mismatch, or the key has expired.
/// - `Err`: SQLite query failed.
pub async fn verify(pool: &SqlitePool, plaintext: &str) -> Result<Option<VerifiedKey>> {
    let computed = hash(plaintext);
    let row: Option<(i64, String, Option<DateTime<Utc>>)> =
        sqlx::query_as("SELECT id, key_hash, expires_at FROM api_keys WHERE key_hash = ?")
            .bind(&computed)
            .fetch_optional(pool)
            .await
            .context("looking up api key")?;
    let Some((id, stored_hash, expires_at)) = row else {
        return Ok(None);
    };
    if !bool::from(stored_hash.as_bytes().ct_eq(computed.as_bytes())) {
        return Ok(None);
    }
    if expires_at.is_some_and(|when| when <= Utc::now()) {
        return Ok(None);
    }
    Ok(Some(VerifiedKey { id }))
}

/// Stamp `last_used_at` for an API key and report whether this was its first use.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the row to stamp.
///
/// ### Returns
/// - `Ok(true)`: The row existed and this call flipped its `NULL` `last_used_at`.
/// - `Ok(false)`: The row did not exist, or `last_used_at` was already set.
/// - `Err`: SQLite query failed.
pub async fn mark_used(pool: &SqlitePool, id: i64) -> Result<bool> {
    let now = Utc::now();
    let first_use =
        sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2 AND last_used_at IS NULL")
            .bind(now)
            .bind(id)
            .execute(pool)
            .await
            .context("stamping first api key use")?
            .rows_affected()
            == 1;
    if first_use {
        return Ok(true);
    }
    let throttle_cutoff = now - Duration::seconds(LAST_USED_THROTTLE_SECONDS);
    sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2 AND last_used_at < ?3")
        .bind(now)
        .bind(id)
        .bind(throttle_cutoff)
        .execute(pool)
        .await
        .context("refreshing api key last_used_at")?;
    Ok(false)
}

/// Delete an API key by id.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the row to delete.
///
/// ### Returns
/// - `Ok(true)`: A row was deleted.
/// - `Ok(false)`: No row matched.
/// - `Err`: SQLite query failed.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let rows = sqlx::query("DELETE FROM api_keys WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting api key")?
        .rows_affected();
    Ok(rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    #[test]
    fn hash_is_deterministic_and_hex() {
        let h1 = hash("hello");
        let h2 = hash("hello");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(hash("hello"), hash("world"));
    }

    #[tokio::test]
    async fn create_persists_hash_only() {
        let pool = pool().await;
        let created = create(&pool, Some("ci runner".into()), ExpiryChoice::Days30)
            .await
            .unwrap();
        assert_eq!(created.record.key_hash, hash(&created.plaintext));
        assert_eq!(
            created.record.key_prefix,
            created.plaintext.chars().take(8).collect::<String>()
        );
        assert_eq!(created.record.description.as_deref(), Some("ci runner"));
        assert!(created.record.expires_at.is_some());
        assert!(created.plaintext.len() >= 32);
    }

    #[tokio::test]
    async fn create_never_expiration_is_null() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        assert!(created.record.expires_at.is_none());
    }

    #[tokio::test]
    async fn create_minted_plaintexts_are_unique() {
        let pool = pool().await;
        let a = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        let b = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        assert_ne!(a.plaintext, b.plaintext);
        assert_ne!(a.record.key_hash, b.record.key_hash);
    }

    #[tokio::test]
    async fn update_expiry_switches_between_bounded_and_never() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Days30).await.unwrap();
        let id = created.record.id;

        let never = update_expiry(&pool, id, ExpiryChoice::Never)
            .await
            .unwrap()
            .unwrap();
        assert!(never.expires_at.is_none());

        let bounded = update_expiry(&pool, id, ExpiryChoice::Year1)
            .await
            .unwrap()
            .unwrap();
        assert!(bounded.expires_at.is_some());
    }

    #[tokio::test]
    async fn update_description_round_trips() {
        let pool = pool().await;
        let created = create(&pool, Some("a".into()), ExpiryChoice::Never)
            .await
            .unwrap();
        let id = created.record.id;
        let updated = update_description(&pool, id, Some("b".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.description.as_deref(), Some("b"));
        let cleared = update_description(&pool, id, None).await.unwrap().unwrap();
        assert!(cleared.description.is_none());
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        assert!(delete(&pool, created.record.id).await.unwrap());
        assert!(get(&pool, created.record.id).await.unwrap().is_none());
        assert!(!delete(&pool, created.record.id).await.unwrap());
    }

    #[tokio::test]
    async fn list_orders_newest_first() {
        let pool = pool().await;
        let _a = create(&pool, Some("first".into()), ExpiryChoice::Never)
            .await
            .unwrap();
        let b = create(&pool, Some("second".into()), ExpiryChoice::Never)
            .await
            .unwrap();
        let rows = list(&pool).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, b.record.id);
    }

    #[tokio::test]
    async fn verify_accepts_matching_unexpired_token() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Year1).await.unwrap();
        let result = verify(&pool, &created.plaintext).await.unwrap();
        assert_eq!(
            result,
            Some(VerifiedKey {
                id: created.record.id
            })
        );
    }

    #[tokio::test]
    async fn verify_rejects_unknown_token() {
        let pool = pool().await;
        let _ = create(&pool, None, ExpiryChoice::Year1).await.unwrap();
        let result = verify(&pool, "definitely-not-a-real-token").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn verify_rejects_expired_token() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Year1).await.unwrap();
        let past = Utc::now() - Duration::days(1);
        sqlx::query("UPDATE api_keys SET expires_at = ? WHERE id = ?")
            .bind(past)
            .bind(created.record.id)
            .execute(&pool)
            .await
            .unwrap();
        let result = verify(&pool, &created.plaintext).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn mark_used_reports_first_use_then_false() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        let first = mark_used(&pool, created.record.id).await.unwrap();
        assert!(first);
        let second = mark_used(&pool, created.record.id).await.unwrap();
        assert!(!second);
        let reloaded = get(&pool, created.record.id).await.unwrap().unwrap();
        assert!(reloaded.last_used_at.is_some());
    }

    #[tokio::test]
    async fn mark_used_within_throttle_window_skips_refresh() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        assert!(mark_used(&pool, created.record.id).await.unwrap());
        let stamped = get(&pool, created.record.id)
            .await
            .unwrap()
            .unwrap()
            .last_used_at;
        assert!(!mark_used(&pool, created.record.id).await.unwrap());
        let unchanged = get(&pool, created.record.id)
            .await
            .unwrap()
            .unwrap()
            .last_used_at;
        assert_eq!(stamped, unchanged);
    }

    #[tokio::test]
    async fn mark_used_refreshes_after_throttle_window() {
        let pool = pool().await;
        let created = create(&pool, None, ExpiryChoice::Never).await.unwrap();
        assert!(mark_used(&pool, created.record.id).await.unwrap());
        let stale = Utc::now() - Duration::seconds(LAST_USED_THROTTLE_SECONDS + 1);
        sqlx::query("UPDATE api_keys SET last_used_at = ?1 WHERE id = ?2")
            .bind(stale)
            .bind(created.record.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!mark_used(&pool, created.record.id).await.unwrap());
        let refreshed = get(&pool, created.record.id)
            .await
            .unwrap()
            .unwrap()
            .last_used_at
            .unwrap();
        assert!(refreshed > stale);
    }

    #[test]
    fn expiry_choice_round_trips_through_str() {
        for c in [
            ExpiryChoice::Days30,
            ExpiryChoice::Days90,
            ExpiryChoice::Days180,
            ExpiryChoice::Year1,
            ExpiryChoice::Never,
        ] {
            assert_eq!(ExpiryChoice::parse(c.as_str()).unwrap(), c);
        }
        assert!(ExpiryChoice::parse("nope").is_err());
    }
}
