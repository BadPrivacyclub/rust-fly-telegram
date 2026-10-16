use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::api::{api_error, api_ok};
use super::AppState;
use crate::recovery;

pub async fn overview(State(state): State<AppState>) -> Response {
    let services = &state.services;
    Json(json!({
        "vault": recovery::vault_info().await,
        "default_enabled": recovery::default_enabled(services).await,
        "accounts": recovery::statuses(services).await,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct PasswordBody {
    password: String,
}

pub async fn create_vault(
    State(state): State<AppState>,
    Json(body): Json<PasswordBody>,
) -> Response {
    match recovery::create_vault(&state.services, &body.password).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    current: String,
    next: String,
}

pub async fn change_vault_password(Json(body): Json<ChangePasswordBody>) -> Response {
    match recovery::change_vault_password(&body.current, &body.next).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct DefaultBody {
    enabled: bool,
}

pub async fn set_default(State(state): State<AppState>, Json(body): Json<DefaultBody>) -> Response {
    match recovery::set_default_enabled(&state.services, body.enabled).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

#[derive(Deserialize)]
pub struct ModeBody {
    mode: String,
}

pub async fn set_mode(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Json(body): Json<ModeBody>,
) -> Response {
    match recovery::set_account_mode(&state.services, user_id, &body.mode).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn restore(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Json(body): Json<PasswordBody>,
) -> Response {
    let Some(loader) = state.loader.as_ref() else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "panel is not ready");
    };
    match recovery::restore(&state.services, loader, user_id, &body.password).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn two_factor(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Json(body): Json<PasswordBody>,
) -> Response {
    let Some(loader) = state.loader.as_ref() else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "panel is not ready");
    };
    match recovery::submit_password(&state.services, loader, user_id, &body.password).await {
        Ok(outcome) => Json(outcome).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

#[derive(Deserialize)]
pub struct PurgeBody {
    confirm: String,
    #[serde(default)]
    exports: bool,
}

pub async fn purge(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Json(body): Json<PurgeBody>,
) -> Response {
    if body.confirm != "DELETE" {
        return api_error(StatusCode::BAD_REQUEST, "type DELETE to confirm");
    }
    match recovery::purge(&state.services, user_id, body.exports).await {
        Ok(summary) => Json(json!({ "ok": true, "removed": summary })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn premium(State(state): State<AppState>) -> Response {
    Json(crate::premium::status(&state.services.db).await).into_response()
}

#[derive(Deserialize)]
pub struct PremiumBody {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    pack: Option<String>,
    #[serde(default)]
    icons: Option<std::collections::BTreeMap<String, Option<String>>>,
}

pub async fn save_premium(
    State(state): State<AppState>,
    Json(body): Json<PremiumBody>,
) -> Response {
    let db = &state.services.db;
    let result = async {
        if let Some(mode) = &body.mode {
            crate::premium::set_mode(db, mode).await?;
        }
        let mut matched = None;
        if let Some(pack) = &body.pack {
            let handle = state.services.accounts.list().await.into_iter().next();
            matched = Some(match handle {
                Some(handle) => crate::premium::use_pack(db, &handle.client, pack).await?,
                None if pack.trim().is_empty() => {
                    crate::premium::use_pack_offline_clear(db).await?
                }
                None => anyhow::bail!("connect an account first; I read the pack through it"),
            });
        }
        if let Some(icons) = &body.icons {
            for (name, id) in icons {
                let id = id.as_deref().and_then(|v| v.trim().parse::<i64>().ok());
                crate::premium::set_icon(db, name, id).await?;
            }
        }
        if let Some(loader) = state.loader.as_ref() {
            loader.refresh_premium().await?;
        }
        anyhow::Ok(matched)
    }
    .await;
    match result {
        Ok(matched) => Json(json!({ "ok": true, "matched": matched })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}
