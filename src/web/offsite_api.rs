//! Encrypted backups in a private channel: settings, a backup now, the list, and restore.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::api::{api_error, api_ok};
use super::AppState;

pub async fn overview(State(state): State<AppState>) -> Response {
    Json(crate::offsite::settings(state.db()).await).into_response()
}

#[derive(Deserialize)]
pub struct SettingsRequest {
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    every_hours: Option<u64>,
    #[serde(default)]
    keep: Option<u64>,
}

pub async fn save(State(state): State<AppState>, Json(body): Json<SettingsRequest>) -> Response {
    match crate::offsite::save_settings(
        state.db(),
        body.password.as_deref(),
        body.every_hours,
        body.keep,
    )
    .await
    {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn run(State(state): State<AppState>) -> Response {
    match crate::offsite::run(&state.services).await {
        Ok(item) => Json(json!({ "ok": true, "backup": item })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn list(State(state): State<AppState>) -> Response {
    match crate::offsite::list(&state.services).await {
        Ok(items) => Json(items).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

#[derive(Deserialize)]
pub struct RestoreRequest {
    id: i32,
    #[serde(default)]
    password: Option<String>,
}

pub async fn restore(State(state): State<AppState>, Json(body): Json<RestoreRequest>) -> Response {
    match crate::offsite::restore(&state.services, body.id, body.password).await {
        Ok(restored) => {
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                crate::restart::restart_now();
            });
            Json(json!({ "ok": true, "restored": restored, "restarting": true })).into_response()
        }
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}
