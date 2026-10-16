//! Panel API for device profiles: the generator switch, the operator's own profiles, and the
//! profile of every account.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::api::{api_error, api_ok};
use super::AppState;
use crate::device_profile::{self, Choice, Profile};

fn profile_json(profile: &Profile) -> Value {
    json!({
        "system": profile.system,
        "device": profile.device,
        "app": profile.app,
        "version": profile.version,
        "label": profile.label(),
    })
}

/// What both the dashboard and the add-account wizard need to offer a choice.
async fn choices(state: &AppState) -> Value {
    let db = state.db();
    let custom = device_profile::custom(db)
        .await
        .into_iter()
        .map(|item| {
            let mut value = profile_json(&item.profile);
            value["id"] = json!(item.id);
            value
        })
        .collect::<Vec<_>>();
    json!({
        "enabled": device_profile::enabled(db).await,
        "use_custom": device_profile::use_custom(db).await,
        "default": profile_json(&Profile::fallback()),
        "custom": custom,
    })
}

pub async fn overview(State(state): State<AppState>) -> Response {
    let mut body = choices(&state).await;
    let db = state.db();
    let runtime = state.services.runtime.accounts().await;
    let mut accounts = Vec::new();
    for session in crate::client::auth::session_files(db).await {
        let handle = state.services.accounts.by_session(&session).await;
        let known = runtime.iter().find(|a| a.session_file == session);
        let assigned = device_profile::assigned(db, &session).await;
        let profile = assigned
            .as_ref()
            .map(|a| a.profile.clone())
            .unwrap_or_else(Profile::fallback);
        accounts.push(json!({
            "session": session,
            "id": handle.as_ref().map(|h| h.user_id),
            "name": handle
                .as_ref()
                .map(|h| h.name.clone())
                .or_else(|| known.map(|a| a.name.clone()))
                .unwrap_or_else(|| session.clone()),
            "connected": handle.is_some(),
            "profile": profile_json(&profile),
            "source": assigned.map(|a| a.source),
        }));
    }
    body["accounts"] = Value::Array(accounts);
    Json(body).into_response()
}

/// The add-account wizard gets the choices plus one generated preview.
pub async fn setup_info(State(state): State<AppState>) -> Response {
    let mut body = choices(&state).await;
    body["preview"] = profile_json(&device_profile::generate(state.db()).await);
    Json(body).into_response()
}

pub async fn generate(State(state): State<AppState>) -> Response {
    Json(profile_json(&device_profile::generate(state.db()).await)).into_response()
}

#[derive(Deserialize)]
pub struct OptionsRequest {
    enabled: Option<bool>,
    use_custom: Option<bool>,
}

pub async fn set_options(
    State(state): State<AppState>,
    Json(body): Json<OptionsRequest>,
) -> Response {
    match device_profile::set_options(state.db(), body.enabled, body.use_custom).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

pub async fn add_custom(State(state): State<AppState>, Json(body): Json<Profile>) -> Response {
    match device_profile::add_custom(state.db(), body).await {
        Ok(item) => {
            let mut value = profile_json(&item.profile);
            value["id"] = json!(item.id);
            Json(json!({ "ok": true, "profile": value })).into_response()
        }
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn remove_custom(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match device_profile::remove_custom(state.db(), &id).await {
        Ok(true) => api_ok(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "this profile is no longer in the list"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

#[derive(Deserialize)]
pub struct AssignRequest {
    session: String,
    #[serde(flatten)]
    choice: Choice,
}

/// I change the profile of an existing account; it applies on the next connection.
pub async fn assign(State(state): State<AppState>, Json(body): Json<AssignRequest>) -> Response {
    let db = state.db();
    let known = crate::client::auth::session_files(db).await;
    let session = body.session.trim().replace('\\', "/");
    if !known.iter().any(|s| *s == session) {
        return api_error(StatusCode::NOT_FOUND, "unknown account session");
    }
    let resolved = match device_profile::resolve(db, body.choice).await {
        Ok(resolved) => resolved,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    };
    if let Err(error) = device_profile::store(db, &session, &resolved).await {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
    }
    Json(json!({
        "ok": true,
        "profile": profile_json(&resolved.profile),
        "restart_required": true,
    }))
    .into_response()
}
