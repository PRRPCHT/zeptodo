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
#[derive(Debug, Clone)]
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

/// List tasks in display order.
///
/// ### Arguments
/// - `pool`: SQLite pool with migrations applied.
/// - `include_terminal`: When `true`, `Done` and `Cancelled` rows are
///   included. When `false`, only `Todo` and `InProgress` rows are returned.
///
/// ### Returns
/// - `Ok(Vec<Task>)`: Tasks ordered by `position ASC`.
/// - `Err`: SQLite query failed.
pub async fn list(pool: &SqlitePool, include_terminal: bool) -> Result<Vec<Task>> {
    let sql = if include_terminal {
        format!("SELECT {SELECT_COLUMNS} FROM tasks ORDER BY position ASC")
    } else {
        format!(
            "SELECT {SELECT_COLUMNS} FROM tasks \
             WHERE status IN ('todo', 'in_progress') ORDER BY position ASC"
        )
    };
    sqlx::query_as::<_, Task>(&sql)
        .fetch_all(pool)
        .await
        .context("listing tasks")
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
    let now = Utc::now();
    let rows =
        sqlx::query("UPDATE tasks SET title = ?, description = ?, updated_at = ? WHERE id = ?")
            .bind(&dto.title)
            .bind(&dto.description)
            .bind(now)
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
    let now = Utc::now();
    let rows = sqlx::query("UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?")
        .bind(status.as_str())
        .bind(now)
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
    async fn list_filters_terminal_by_default() {
        let pool = pool().await;
        let a = create(&pool, new("a")).await.unwrap();
        let _b = create(&pool, new("b")).await.unwrap();
        set_status(&pool, a.id, Status::Done).await.unwrap();
        let active = list(&pool, false).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "b");
        let all = list(&pool, true).await.unwrap();
        assert_eq!(all.len(), 2);
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
    async fn delete_removes_row() {
        let pool = pool().await;
        let t = create(&pool, new("a")).await.unwrap();
        assert!(delete(&pool, t.id).await.unwrap());
        assert!(get(&pool, t.id).await.unwrap().is_none());
        assert!(!delete(&pool, t.id).await.unwrap());
    }
}
