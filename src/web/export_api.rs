use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::api::{api_error, api_ok};
use super::AppState;
use crate::export::{self, rules, ExportRequest};

pub async fn overview(State(state): State<AppState>) -> Response {
    let db = &state.services.db;
    Json(json!({
        "exports": export::list().await,
        "takeout_waits": export::takeout_waits(&state.services).await,
        "default_rules": rules::default_rules(db).await,
        "account_rules": rules::account_overrides(db).await,
    }))
    .into_response()
}

pub async fn chats(State(state): State<AppState>, Path(account_id): Path<i64>) -> Response {
    let Some(handle) = state.services.accounts.get(account_id).await else {
        return api_error(StatusCode::NOT_FOUND, "account is not connected");
    };
    match crate::account_tools::dialogs(&handle).await {
        Ok(dialogs) => Json(
            dialogs
                .iter()
                .map(|d| {
                    json!({
                        "id": d.id,
                        "title": d.title,
                        "kind": d.kind,
                        "username": d.username,
                        "archived": d.archived,
                        "last_date": d.last_date,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, error),
    }
}

pub async fn start(State(state): State<AppState>, Json(request): Json<ExportRequest>) -> Response {
    match export::start(&state.services, request).await {
        Ok(jobs) => Json(json!({ "ok": true, "jobs": jobs })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn save_default_rules(
    State(state): State<AppState>,
    Json(rules): Json<rules::ExportRules>,
) -> Response {
    match rules::save_default_rules(&state.services.db, &rules).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct OverrideRequest {
    #[serde(default)]
    patch: Option<Value>,
}

pub async fn save_account_rules(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Json(body): Json<OverrideRequest>,
) -> Response {
    match rules::save_account_override(&state.services.db, account_id, body.patch).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn clear_takeout_wait(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
) -> Response {
    match export::clear_takeout_wait(&state.services, account_id).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

pub async fn delete(Path((account_id, id)): Path<(i64, String)>) -> Response {
    match export::delete(account_id, &id).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::NOT_FOUND, error),
    }
}

pub async fn archive(Path((account_id, id)): Path<(i64, String)>) -> Response {
    match export::archive(account_id, &id).await {
        Ok(_) => Json(json!({ "ok": true, "url": format!("/exports/{account_id}/{id}.tar.gz") }))
            .into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

const INLINE_MEDIA: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "mp4", "webm", "mov", "ogg", "oga", "mp3", "m4a", "wav",
];

/// I serve files people sent in chats so they never run in the panel origin: anything in a media
/// folder that is not plain image, video, or audio becomes a download. My own export pages get
/// a policy that allows only their own script and blocks requests to the panel API.
pub async fn sandbox_headers(request: Request<Body>, next: Next) -> Response {
    let path = request.uri().path().to_ascii_lowercase();
    let user_file = path.contains("/media/") || path.contains("/deleted_media/");
    let extension = path
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_string())
        .unwrap_or_default();
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if user_file && !INLINE_MEDIA.contains(&extension.as_str()) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        );
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self'; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
