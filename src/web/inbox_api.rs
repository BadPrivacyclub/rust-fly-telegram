//! The inbox: notifications for the owner, some with actions to answer from the panel.

use anyhow::Context;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::api::{api_error, api_ok};
use super::AppState;
use crate::i18n::{self, Lang};

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    after: u64,
}

pub async fn page(State(state): State<AppState>, Query(query): Query<PageQuery>) -> Response {
    Json(state.services.notifier.page(query.after)).into_response()
}

#[derive(Deserialize)]
pub struct ReadRequest {
    up_to: u64,
}

pub async fn mark_read(State(state): State<AppState>, Json(body): Json<ReadRequest>) -> Response {
    state.services.notifier.mark_read(body.up_to);
    api_ok()
}

#[derive(Deserialize)]
pub struct ActionRequest {
    event: u64,
    action: String,
}

pub async fn act(State(state): State<AppState>, Json(body): Json<ActionRequest>) -> Response {
    let notifier = &state.services.notifier;
    if notifier.find_action(body.event, &body.action).is_none() {
        return api_error(
            StatusCode::CONFLICT,
            "this notification was already answered or is gone",
        );
    }
    let lang = crate::core_settings::language(state.db()).await;
    match run_action(&state, &body.action, lang).await {
        Ok(outcome) => Json(json!({ "ok": true, "outcome": outcome })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

/// Carries out an inbox action and returns what to show in place of the buttons. Every event
/// offering an action on the same subject is resolved together.
async fn run_action(state: &AppState, action: &str, lang: Lang) -> anyhow::Result<String> {
    let (kind, subject) = action.split_once(':').unwrap_or((action, ""));
    let notifier = &state.services.notifier;
    match kind {
        "pm.allow" | "pm.block" => {
            let user_id = subject.parse::<i64>()?;
            let allow = kind == "pm.allow";
            crate::client::decide_pm(&state.services, user_id, allow).await?;
            let outcome = i18n::tr(
                lang,
                if allow {
                    "bot.pm_allowed"
                } else {
                    "bot.pm_denied"
                },
            );
            notifier.resolve(&format!("pm.allow:{user_id}"), outcome);
            notifier.resolve(&format!("pm.block:{user_id}"), outcome);
            Ok(outcome.to_string())
        }
        "perm.once" | "perm.scope" | "perm.always" | "perm.deny" => {
            let loader = state
                .loader
                .as_ref()
                .context("modules are not loaded yet")?;
            let choice = kind.trim_start_matches("perm.");
            loader.decide_request(subject.parse()?, choice).await
        }
        _ => anyhow::bail!("unknown action {action}"),
    }
}
