use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::AppState;
use crate::api::errors::ApiError;
use crate::domain::task::{self, NewTask, Status, Task, UpdateTask};

const MAX_TITLE_LEN: usize = 128;
const MAX_DESCRIPTION_LEN: usize = 16 * 1024;

/// Query string for `GET /api/v1/tasks`.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub include_terminal: bool,
}

/// JSON body for `POST /api/v1/tasks`.
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// JSON body for `PUT /api/v1/tasks/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// JSON body for `POST /api/v1/tasks/{id}/status`.
#[derive(Debug, Deserialize)]
pub struct StatusBody {
    pub status: Status,
}

/// JSON body for `POST /api/v1/tasks/reorder`.
#[derive(Debug, Deserialize)]
pub struct ReorderBody {
    pub ids: Vec<i64>,
}

/// List tasks in display order.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `query`: `include_terminal=true` to include `Done` and `Cancelled` rows.
///
/// ### Returns
/// - `Ok(Json<Vec<Task>>)`: Tasks ordered by `position ASC`.
/// - `Err(ApiError)`: SQLite query failed.
pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Task>>, ApiError> {
    let statuses: &[Status] = if query.include_terminal {
        &[
            Status::Todo,
            Status::InProgress,
            Status::Done,
            Status::Cancelled,
        ]
    } else {
        &[Status::Todo, Status::InProgress]
    };
    let tasks = task::list(&state.pool, statuses)
        .await
        .map_err(|e| ApiError::internal("listing tasks failed", e))?;
    Ok(Json(tasks))
}

/// Fetch a single task by id.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `id`: Primary key of the task.
///
/// ### Returns
/// - `Ok(Json<Task>)`: The task.
/// - `Err(ApiError)`: 404 when missing, 500 on backend errors.
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Task>, ApiError> {
    let task = task::get(&state.pool, id)
        .await
        .map_err(|e| ApiError::internal("loading task failed", e))?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    Ok(Json(task))
}

/// Create a new task.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `body`: JSON body with `title` and optional `description`.
///
/// ### Returns
/// - `Ok((StatusCode, Json<Task>))`: 201 Created with the persisted task.
/// - `Err(ApiError)`: 400 on invalid input, 500 on backend errors.
pub async fn create(
    State(state): State<AppState>,
    body: Result<Json<CreateBody>, JsonRejection>,
) -> Result<(StatusCode, Json<Task>), ApiError> {
    let Json(body) = body.map_err(json_rejection)?;
    let title = trim_required_title(&body.title)?;
    let description = sanitize_description(body.description.as_deref())?;
    let task = task::create(&state.pool, NewTask { title, description })
        .await
        .map_err(|e| ApiError::internal("creating task failed", e))?;
    Ok((StatusCode::CREATED, Json(task)))
}

/// Update the editable fields of a task.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `id`: Primary key of the task.
/// - `body`: JSON body with `title` and optional `description`.
///
/// ### Returns
/// - `Ok(Json<Task>)`: The updated task.
/// - `Err(ApiError)`: 400 on invalid input, 404 when missing, 500 on backend errors.
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Result<Json<UpdateBody>, JsonRejection>,
) -> Result<Json<Task>, ApiError> {
    let Json(body) = body.map_err(json_rejection)?;
    let title = trim_required_title(&body.title)?;
    let description = sanitize_description(body.description.as_deref())?;
    let task = task::update(&state.pool, id, UpdateTask { title, description })
        .await
        .map_err(|e| ApiError::internal("updating task failed", e))?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    Ok(Json(task))
}

/// Change the status of a task.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `id`: Primary key of the task.
/// - `body`: JSON body with a `status` matching one of the closed enum values.
///
/// ### Returns
/// - `Ok(Json<Task>)`: The updated task.
/// - `Err(ApiError)`: 400 on invalid status, 404 when missing, 500 on backend errors.
pub async fn set_status(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    body: Result<Json<StatusBody>, JsonRejection>,
) -> Result<Json<Task>, ApiError> {
    let Json(body) = body.map_err(json_rejection)?;
    let task = task::set_status(&state.pool, id, body.status)
        .await
        .map_err(|e| ApiError::internal("updating task status failed", e))?
        .ok_or_else(|| ApiError::not_found("task not found"))?;
    Ok(Json(task))
}

/// Permanently delete a task.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `id`: Primary key of the task.
///
/// ### Returns
/// - `Ok(StatusCode)`: 204 No Content (idempotent; absent rows still respond 204).
/// - `Err(ApiError)`: 500 on backend errors.
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    task::delete(&state.pool, id)
        .await
        .map_err(|e| ApiError::internal("deleting task failed", e))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reorder tasks to match the supplied visible ordering.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `body`: JSON body with `ids` listed in the new display order.
///
/// ### Returns
/// - `Ok(Json<ReorderResult>)`: Number of rows whose positions were rewritten.
/// - `Err(ApiError)`: 400 on bad JSON, 500 on backend errors.
pub async fn reorder(
    State(state): State<AppState>,
    body: Result<Json<ReorderBody>, JsonRejection>,
) -> Result<Json<ReorderResult>, ApiError> {
    let Json(body) = body.map_err(json_rejection)?;
    let rewritten = task::reorder(&state.pool, &body.ids)
        .await
        .map_err(|e| ApiError::internal("reordering tasks failed", e))?;
    Ok(Json(ReorderResult {
        rewritten: rewritten as u64,
    }))
}

/// Response body for `POST /api/v1/tasks/reorder`.
#[derive(Debug, serde::Serialize)]
pub struct ReorderResult {
    pub rewritten: u64,
}

/// Convert a [`JsonRejection`] into a JSON 400 envelope.
///
/// ### Arguments
/// - `rejection`: The rejection emitted by Axum's JSON extractor.
///
/// ### Returns
/// - `ApiError`: A `bad_request` envelope carrying the rejection message.
fn json_rejection(rejection: JsonRejection) -> ApiError {
    ApiError::bad_request(rejection.body_text())
}

/// Trim and validate a required title field.
///
/// ### Arguments
/// - `raw`: The submitted title.
///
/// ### Returns
/// - `Ok(String)`: A trimmed title within 1..=128 bytes.
/// - `Err(ApiError)`: 400 when empty or too long.
fn trim_required_title(raw: &str) -> Result<String, ApiError> {
    let trimmed = raw.trim().to_owned();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("title must not be empty"));
    }
    if trimmed.len() > MAX_TITLE_LEN {
        return Err(ApiError::bad_request(format!(
            "title must be at most {MAX_TITLE_LEN} characters"
        )));
    }
    Ok(trimmed)
}

/// Normalise and validate an optional description.
///
/// ### Arguments
/// - `raw`: The submitted description, if any.
///
/// ### Returns
/// - `Ok(Some(String))`: A trimmed non-empty description within the size limit.
/// - `Ok(None)`: Missing, empty, or whitespace-only input.
/// - `Err(ApiError)`: 400 when the description exceeds the size limit.
fn sanitize_description(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = raw else { return Ok(None) };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.len() > MAX_DESCRIPTION_LEN {
        return Err(ApiError::bad_request(format!(
            "description must be at most {MAX_DESCRIPTION_LEN} bytes"
        )));
    }
    Ok(Some(trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::{get, post};
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    use crate::AppState;
    use crate::api;
    use crate::config::Config;
    use crate::domain::api_key::{self, ExpiryChoice};
    use crate::domain::task::{self, NewTask, Status};

    async fn build_app() -> (Router, SqlitePool) {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let config = Config {
            bind_addr: "127.0.0.1:0".into(),
            database_url: "sqlite::memory:".into(),
            base_url: "http://localhost".into(),
            session_secret: "0".repeat(64),
            username: None,
            password: None,
            timezone: None,
            log_dir: None,
            behind_proxy: false,
        };
        let state = AppState {
            pool: pool.clone(),
            config: Arc::new(config),
        };
        let api_v1 = Router::new()
            .route("/tasks", get(api::tasks::list).post(api::tasks::create))
            .route(
                "/tasks/{id}",
                get(api::tasks::get)
                    .put(api::tasks::update)
                    .delete(api::tasks::delete),
            )
            .route("/tasks/{id}/status", post(api::tasks::set_status))
            .route("/tasks/reorder", post(api::tasks::reorder))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                api::auth::require_api_key,
            ));
        let app = Router::new().nest("/api/v1", api_v1).with_state(state);
        (app, pool)
    }

    async fn mint_key(pool: &SqlitePool) -> String {
        api_key::create(pool, Some("test".into()), ExpiryChoice::Year1)
            .await
            .unwrap()
            .plaintext
    }

    async fn read_body(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        std::str::from_utf8(&bytes).unwrap().to_owned()
    }

    fn bearer(key: &str) -> String {
        format!("Bearer {key}")
    }

    #[tokio::test]
    async fn list_without_bearer_returns_401() {
        let (app, _pool) = build_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(read_body(resp).await.contains("\"code\":\"unauthorized\""));
    }

    #[tokio::test]
    async fn list_with_non_bearer_scheme_returns_401() {
        let (app, _pool) = build_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, "Basic abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_with_valid_key_serializes_tasks() {
        let (app, pool) = build_app().await;
        task::create(
            &pool,
            NewTask {
                title: "alpha".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let key = mint_key(&pool).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = read_body(resp).await;
        assert!(body.contains("\"alpha\""));
        assert!(body.contains("\"status\":\"todo\""));
    }

    #[tokio::test]
    async fn create_returns_201_and_persists() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"title":"hello","description":"world"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = read_body(resp).await;
        assert!(body.contains("\"hello\""));
        let rows = task::list(&pool, &[Status::Todo, Status::InProgress])
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn create_with_empty_title_returns_400_envelope() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"title":"   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(read_body(resp).await.contains("\"code\":\"bad_request\""));
    }

    #[tokio::test]
    async fn get_unknown_id_returns_404_envelope() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks/9999")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert!(read_body(resp).await.contains("\"code\":\"not_found\""));
    }

    #[tokio::test]
    async fn set_status_changes_persisted_status() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let created = task::create(
            &pool,
            NewTask {
                title: "t".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/tasks/{}/status", created.id))
                    .header(header::AUTHORIZATION, bearer(&key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"done"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let reloaded = task::get(&pool, created.id).await.unwrap().unwrap();
        assert_eq!(reloaded.status, task::Status::Done);
    }

    #[tokio::test]
    async fn delete_is_idempotent() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let created = task::create(
            &pool,
            NewTask {
                title: "t".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let resp = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("DELETE")
                        .uri(format!("/api/v1/tasks/{}", created.id))
                        .header(header::AUTHORIZATION, bearer(&key))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        }
    }

    #[tokio::test]
    async fn reorder_returns_rewritten_count() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let a = task::create(
            &pool,
            NewTask {
                title: "a".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let b = task::create(
            &pool,
            NewTask {
                title: "b".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/tasks/reorder")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"ids":[{},{}]}}"#, b.id, a.id)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(read_body(resp).await.contains("\"rewritten\":2"));
    }

    #[tokio::test]
    async fn include_terminal_param_toggles_filter() {
        let (app, pool) = build_app().await;
        let key = mint_key(&pool).await;
        let a = task::create(
            &pool,
            NewTask {
                title: "active".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let b = task::create(
            &pool,
            NewTask {
                title: "finished".into(),
                description: None,
            },
        )
        .await
        .unwrap();
        let _ = a;
        task::set_status(&pool, b.id, task::Status::Done)
            .await
            .unwrap();

        let default = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let default_body = read_body(default).await;
        assert!(default_body.contains("\"active\""));
        assert!(!default_body.contains("\"finished\""));

        let with_terminal = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks?include_terminal=true")
                    .header(header::AUTHORIZATION, bearer(&key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let all_body = read_body(with_terminal).await;
        assert!(all_body.contains("\"active\""));
        assert!(all_body.contains("\"finished\""));
    }
}
