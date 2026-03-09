// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! HTTP API handlers and router construction.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;

use crate::load_test::{LoadTestConfig, LoadTestMetrics};
use crate::redis_browser;
use crate::state::{AppState, LoadTestHandle};

/// Build the full axum router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(serve_dashboard))
        .route("/api/connect", post(connect))
        .route("/api/keys", get(scan_keys))
        .route("/api/keys", delete(flush_db))
        .route("/api/key/{key}", get(get_key))
        .route("/api/key/{key}", delete(delete_key))
        .route("/api/insert", post(insert_key))
        .route("/api/load-test", post(start_load_test))
        .route("/api/load-test/stream", get(load_test_stream))
        .route("/api/load-test/stop", post(stop_load_test))
        .with_state(state)
}

// ── Error type ──────────────────────────────────────────────────────────

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

impl From<redis::RedisError> for ApiError {
    fn from(e: redis::RedisError) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

fn no_connection() -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "Not connected to Redis. POST /api/connect first.".to_string(),
    )
}

// ── Handlers ────────────────────────────────────────────────────────────

async fn serve_dashboard() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-cache, no-store, must-revalidate")],
        axum::response::Html(include_str!("dashboard.html")),
    )
}

#[derive(Deserialize)]
struct ConnectRequest {
    redis_url: String,
}

async fn connect(
    State(state): State<AppState>,
    Json(body): Json<ConnectRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let client = redis::Client::open(body.redis_url.as_str())
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("Invalid URL: {e}")))?;

    let conn = redis::aio::ConnectionManager::new(client)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_GATEWAY, format!("Connection failed: {e}")))?;

    state.set_connection(conn).await;

    Ok(Json(
        serde_json::json!({ "status": "connected", "url": body.redis_url }),
    ))
}

#[derive(Deserialize)]
struct ScanParams {
    #[serde(default = "default_pattern")]
    pattern: String,
    #[serde(default)]
    cursor: u64,
    #[serde(default = "default_count")]
    count: usize,
}

fn default_pattern() -> String {
    "*".to_string()
}
fn default_count() -> usize {
    100
}

async fn scan_keys(
    State(state): State<AppState>,
    Query(params): Query<ScanParams>,
) -> Result<Json<redis_browser::ScanResult>, ApiError> {
    let mut conn = state.connection().await.ok_or_else(no_connection)?;
    let result =
        redis_browser::scan_keys(&mut conn, &params.pattern, params.cursor, params.count).await?;
    Ok(Json(result))
}

async fn get_key(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<redis_browser::KeyDetail>, ApiError> {
    let mut conn = state.connection().await.ok_or_else(no_connection)?;
    let detail = redis_browser::get_key_detail(&mut conn, &key).await?;
    Ok(Json(detail))
}

async fn delete_key(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut conn = state.connection().await.ok_or_else(no_connection)?;
    let deleted: i64 = redis::AsyncCommands::del(&mut conn, &key).await?;
    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

async fn flush_db(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut conn = state.connection().await.ok_or_else(no_connection)?;
    redis::cmd("FLUSHDB")
        .query_async::<()>(&mut conn)
        .await?;
    Ok(Json(serde_json::json!({ "status": "flushed" })))
}

#[derive(Deserialize)]
struct InsertRequest {
    key: String,
    value: String,
    #[serde(default)]
    ttl_secs: Option<u64>,
}

async fn insert_key(
    State(state): State<AppState>,
    Json(body): Json<InsertRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut conn = state.connection().await.ok_or_else(no_connection)?;

    if let Some(ttl) = body.ttl_secs.filter(|&t| t > 0) {
        redis::AsyncCommands::set_ex::<_, _, ()>(&mut conn, &body.key, &body.value, ttl).await?;
    } else {
        redis::AsyncCommands::set::<_, _, ()>(&mut conn, &body.key, &body.value).await?;
    }

    Ok(Json(serde_json::json!({ "status": "inserted", "key": body.key })))
}

async fn start_load_test(
    State(state): State<AppState>,
    Json(config): Json<LoadTestConfig>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let conn = state.connection().await.ok_or_else(no_connection)?;

    let stop_flag = Arc::new(AtomicBool::new(false));
    let metrics_tx = state.metrics_tx().clone();

    let join_handle =
        crate::load_test::start(config.clone(), conn, metrics_tx, Arc::clone(&stop_flag));

    let handle = LoadTestHandle {
        join_handle,
        stop_flag,
    };

    if !state.set_load_test(handle).await {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A load test is already running. Stop it first.".to_string(),
        ));
    }

    Ok(Json(serde_json::json!({ "status": "started" })))
}

async fn stop_load_test(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if state.stop_load_test().await {
        Ok(Json(serde_json::json!({ "status": "stopped" })))
    } else {
        Err(ApiError(
            StatusCode::NOT_FOUND,
            "No load test is running.".to_string(),
        ))
    }
}

async fn load_test_stream(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.metrics_rx();
    let stream = WatchStream::new(rx).map(|metrics: Option<LoadTestMetrics>| {
        let data = match metrics {
            Some(m) => serde_json::to_string(&m).unwrap_or_default(),
            None => r#"{"running":false}"#.to_string(),
        };
        Ok(Event::default().data(data))
    });

    Sse::new(stream)
}
