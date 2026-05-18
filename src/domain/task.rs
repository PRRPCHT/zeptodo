use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteRow;

/// Closed status enum for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Todo,
    InProgress,
    Done,
    Cancelled,
}

impl serde::Serialize for Status {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Status {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = <&str>::deserialize(deserializer)?;
        Status::parse(raw).map_err(serde::de::Error::custom)
    }
}

impl Status {
    /// Return the snake_case identifier used in SQL `CHECK` constraints.
    ///
    /// ### Returns
    /// - `&'static str`: `"todo"`, `"in_progress"`, `"done"`, or `"cancelled"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Todo => "todo",
            Status::InProgress => "in_progress",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
        }
    }

    /// Parse a string identifier into a [`Status`].
    ///
    /// ### Arguments
    /// - `value`: One of `"todo"`, `"in_progress"`, `"done"`, `"cancelled"`.
    ///
    /// ### Returns
    /// - `Ok(Status)`: A recognized value.
    /// - `Err`: The string was not a recognized status.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "todo" => Ok(Status::Todo),
            "in_progress" => Ok(Status::InProgress),
            "done" => Ok(Status::Done),
            "cancelled" => Ok(Status::Cancelled),
            other => Err(anyhow!("invalid status: {other}")),
        }
    }
}

/// A persisted task row.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: Status,
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<'r> sqlx::FromRow<'r, SqliteRow> for Task {
    fn from_row(row: &'r SqliteRow) -> std::result::Result<Self, sqlx::Error> {
        let status_str: String = row.try_get("status")?;
        let status = Status::parse(&status_str).map_err(|e| sqlx::Error::ColumnDecode {
            index: "status".into(),
            source: e.into(),
        })?;
        Ok(Task {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            description: row.try_get("description")?,
            status,
            position: row.try_get("position")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// Fields required to create a new task. `status` always starts at `Todo`,
/// `position` is assigned by the repository.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub title: String,
    pub description: Option<String>,
}

/// Fields editable by the inline edit form. Status is mutated through
/// [`set_status`] instead so the dropdown can submit independently.
#[derive(Debug, Clone)]
pub struct UpdateTask {
    pub title: String,
    pub description: Option<String>,
}

const SELECT_COLUMNS: &str = "id, title, description, status, position, created_at, updated_at";

/// List tasks in display order, filtered by status.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `statuses`: Statuses to include. An empty slice returns no rows.
///
/// ### Returns
/// - `Ok(Vec<Task>)`: Tasks ordered by `position ASC`.
/// - `Err`: SQLite query failed.
pub async fn list(pool: &SqlitePool, statuses: &[Status]) -> Result<Vec<Task>> {
    if statuses.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; statuses.len()].join(", ");
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM tasks \
         WHERE status IN ({placeholders}) ORDER BY position ASC"
    );
    let mut query = sqlx::query_as::<_, Task>(&sql);
    for status in statuses {
        query = query.bind(status.as_str());
    }
    query.fetch_all(pool).await.context("listing tasks")
}

/// Fetch a single task by id.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the task to load.
///
/// ### Returns
/// - `Ok(Some(Task))`: A task with this id exists.
/// - `Ok(None)`: No task with this id.
/// - `Err`: SQLite query failed.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<Task>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM tasks WHERE id = ?");
    sqlx::query_as::<_, Task>(&sql)
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("loading task")
}

/// Insert a new task and assign it the next free `position`.
///
/// ### Description
/// The position is computed as `COALESCE(MAX(position), 0) + 1` inside the
/// same transaction as the insert, preventing two concurrent inserts from
/// claiming the same position and violating the `UNIQUE` constraint.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `dto`: Fields supplied by the user. Status starts at `Todo`.
///
/// ### Returns
/// - `Ok(Task)`: The freshly persisted task with its assigned `id` and `position`.
/// - `Err`: SQLite query failed or the inserted row could not be read back.
pub async fn create(pool: &SqlitePool, dto: NewTask) -> Result<Task> {
    let mut tx = pool.begin().await.context("starting create tx")?;
    let max_pos: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position), 0) FROM tasks")
        .fetch_one(&mut *tx)
        .await
        .context("reading max position")?;
    let now = Utc::now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tasks (title, description, status, position, created_at, updated_at) \
         VALUES (?, ?, 'todo', ?, ?, ?) RETURNING id",
    )
    .bind(&dto.title)
    .bind(&dto.description)
    .bind(max_pos + 1)
    .bind(now)
    .bind(now)
    .fetch_one(&mut *tx)
    .await
    .context("inserting task")?;
    tx.commit().await.context("committing create tx")?;

    get(pool, id)
        .await?
        .ok_or_else(|| anyhow!("task {id} vanished after insert"))
}

/// Update the editable fields of a task.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the task to update.
/// - `dto`: New values for the editable columns.
///
/// ### Returns
/// - `Ok(Some(Task))`: The task was updated and is returned in its new state.
/// - `Ok(None)`: No task with this id; nothing was changed.
/// - `Err`: SQLite query failed.
pub async fn update(pool: &SqlitePool, id: i64, dto: UpdateTask) -> Result<Option<Task>> {
    let rows = sqlx::query("UPDATE tasks SET title = ?, description = ? WHERE id = ?")
        .bind(&dto.title)
        .bind(&dto.description)
        .bind(id)
        .execute(pool)
        .await
        .context("updating task")?
        .rows_affected();
    if rows == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// Change the status of a task.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the task to update.
/// - `status`: New status value.
///
/// ### Returns
/// - `Ok(Some(Task))`: The status was updated and the task is returned.
/// - `Ok(None)`: No task with this id.
/// - `Err`: SQLite query failed.
pub async fn set_status(pool: &SqlitePool, id: i64, status: Status) -> Result<Option<Task>> {
    let rows = sqlx::query("UPDATE tasks SET status = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(id)
        .execute(pool)
        .await
        .context("updating task status")?
        .rows_affected();
    if rows == 0 {
        return Ok(None);
    }
    get(pool, id).await
}

/// Rewrite positions to match a new visible ordering.
///
/// ### Description
/// The supplied `ordered_ids` are interpreted as the user-visible order. The
/// set of positions those ids currently occupy is reused: after the call, the
/// ids end up sorted into that same set of positions but in the supplied
/// order. Hidden rows keep their positions untouched.
///
/// To stay inside the `UNIQUE(position)` constraint mid-rewrite, the rows are
/// first parked at distinct negative positions, then assigned their final
/// values inside the same transaction.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `ordered_ids`: Task ids in their new visible order. Unknown ids are
///   ignored, and rows whose status is `Done` or `Cancelled` are skipped so
///   terminal tasks keep their positions.
///
/// ### Returns
/// - `Ok(usize)`: Number of rows whose positions were actually rewritten.
/// - `Err`: SQLite query failed.
pub async fn reorder(pool: &SqlitePool, ordered_ids: &[i64]) -> Result<usize> {
    if ordered_ids.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await.context("starting reorder tx")?;

    let mut current_positions: Vec<i64> = Vec::with_capacity(ordered_ids.len());
    let mut effective_ids: Vec<i64> = Vec::with_capacity(ordered_ids.len());
    for id in ordered_ids {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT position, status FROM tasks WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await
                .context("reading current position")?;
        if let Some((position, status)) = row {
            let parsed =
                Status::parse(&status).map_err(|e| anyhow!("invalid stored status: {e}"))?;
            if matches!(parsed, Status::Done | Status::Cancelled) {
                continue;
            }
            current_positions.push(position);
            effective_ids.push(*id);
        }
    }
    if effective_ids.is_empty() {
        return Ok(0);
    }
    let mut slots = current_positions.clone();
    slots.sort_unstable();

    for (i, id) in effective_ids.iter().enumerate() {
        let parking = -(i as i64) - 1;
        sqlx::query("UPDATE tasks SET position = ? WHERE id = ?")
            .bind(parking)
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("parking row for reorder")?;
    }
    for (i, id) in effective_ids.iter().enumerate() {
        sqlx::query("UPDATE tasks SET position = ? WHERE id = ?")
            .bind(slots[i])
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("writing final position")?;
    }
    tx.commit().await.context("committing reorder tx")?;
    Ok(effective_ids.len())
}

/// Delete a task by id.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `id`: Primary key of the task to delete.
///
/// ### Returns
/// - `Ok(true)`: A row was deleted.
/// - `Ok(false)`: No row matched the id; nothing was changed.
/// - `Err`: SQLite query failed.
pub async fn delete(pool: &SqlitePool, id: i64) -> Result<bool> {
    let rows = sqlx::query("DELETE FROM tasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await
        .context("deleting task")?
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

    fn new(title: &str) -> NewTask {
        NewTask {
            title: title.into(),
            description: None,
        }
    }

    #[tokio::test]
    async fn create_assigns_increasing_positions() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let c = create(&pool, new("c")).await.unwrap();
        assert_eq!(a.position, 1);
        assert_eq!(b.position, 2);
        assert_eq!(c.position, 3);
        assert_eq!(a.status, Status::Todo);
    }

    #[tokio::test]
    async fn list_filters_by_status_set() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let _b = create(&pool, new("b")).await.unwrap();
        let c = create(&pool, new("c")).await.unwrap();
        set_status(&pool, a.id, Status::Done).await.unwrap();
        set_status(&pool, c.id, Status::Cancelled).await.unwrap();

        let active = list(&pool, &[Status::Todo, Status::InProgress])
            .await
            .unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "b");

        let all = list(
            &pool,
            &[
                Status::Todo,
                Status::InProgress,
                Status::Done,
                Status::Cancelled,
            ],
        )
        .await
        .unwrap();
        assert_eq!(all.len(), 3);

        let only_done = list(&pool, &[Status::Done]).await.unwrap();
        assert_eq!(only_done.len(), 1);
        assert_eq!(only_done[0].title, "a");

        let none = list(&pool, &[]).await.unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn update_changes_fields() {
        let pool = pool().await;
        let t = create(&pool, new("old")).await.unwrap();
        let updated = update(
            &pool,
            t.id,
            UpdateTask {
                title: "new".into(),
                description: Some("desc".into()),
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(updated.title, "new");
        assert_eq!(updated.description.as_deref(), Some("desc"));
    }

    #[tokio::test]
    async fn set_status_cycles_through_all_states() {
        let pool = pool().await;
        let t = create(&pool, new("a")).await.unwrap();
        for s in [
            Status::InProgress,
            Status::Done,
            Status::Cancelled,
            Status::Todo,
        ] {
            let updated = set_status(&pool, t.id, s).await.unwrap().unwrap();
            assert_eq!(updated.status, s);
        }
    }

    #[tokio::test]
    async fn status_round_trips_through_each_variant() {
        for s in [
            Status::Todo,
            Status::InProgress,
            Status::Done,
            Status::Cancelled,
        ] {
            assert_eq!(Status::parse(s.as_str()).unwrap(), s);
        }
    }

    #[tokio::test]
    async fn reorder_rewrites_visible_ids_only() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let c = create(&pool, new("c")).await.unwrap();
        let d = create(&pool, new("d")).await.unwrap();
        set_status(&pool, b.id, Status::Done).await.unwrap();

        let n = reorder(&pool, &[d.id, a.id, c.id]).await.unwrap();
        assert_eq!(n, 3);

        let visible = list(&pool, &[Status::Todo, Status::InProgress])
            .await
            .unwrap();
        let visible_titles: Vec<_> = visible.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(visible_titles, vec!["d", "a", "c"]);

        let all = list(
            &pool,
            &[
                Status::Todo,
                Status::InProgress,
                Status::Done,
                Status::Cancelled,
            ],
        )
        .await
        .unwrap();
        let b_after = all.iter().find(|t| t.id == b.id).unwrap();
        assert_eq!(b_after.position, b.position);
    }

    #[tokio::test]
    async fn reorder_keeps_position_uniqueness() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let c = create(&pool, new("c")).await.unwrap();

        reorder(&pool, &[c.id, b.id, a.id]).await.unwrap();
        let all = list(
            &pool,
            &[
                Status::Todo,
                Status::InProgress,
                Status::Done,
                Status::Cancelled,
            ],
        )
        .await
        .unwrap();
        let mut positions: Vec<i64> = all.iter().map(|t| t.position).collect();
        positions.sort_unstable();
        positions.dedup();
        assert_eq!(positions.len(), 3);
    }

    #[tokio::test]
    async fn reorder_ignores_unknown_ids() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let n = reorder(&pool, &[b.id, 9999, a.id]).await.unwrap();
        assert_eq!(n, 2);
        let visible = list(&pool, &[Status::Todo, Status::InProgress])
            .await
            .unwrap();
        let titles: Vec<_> = visible.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["b", "a"]);
    }

    #[tokio::test]
    async fn reorder_skips_terminal_rows() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let c = create(&pool, new("c")).await.unwrap();
        set_status(&pool, b.id, Status::Done).await.unwrap();
        set_status(&pool, c.id, Status::Cancelled).await.unwrap();

        let n = reorder(&pool, &[c.id, b.id, a.id]).await.unwrap();
        assert_eq!(n, 1);

        let all = list(
            &pool,
            &[
                Status::Todo,
                Status::InProgress,
                Status::Done,
                Status::Cancelled,
            ],
        )
        .await
        .unwrap();
        let a_after = all.iter().find(|t| t.id == a.id).unwrap();
        let b_after = all.iter().find(|t| t.id == b.id).unwrap();
        let c_after = all.iter().find(|t| t.id == c.id).unwrap();
        assert_eq!(a_after.position, a.position);
        assert_eq!(b_after.position, b.position);
        assert_eq!(c_after.position, c.position);
    }

    #[tokio::test]
    async fn updated_at_tracks_edits_but_not_reorders() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let b = create(&pool, new("b")).await.unwrap();
        let initial = a.updated_at;

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let edited = update(
            &pool,
            a.id,
            UpdateTask {
                title: "renamed".into(),
                description: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            edited.updated_at > initial,
            "title edit should bump updated_at"
        );

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let restatuses = set_status(&pool, a.id, Status::InProgress)
            .await
            .unwrap()
            .unwrap();
        assert!(
            restatuses.updated_at > edited.updated_at,
            "status change should bump updated_at"
        );

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        reorder(&pool, &[b.id, a.id]).await.unwrap();
        let a_after_reorder = get(&pool, a.id).await.unwrap().unwrap();
        assert_eq!(
            a_after_reorder.updated_at, restatuses.updated_at,
            "reorder must not bump updated_at"
        );
    }

    #[tokio::test]
    async fn delete_removes_row() {
        let pool = pool().await;
        let t = create(&pool, new("a")).await.unwrap();
        assert!(delete(&pool, t.id).await.unwrap());
        assert!(get(&pool, t.id).await.unwrap().is_none());
        assert!(!delete(&pool, t.id).await.unwrap());
    }
}
