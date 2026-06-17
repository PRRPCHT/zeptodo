use anyhow::Result;
use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;
use tower_sessions::Session;

use crate::AppState;
use crate::auth::csrf;
use crate::auth::guard::AuthedUser;
use crate::domain::task::{self, NewTask, Status, Task, UpdateTask};
use crate::web::layout::{self, LayoutContext};
use crate::web::markdown;
use crate::web::theme::Theme;

const SESSION_FILTER_KEY: &str = "status_filter";

const DEFAULT_FILTER: [Status; 2] = [Status::Todo, Status::InProgress];

const ICON_TODO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="size-5"><circle cx="12" cy="12" r="9"/></svg>"##;

const ICON_IN_PROGRESS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="size-5"><circle cx="12" cy="12" r="9" opacity="0.35"/><path d="M12 3 A 9 9 0 0 1 21 12"/></svg>"##;

const ICON_DONE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="size-5"><circle cx="12" cy="12" r="9"/><polyline points="8 12.5 11 15.5 16.5 9.5"/></svg>"##;

const ICON_CANCELLED: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="size-5"><circle cx="12" cy="12" r="9"/><line x1="8" y1="8" x2="16" y2="16"/><line x1="16" y1="8" x2="8" y2="16"/></svg>"##;

/// Static description of a status menu option, used by the per-row dropdown.
#[derive(Debug, Clone)]
pub struct StatusOption {
    pub value: &'static str,
    pub label: &'static str,
    pub text_class: &'static str,
    pub icon_svg: &'static str,
}

/// Return the ordered list of statuses shown in the per-row status menu.
///
/// ### Returns
/// - `Vec<StatusOption>`: Four options in the order `todo, in_progress, done, cancelled`.
fn status_options() -> Vec<StatusOption> {
    vec![
        StatusOption {
            value: "todo",
            label: "To do",
            text_class: "text-primary",
            icon_svg: ICON_TODO,
        },
        StatusOption {
            value: "in_progress",
            label: "In progress",
            text_class: "text-info",
            icon_svg: ICON_IN_PROGRESS,
        },
        StatusOption {
            value: "done",
            label: "Done",
            text_class: "text-success",
            icon_svg: ICON_DONE,
        },
        StatusOption {
            value: "cancelled",
            label: "Cancelled",
            text_class: "text-base-content/40",
            icon_svg: ICON_CANCELLED,
        },
    ]
}

/// Template-ready projection of a [`Task`].
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TaskView {
    pub id: i64,
    pub title: String,
    pub description_text: String,
    pub description_html: String,
    pub has_description: bool,
    pub status: &'static str,
    pub status_label: &'static str,
    pub status_text_class: &'static str,
    pub status_icon_svg: &'static str,
    pub is_done: bool,
    pub is_cancelled: bool,
}

/// Build a [`TaskView`] for rendering.
///
/// ### Arguments
/// - `task`: The persisted task.
///
/// ### Returns
/// - `TaskView`: A view object with pre-computed display strings and CSS classes.
fn build_view(task: &Task) -> TaskView {
    let (status_label, status_text_class, status_icon_svg) = match task.status {
        Status::Todo => ("To do", "text-primary", ICON_TODO),
        Status::InProgress => ("In progress", "text-info", ICON_IN_PROGRESS),
        Status::Done => ("Done", "text-success", ICON_DONE),
        Status::Cancelled => ("Cancelled", "text-base-content/40", ICON_CANCELLED),
    };
    let description_text = task.description.clone().unwrap_or_default();
    let has_description = !description_text.trim().is_empty();
    let description_html = if has_description {
        markdown::render(&description_text)
    } else {
        String::new()
    };

    TaskView {
        id: task.id,
        title: task.title.clone(),
        description_text,
        description_html,
        has_description,
        status: task.status.as_str(),
        status_label,
        status_text_class,
        status_icon_svg,
        is_done: task.status == Status::Done,
        is_cancelled: task.status == Status::Cancelled,
    }
}

/// View-model describing the four status checkboxes in the filter UI.
#[derive(Debug, Clone)]
pub struct FilterView {
    pub todo: bool,
    pub in_progress: bool,
    pub done: bool,
    pub cancelled: bool,
}

impl FilterView {
    fn from_statuses(statuses: &[Status]) -> Self {
        FilterView {
            todo: statuses.contains(&Status::Todo),
            in_progress: statuses.contains(&Status::InProgress),
            done: statuses.contains(&Status::Done),
            cancelled: statuses.contains(&Status::Cancelled),
        }
    }
}

#[derive(Template)]
#[template(path = "todos.html")]
struct TodosPage {
    layout: LayoutContext,
    tasks: Vec<TaskView>,
    filter: FilterView,
    counts: task::StatusCounts,
    total: usize,
    oob: bool,
    csrf_token: String,
    status_options: Vec<StatusOption>,
}

#[derive(Template)]
#[template(path = "_task_fragment.html")]
#[allow(dead_code)]
struct TaskListFragment {
    tasks: Vec<TaskView>,
    filter: FilterView,
    counts: task::StatusCounts,
    total: usize,
    oob: bool,
    csrf_token: String,
    status_options: Vec<StatusOption>,
}

/// Render the master task list and create form.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `theme`: Resolved theme for this request.
/// - `session`: tower-sessions session, used for the show-terminal preference and CSRF token.
/// - `user`: Authenticated user supplied by the [`AuthedUser`] extractor.
///
/// ### Returns
/// - `Response`: A 200 HTML page on success, or 500 if the database or template fails.
pub async fn dashboard(
    State(state): State<AppState>,
    theme: Theme,
    session: Session,
    user: AuthedUser,
) -> Response {
    tracing::debug!(username = %user.0, "rendering dashboard");
    let statuses = read_status_filter(&session).await;
    let layout = match layout::build(theme, &session).await {
        Ok(l) => l,
        Err(err) => return internal_error("layout build failed", err),
    };
    let tasks = match load_views(&state, &statuses).await {
        Ok(v) => v,
        Err(err) => return internal_error("loading tasks failed", err),
    };
    let counts = match task::count_by_status(&state.pool).await {
        Ok(c) => c,
        Err(err) => return internal_error("counting tasks failed", err),
    };
    let total = tasks.len();
    let csrf_token = layout.csrf_token.clone();
    render(TodosPage {
        layout,
        tasks,
        filter: FilterView::from_statuses(&statuses),
        counts,
        total,
        oob: false,
        csrf_token,
        status_options: status_options(),
    })
}

/// Decoded body of the `POST /tasks` create form.
#[derive(Debug, Deserialize)]
pub struct CreateForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub title: String,
    pub description: Option<String>,
}

/// Create a new task and respond with the refreshed list.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `form`: Decoded form body.
///
/// ### Returns
/// - `Response`: HTMX fragment with the updated list on `HX-Request`, otherwise
///   a 302 redirect to `/`. 400 on invalid input, 403 on CSRF mismatch.
pub async fn create_task(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let title = form.title.trim().to_owned();
    if title.is_empty() || title.len() > 128 {
        return bad_request("title must be 1 to 128 characters");
    }
    let description = sanitize_optional(form.description.as_deref());

    let dto = NewTask { title, description };
    if let Err(err) = task::create(&state.pool, dto).await {
        return internal_error("creating task failed", err);
    }
    respond_with_list(state, session, headers).await
}

/// Decoded body of the `POST /tasks/{id}` inline-edit form.
#[derive(Debug, Deserialize)]
pub struct UpdateForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub title: String,
    pub description: Option<String>,
}

/// Update an existing task in place.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `id`: Primary key of the task to update.
/// - `form`: Decoded form body.
///
/// ### Returns
/// - `Response`: HTMX fragment with the updated list on `HX-Request`, otherwise
///   a 302 redirect to `/`. 400 on invalid input, 403 on CSRF mismatch, 404
///   when no task matches `id`, 500 on backend errors.
pub async fn update_task(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<UpdateForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let title = form.title.trim().to_owned();
    if title.is_empty() || title.len() > 128 {
        return bad_request("title must be 1 to 128 characters");
    }
    let description = sanitize_optional(form.description.as_deref());

    let dto = UpdateTask { title, description };
    match task::update(&state.pool, id, dto).await {
        Ok(Some(_)) => respond_with_list(state, session, headers).await,
        Ok(None) => not_found(),
        Err(err) => internal_error("updating task failed", err),
    }
}

/// Decoded body of the `POST /tasks/{id}/status` form.
#[derive(Debug, Deserialize)]
pub struct StatusForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub status: String,
}

/// Mutate the status of a task via the per-row dropdown.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `id`: Primary key of the task to update.
/// - `form`: Decoded form body. `status` must be one of `todo`, `in_progress`,
///   `done`, `cancelled`.
///
/// ### Returns
/// - `Response`: HTMX fragment with the updated list on `HX-Request`, otherwise
///   a 302 redirect to `/`. 400 on invalid status, 403 on CSRF mismatch, 404
///   when no task matches `id`, 500 on backend errors.
pub async fn set_status(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<StatusForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let status = match Status::parse(&form.status) {
        Ok(s) => s,
        Err(_) => return bad_request("invalid status"),
    };
    match task::set_status(&state.pool, id, status).await {
        Ok(Some(_)) => respond_with_list(state, session, headers).await,
        Ok(None) => not_found(),
        Err(err) => internal_error("updating status failed", err),
    }
}

/// Decoded body of the `POST /tasks/{id}/delete` form (CSRF token only).
#[derive(Debug, Deserialize)]
pub struct DeleteForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
}

/// Permanently remove a task.
///
/// ### Description
/// Deleting an absent id is treated as a no-op rather than 404, so a double
/// click on the delete button does not surface a confusing error to the user.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `id`: Primary key of the task to delete.
/// - `form`: Decoded form body (CSRF token only).
///
/// ### Returns
/// - `Response`: HTMX fragment with the refreshed list on `HX-Request`,
///   otherwise a 302 redirect to `/`. 403 on CSRF mismatch, 500 on backend
///   errors.
pub async fn delete_task(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<DeleteForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    match task::delete(&state.pool, id).await {
        Ok(_) => respond_with_list(state, session, headers).await,
        Err(err) => internal_error("deleting task failed", err),
    }
}

/// Decoded body of the `POST /tasks/reorder` form.
#[derive(Debug, Deserialize)]
pub struct ReorderForm {
    #[serde(rename = "_csrf")]
    pub csrf: String,
    pub ids: String,
}

/// Rewrite positions to match a new visible ordering produced by drag-and-drop.
///
/// ### Description
/// `ids` is a comma-separated list of task ids in their new visible order.
/// Empty entries are ignored; unparseable ids fail the whole request with 400.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used to validate the CSRF token.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `form`: Decoded form body. `ids` is a comma-separated list of task ids.
///
/// ### Returns
/// - `Response`: HTMX fragment with the updated list on `HX-Request`, otherwise
///   a 302 redirect to `/`. 400 on invalid input, 403 on CSRF mismatch, 500
///   on backend errors.
pub async fn reorder_tasks(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<ReorderForm>,
) -> Response {
    if !csrf_ok(&session, &form.csrf).await {
        return forbidden_csrf();
    }
    let mut ids: Vec<i64> = Vec::new();
    for raw in form.ids.split(',') {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<i64>() {
            Ok(v) => ids.push(v),
            Err(_) => return bad_request("invalid id in reorder list"),
        }
    }
    if let Err(err) = task::reorder(&state.pool, &ids).await {
        return internal_error("reordering tasks failed", err);
    }
    respond_with_list(state, session, headers).await
}

/// Update the session-stored status filter.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, where the preference is stored.
/// - `headers`: Request headers, used to detect HTMX callers.
/// - `form`: Decoded form body as raw key/value pairs.
///
/// ### Returns
/// - `Response`: HTMX fragment with the refreshed list on `HX-Request`,
///   otherwise a 302 redirect to `/`. 403 on CSRF mismatch.
pub async fn set_filter(
    State(state): State<AppState>,
    session: Session,
    headers: HeaderMap,
    Form(form): Form<Vec<(String, String)>>,
) -> Response {
    let mut csrf_token: Option<&str> = None;
    let mut selected: Vec<&'static str> = Vec::new();
    for (key, value) in &form {
        match key.as_str() {
            "_csrf" => csrf_token = Some(value.as_str()),
            "statuses" => {
                if let Ok(status) = Status::parse(value) {
                    let canonical = status.as_str();
                    if !selected.contains(&canonical) {
                        selected.push(canonical);
                    }
                }
            }
            _ => {}
        }
    }
    if !csrf_ok(&session, csrf_token.unwrap_or("")).await {
        return forbidden_csrf();
    }
    if let Err(err) = session.insert(SESSION_FILTER_KEY, &selected).await {
        tracing::error!(error = %err, "storing status filter failed");
    }
    respond_with_list(state, session, headers).await
}

/// Read the status filter from the session, defaulting to `todo + in_progress`.
///
/// ### Arguments
/// - `session`: tower-sessions session.
///
/// ### Returns
/// - `Vec<Status>`: The active filter set.
async fn read_status_filter(session: &Session) -> Vec<Status> {
    match session.get::<Vec<String>>(SESSION_FILTER_KEY).await {
        Ok(Some(raw)) => raw.iter().filter_map(|s| Status::parse(s).ok()).collect(),
        _ => DEFAULT_FILTER.to_vec(),
    }
}

/// Verify a submitted CSRF token, returning `false` on any error path.
///
/// ### Arguments
/// - `session`: tower-sessions session holding the canonical token.
/// - `submitted`: The value submitted in the form's `_csrf` field.
///
/// ### Returns
/// - `bool`: `true` when the submitted token matches the session token,
///   `false` on mismatch or on any error reading the session.
async fn csrf_ok(session: &Session, submitted: &str) -> bool {
    csrf::verify(session, submitted).await.unwrap_or(false)
}

/// Load the current task list and project each row to a [`TaskView`].
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `statuses`: Statuses to include in the result. Empty means no rows.
///
/// ### Returns
/// - `Ok(Vec<TaskView>)`: Tasks projected for template rendering.
/// - `Err`: The underlying repo call failed.
async fn load_views(state: &AppState, statuses: &[Status]) -> Result<Vec<TaskView>> {
    let tasks = task::list(&state.pool, statuses).await?;
    Ok(tasks.iter().map(build_view).collect())
}

/// Build the post-mutation response: HTMX fragment or full-page redirect.
///
/// ### Arguments
/// - `state`: Shared application state.
/// - `session`: tower-sessions session, used for the show-terminal preference
///   and to mint the CSRF token used by the rendered fragment.
/// - `headers`: Request headers, used to detect HTMX callers.
///
/// ### Returns
/// - `Response`: HTMX fragment with the refreshed list on `HX-Request`,
///   otherwise a 302 redirect to `/`. 500 on backend errors.
async fn respond_with_list(state: AppState, session: Session, headers: HeaderMap) -> Response {
    if is_htmx(&headers) {
        let statuses = read_status_filter(&session).await;
        let csrf_token = match csrf::token(&session).await {
            Ok(t) => t,
            Err(err) => return internal_error("csrf token failed", err),
        };
        let tasks = match load_views(&state, &statuses).await {
            Ok(v) => v,
            Err(err) => return internal_error("loading tasks failed", err),
        };
        let counts = match task::count_by_status(&state.pool).await {
            Ok(c) => c,
            Err(err) => return internal_error("counting tasks failed", err),
        };
        let total = tasks.len();
        render(TaskListFragment {
            tasks,
            filter: FilterView::from_statuses(&statuses),
            counts,
            total,
            oob: true,
            csrf_token,
            status_options: status_options(),
        })
    } else {
        Redirect::to("/").into_response()
    }
}

/// Detect whether the current request was issued by HTMX.
///
/// ### Arguments
/// - `headers`: Request headers.
///
/// ### Returns
/// - `bool`: `true` when the `HX-Request` header is present.
fn is_htmx(headers: &HeaderMap) -> bool {
    headers.get("HX-Request").is_some()
}

/// Normalise an optional form field, returning `None` for whitespace-only input.
///
/// ### Arguments
/// - `input`: The raw value from the form, if any.
///
/// ### Returns
/// - `Some(String)`: The input is non-empty after trimming.
/// - `None`: The input was missing, empty, or whitespace-only.
fn sanitize_optional(input: Option<&str>) -> Option<String> {
    input.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Render an Askama template into an HTML response.
///
/// ### Arguments
/// - `template`: Any value implementing [`Template`].
///
/// ### Returns
/// - `Response`: A 200 HTML response on success, or 500 with a generic message
///   when the template renderer returned an error.
fn render<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(body) => Html(body).into_response(),
        Err(err) => {
            tracing::error!(error = %err, "template render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}

/// Build the 403 response used when the CSRF token fails to verify.
///
/// ### Returns
/// - `Response`: A 403 with a generic plain-text body.
fn forbidden_csrf() -> Response {
    (StatusCode::FORBIDDEN, "invalid CSRF token").into_response()
}

/// Build a 400 response with a static message body.
///
/// ### Arguments
/// - `message`: The body to send back to the client.
///
/// ### Returns
/// - `Response`: A 400 with the supplied plain-text body.
fn bad_request(message: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, message).into_response()
}

/// Build the 404 response used when a task id does not exist.
///
/// ### Returns
/// - `Response`: A 404 with a generic plain-text body.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "task not found").into_response()
}

/// Log a backend error and build the 500 response.
///
/// ### Arguments
/// - `context`: Static description of the failed step, used in the log line.
/// - `err`: The error that caused the failure.
///
/// ### Returns
/// - `Response`: A 500 with a generic plain-text body. The detailed error is
///   logged, never sent to the client.
fn internal_error(context: &'static str, err: anyhow::Error) -> Response {
    tracing::error!(error = %err, "{context}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_view_from_default_marks_only_active_states() {
        let view = FilterView::from_statuses(&DEFAULT_FILTER);
        assert!(view.todo);
        assert!(view.in_progress);
        assert!(!view.done);
        assert!(!view.cancelled);
    }

    #[test]
    fn filter_view_from_empty_unchecks_everything() {
        let view = FilterView::from_statuses(&[]);
        assert!(!view.todo);
        assert!(!view.in_progress);
        assert!(!view.done);
        assert!(!view.cancelled);
    }

    #[test]
    fn filter_view_from_all_checks_everything() {
        let view = FilterView::from_statuses(&[
            Status::Todo,
            Status::InProgress,
            Status::Done,
            Status::Cancelled,
        ]);
        assert!(view.todo);
        assert!(view.in_progress);
        assert!(view.done);
        assert!(view.cancelled);
    }

    #[test]
    fn filter_view_from_partial_subset_only_marks_those_states() {
        let view = FilterView::from_statuses(&[Status::Done, Status::Cancelled]);
        assert!(!view.todo);
        assert!(!view.in_progress);
        assert!(view.done);
        assert!(view.cancelled);
    }
}
