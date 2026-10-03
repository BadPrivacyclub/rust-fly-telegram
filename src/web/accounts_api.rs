//! Panel API for account management across one or several connected accounts.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::api::{api_error, api_ok};
use super::AppState;
use crate::account_tools::{self, BulkAction, CleanupOptions, DialogKind, ProfileUpdate};

pub async fn list(State(state): State<AppState>) -> Response {
    let runtime_accounts = state.services.runtime.accounts().await;
    let handles = state.services.accounts.list().await;
    let list = handles
        .iter()
        .map(|handle| {
            let runtime = runtime_accounts
                .iter()
                .find(|a| a.session_file == handle.session_file);
            let avatar =
                runtime.and_then(|a| crate::anti_delete::account_avatar_url(&a.account_id));
            json!({
                "id": handle.user_id,
                "avatar": avatar,
                "name": handle.name,
                "session": handle.session_file,
                "connected": runtime.map(|a| a.connected).unwrap_or(false),
                "commands": runtime.map(|a| a.commands_seen).unwrap_or(0),
                "updates": runtime.map(|a| a.updates_seen).unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();
    Json(list).into_response()
}

pub async fn counts(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let Some(handle) = state.services.accounts.get(id).await else {
        return api_error(StatusCode::NOT_FOUND, "account is not connected");
    };
    match account_tools::counts(&handle).await {
        Ok(counts) => Json(counts).into_response(),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, error),
    }
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    #[serde(default)]
    accounts: Vec<i64>,
    options: CleanupOptions,
}

pub async fn cleanup_preview(
    State(state): State<AppState>,
    Json(body): Json<PreviewRequest>,
) -> Response {
    if body.options.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "choose at least one thing to clean",
        );
    }
    let handles = match state.services.accounts.select(&body.accounts).await {
        Ok(handles) => handles,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error),
    };
    let mut previews = Vec::new();
    for handle in handles {
        let plan = match account_tools::plan_cleanup(&handle, &body.options).await {
            Ok(plan) => plan,
            Err(error) => {
                previews.push(json!({ "account_id": handle.user_id, "name": handle.name, "error": error.to_string() }));
                continue;
            }
        };
        let items = plan
            .dialogs
            .iter()
            .map(
                |d| json!({ "id": d.id, "title": d.title, "kind": d.kind, "username": d.username }),
            )
            .collect::<Vec<_>>();
        let contacts = plan
            .contacts
            .iter()
            .map(|c| json!({ "id": c.id, "name": c.name, "username": c.username }))
            .collect::<Vec<_>>();
        let summary = json!({
            "account_id": handle.user_id,
            "name": handle.name,
            "contacts": contacts,
            "items": items,
            "users": plan.count(DialogKind::User),
            "bots": plan.count(DialogKind::Bot),
            "groups": plan.count(DialogKind::Group) + plan.count(DialogKind::Supergroup),
            "channels": plan.count(DialogKind::Channel),
            "kept": plan.kept,
            "owned": plan.owned_skipped,
            "total": plan.total(),
        });
        let code = state
            .services
            .accounts
            .store_plan(handle.user_id, body.options.clone(), plan)
            .await;
        let mut summary = summary;
        summary["code"] = json!(code);
        previews.push(summary);
    }
    Json(previews).into_response()
}

#[derive(Deserialize)]
pub struct RunRequest {
    codes: Vec<String>,
}

pub async fn cleanup_run(State(state): State<AppState>, Json(body): Json<RunRequest>) -> Response {
    let services = &state.services;
    let mut started = Vec::new();
    for code in body.codes {
        let Some((user_id, options, plan)) = services.accounts.take_plan(&code).await else {
            return api_error(StatusCode::BAD_REQUEST, "a preview expired; build it again");
        };
        let Some(handle) = services.accounts.get(user_id).await else {
            continue;
        };
        let job = services
            .jobs
            .start("cleanup", "Account cleanup", &handle.name)
            .await;
        started.push(job.id().to_string());
        services.spawn_job(
            job.clone(),
            account_tools::run_cleanup(handle, plan, options, job),
        );
    }
    Json(json!({ "ok": true, "jobs": started })).into_response()
}

#[derive(Deserialize)]
pub struct BulkRequest {
    #[serde(default)]
    accounts: Vec<i64>,
    action: String,
    #[serde(default)]
    days: Option<u32>,
}

pub async fn bulk(State(state): State<AppState>, Json(body): Json<BulkRequest>) -> Response {
    let Some(action) = BulkAction::parse(&body.action) else {
        return api_error(StatusCode::BAD_REQUEST, "unknown action");
    };
    let services = &state.services;
    let handles = match services.accounts.select(&body.accounts).await {
        Ok(handles) => handles,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, error),
    };
    let mut started = Vec::new();
    for handle in handles {
        let job = services
            .jobs
            .start("bulk", action.title(), &handle.name)
            .await;
        started.push(job.id().to_string());
        services.spawn_job(
            job.clone(),
            account_tools::run_bulk(handle, action, body.days.unwrap_or(30), job),
        );
    }
    Json(json!({ "ok": true, "jobs": started })).into_response()
}

pub async fn profile(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<ProfileUpdate>,
) -> Response {
    let Some(handle) = state.services.accounts.get(id).await else {
        return api_error(StatusCode::NOT_FOUND, "account is not connected");
    };
    match account_tools::update_profile(&handle, &body).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn jobs(State(state): State<AppState>) -> Response {
    Json(state.services.jobs.list().await).into_response()
}

pub async fn cancel_job(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if state.services.jobs.cancel(&id).await {
        api_ok()
    } else {
        api_error(StatusCode::NOT_FOUND, "no running task with that id")
    }
}

pub async fn automations(State(state): State<AppState>) -> Response {
    Json(crate::automations::load_rules(&state.services).await).into_response()
}

pub async fn save_automation(
    State(state): State<AppState>,
    Json(rule): Json<crate::automations::Rule>,
) -> Response {
    match crate::automations::upsert_rule(&state.services, rule).await {
        Ok(rule) => Json(json!({ "ok": true, "rule": rule })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct EnabledRequest {
    enabled: bool,
}

pub async fn toggle_automation(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<EnabledRequest>,
) -> Response {
    match crate::automations::set_enabled(&state.services, &id, body.enabled).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::NOT_FOUND, error),
    }
}

pub async fn delete_automation(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match crate::automations::delete_rule(&state.services, &id).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::NOT_FOUND, error),
    }
}

pub async fn profiles(State(state): State<AppState>) -> Response {
    let db = state.db();
    let assigned = crate::profiles::assignments(db).await;
    let accounts = state
        .services
        .accounts
        .list()
        .await
        .into_iter()
        .map(|handle| {
            let session = crate::client::auth::normalize_session_file(&handle.session_file);
            json!({
                "id": handle.user_id,
                "name": handle.name,
                "session": handle.session_file,
                "profile_id": assigned.get(&session),
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "enabled": crate::profiles::enabled(db).await,
        "profiles": crate::profiles::list(db).await,
        "accounts": accounts,
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct ProfilesUpdate {
    enabled: Option<bool>,
    profile: Option<crate::profiles::DeviceProfile>,
}

pub async fn update_profiles(
    State(state): State<AppState>,
    Json(body): Json<ProfilesUpdate>,
) -> Response {
    let db = state.db();
    if let Some(enabled) = body.enabled {
        if let Err(error) = crate::profiles::set_enabled(db, enabled).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    if let Some(profile) = body.profile {
        if let Err(error) = crate::profiles::save(db, profile).await {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }
    api_ok()
}

pub async fn delete_profile(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match crate::profiles::delete(state.db(), &id).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

#[derive(Deserialize)]
pub struct AssignRequest {
    /// Account to change; empty with `generate` set means every account without a profile.
    account: Option<i64>,
    /// Profile to pin; `None` unpins unless `generate` is set.
    profile_id: Option<String>,
    #[serde(default)]
    generate: bool,
}

pub async fn assign_profile(
    State(state): State<AppState>,
    Json(body): Json<AssignRequest>,
) -> Response {
    let db = state.db();
    let handles = match body.account {
        Some(id) => match state.services.accounts.get(id).await {
            Some(handle) => vec![handle],
            None => return api_error(StatusCode::NOT_FOUND, "account is not connected"),
        },
        None if body.generate => {
            let assigned = crate::profiles::assignments(db).await;
            state
                .services
                .accounts
                .list()
                .await
                .into_iter()
                .filter(|handle| {
                    let session = crate::client::auth::normalize_session_file(&handle.session_file);
                    !assigned.contains_key(&session)
                })
                .collect()
        }
        None => return api_error(StatusCode::BAD_REQUEST, "choose an account"),
    };
    for handle in handles {
        let result = if body.generate {
            crate::profiles::assign_generated(db, &handle.session_file)
                .await
                .map(|_| ())
        } else {
            crate::profiles::assign(db, &handle.session_file, body.profile_id.as_deref()).await
        };
        if let Err(error) = result {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }
    api_ok()
}
