use anyhow::Result;
use axum::Router;
use axum::routing::{get, post};
use sqlx::SqlitePool;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_governor::GovernorLayer;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod api;
mod auth;
mod config;
mod db;
mod domain;
mod web;

/// Shared application state injected into handlers.
#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: Arc<config::Config>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cfg = config::Config::from_env()?;
    let _log_guard = init_tracing(cfg.log_dir.as_deref())?;

    tracing::info!("starting zeptodo");
    let pool = db::init(&cfg.database_url).await?;
    domain::credentials::reconcile(&pool, &cfg).await?;

    let session_layer = auth::session::build_layer(pool.clone(), &cfg).await?;

    let state = AppState {
        pool,
        config: Arc::new(cfg.clone()),
    };

    let api_rate_limit_config = web::rate_limit::api_config();
    let api_rate_limit_layer: GovernorLayer<_, _, axum::body::Body> =
        GovernorLayer::new(api_rate_limit_config).error_handler(web::rate_limit::api_error);

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
        ))
        .layer(api_rate_limit_layer);

    let login_rate_limit_config = web::rate_limit::login_config();
    let login_rate_limit_layer: GovernorLayer<_, _, axum::body::Body> =
        GovernorLayer::new(login_rate_limit_config).error_handler(web::rate_limit::html_error);

    let cookies_secure = cfg.cookies_secure();

    let login_routes = Router::new()
        .route(
            "/login",
            get(web::login::get_login).post(web::login::post_login),
        )
        .layer(login_rate_limit_layer);

    let app = Router::new()
        .route("/", get(web::tasks::dashboard))
        .route("/healthz", get(web::routes::healthz))
        .merge(login_routes)
        .route("/logout", post(web::login::post_logout))
        .route("/tasks", post(web::tasks::create_task))
        .route("/tasks/{id}", post(web::tasks::update_task))
        .route("/tasks/{id}/status", post(web::tasks::set_status))
        .route("/tasks/{id}/delete", post(web::tasks::delete_task))
        .route("/tasks/reorder", post(web::tasks::reorder_tasks))
        .route(
            "/tasks/show-terminal",
            post(web::tasks::toggle_show_terminal),
        )
        .route(
            "/api-keys",
            get(web::api_keys::index).post(web::api_keys::create),
        )
        .route(
            "/api-keys/{id}/edit_expiry",
            post(web::api_keys::edit_expiry),
        )
        .route(
            "/api-keys/{id}/edit_description",
            post(web::api_keys::edit_description),
        )
        .route("/api-keys/{id}/delete", post(web::api_keys::delete))
        .route("/theme/toggle", post(web::theme::toggle))
        .nest("/api/v1", api_v1)
        .nest_service("/static", ServeDir::new("static"))
        .layer(TraceLayer::new_for_http())
        .layer(session_layer)
        .layer({
            let cfg = web::rate_limit::global_config();
            let layer: GovernorLayer<_, _, axum::body::Body> =
                GovernorLayer::new(cfg).error_handler(web::rate_limit::html_error);
            layer
        })
        .layer(web::security::csp_layer())
        .layer(web::security::nosniff_layer())
        .layer(web::security::frame_options_layer())
        .layer(web::security::referrer_policy_layer())
        .layer(web::security::permissions_policy_layer())
        .layer(tower::util::option_layer(web::security::hsts_layer(
            cookies_secure,
        )))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.bind_addr).await?;
    let addr = listener.local_addr()?;
    tracing::info!(%addr, "listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// Install a JSON `tracing` subscriber to stdout, optionally tee'd to a daily log file.
///
/// ### Arguments
/// - `log_dir`: Directory to write rotated log files into. When `None`, only stdout is used.
///
/// ### Returns
/// - `Ok(Some(guard))`: Stdout plus file appender installed; guard keeps the writer alive.
/// - `Ok(None)`: Stdout-only subscriber installed.
/// - `Err`: Building the rolling appender or installing the subscriber failed.
fn init_tracing(
    log_dir: Option<&str>,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    use tracing_subscriber::{EnvFilter, Registry, fmt};

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn"));
    let stdout_layer = fmt::layer().json().with_current_span(false);

    if let Some(dir) = log_dir {
        let file_appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .max_log_files(7)
            .filename_prefix("zeptodo")
            .filename_suffix("log")
            .build(dir)?;
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let file_layer = fmt::layer()
            .json()
            .with_current_span(false)
            .with_writer(non_blocking);
        Registry::default()
            .with(env_filter)
            .with(stdout_layer)
            .with(file_layer)
            .try_init()?;
        Ok(Some(guard))
    } else {
        Registry::default()
            .with(env_filter)
            .with(stdout_layer)
            .try_init()?;
        Ok(None)
    }
}
