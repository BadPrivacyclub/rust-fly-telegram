//! First-run wizard endpoints and account sign-in (also used to add accounts later).

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::api::{api_error, api_ok};
use super::{auth, AppState};
use crate::config::{db_key, DEFAULT_SESSION_FILE, SESSIONS_DIR, SOCKS5_SCHEME};
use crate::core_settings::{self, key};
use crate::i18n::Lang;

pub async fn state(State(state): State<AppState>) -> Json<Value> {
    let db = state.db();
    Json(json!({
        "mode": if state.loader.is_some() { "account" } else { "setup" },
        "has_api": db.get(db_key::API_ID).await.as_str().is_some_and(|v| !v.is_empty()),
        "api_id": db.get(db_key::API_ID).await,
        "proxy_url": db.get(db_key::PROXY_URL).await,
        "desktop_profiles": crate::profiles::enabled(db).await,
        "language": core_settings::language(db).await.code(),
        "prefix": core_settings::prefixes(db).await.into_iter().next(),
        "bot_configured": core_settings::bot_token(db).await.is_some(),
        "panel_password_set": auth::password_is_set(db).await,
        "version": crate::VERSION,
    }))
}

#[derive(Deserialize)]
pub struct PrefsRequest {
    #[serde(default)]
    panel_password: Option<String>,
    #[serde(default)]
    bot_token: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

pub async fn save_prefs(State(state): State<AppState>, Json(body): Json<PrefsRequest>) -> Response {
    let db = state.db();
    if let Some(password) = body.panel_password.as_deref().filter(|p| !p.is_empty()) {
        if let Err(error) = auth::set_password(db, Some(password)).await {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }
    if let Some(token) = body
        .bot_token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        match crate::bot::validate_token(token).await {
            Ok(_) => {
                if let Err(error) = db
                    .set(key::BOT_TOKEN, Value::String(token.to_string()))
                    .await
                {
                    return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
                }
            }
            Err(error) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    format!("Telegram rejected the bot token: {error}"),
                )
            }
        }
    }
    if let Some(prefix) = body.prefix.as_deref().filter(|p| !p.trim().is_empty()) {
        let values = prefix
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if let Err(error) = core_settings::set_prefixes(db, &values).await {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }
    if let Some(language) = body.language.as_deref() {
        if let Err(error) = core_settings::set_language(db, Lang::parse(language)).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    api_ok()
}

#[derive(Deserialize)]
pub struct SendCodeRequest {
    phone: String,
    #[serde(default)]
    api_id: String,
    #[serde(default)]
    api_hash: String,
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    session_name: String,
    /// Absent keeps the saved choice, so older clients behave as before.
    #[serde(default)]
    desktop_profiles: Option<bool>,
}

pub async fn send_code(
    State(state): State<AppState>,
    Json(body): Json<SendCodeRequest>,
) -> Response {
    // Empty API fields reuse the stored application, e.g. when adding a second account.
    let stored = |key: &'static str| {
        let db = state.db();
        async move { db.get(key).await.as_str().unwrap_or_default().to_string() }
    };
    let api_id_text = if body.api_id.trim().is_empty() {
        stored(db_key::API_ID).await
    } else {
        body.api_id.trim().to_string()
    };
    let api_id: i32 = match api_id_text.parse() {
        Ok(v) => v,
        Err(_) => return api_error(StatusCode::BAD_REQUEST, "api_id must be an integer"),
    };
    let api_hash = if body.api_hash.trim().is_empty() {
        stored(db_key::API_HASH).await
    } else {
        body.api_hash.trim().to_string()
    };
    if api_hash.len() != 32 || !api_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "api_hash must be the 32-character value from my.telegram.org",
        );
    }
    let phone = body
        .phone
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect::<String>();
    if phone.len() < 6 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "enter the phone number in international format",
        );
    }
    let proxy_url = body.proxy_url.trim().to_string();
    if !proxy_url.is_empty() && !proxy_url.starts_with(SOCKS5_SCHEME) {
        return api_error(StatusCode::BAD_REQUEST, "proxy must use socks5://");
    }

    let session_file = session_file_from_name(&body.session_name);
    let db = state.db();
    for (key, value) in [
        (db_key::API_ID, api_id.to_string()),
        (db_key::API_HASH, api_hash.clone()),
        (db_key::PHONE, phone.clone()),
        (db_key::PROXY_URL, proxy_url.clone()),
        (db_key::SESSION_FILE, session_file.clone()),
    ] {
        // Adding a second account must not move the primary session pointer.
        if key == db_key::SESSION_FILE && state.loader.is_some() {
            continue;
        }
        if let Err(error) = db.set(key, Value::String(value)).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }

    if let Some(on) = body.desktop_profiles {
        if let Err(error) = crate::profiles::set_enabled(db, on).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }

    match crate::client::auth::connect_and_send_code(
        db,
        api_id,
        &api_hash,
        &phone,
        &session_file,
        Some(proxy_url).filter(|value| !value.is_empty()),
    )
    .await
    {
        Ok(pending) => {
            *state.pending.lock().await = Some(pending);
            api_ok()
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, format!("{e:#}")),
    }
}

pub fn session_file_from_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() || name == "default" {
        return DEFAULT_SESSION_FILE.to_string();
    }

    let safe_name = name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        .collect::<String>();
    let safe_name = if safe_name.is_empty() {
        "account".to_string()
    } else {
        safe_name
    };

    format!("{SESSIONS_DIR}/{safe_name}.session")
}

#[derive(Deserialize)]
pub struct SignInRequest {
    code: String,
    #[serde(default)]
    password: String,
}

pub async fn sign_in(State(state): State<AppState>, Json(body): Json<SignInRequest>) -> Response {
    let pending = state.pending.lock().await.take();
    let Some(pending) = pending else {
        return api_error(StatusCode::BAD_REQUEST, "Request a login code first");
    };

    match crate::client::auth::complete_sign_in(pending, body.code.trim(), &body.password).await {
        Ok(session_file) => {
            if let Err(e) =
                crate::client::auth::remember_session_file(state.db(), &session_file).await
            {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, e);
            }

            if let Some(loader) = &state.loader {
                crate::client::spawn_session(Arc::clone(loader), session_file);
            }

            if let Some(tx) = state.shutdown_tx.lock().await.take() {
                let _ = tx.send(());
            }
            // The operator who just signed in owns the account, so keep them logged in.
            let session = state.services.panel_auth.create_session().await;
            let mut response = api_ok();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, auth::session_cookie(&session));
            response
        }
        Err(crate::client::auth::SignInOutcome::NeedPassword { hint, pending }) => {
            // Restore the moved login token so the password retry can reuse it.
            *state.pending.lock().await = Some(pending);
            (
                StatusCode::OK,
                Json(json!({ "ok": false, "need_password": true, "password_hint": hint })),
            )
                .into_response()
        }
        Err(crate::client::auth::SignInOutcome::Failed(e)) => api_error(
            StatusCode::BAD_REQUEST,
            friendly_sign_in_error(&e.to_string()),
        ),
    }
}

fn friendly_sign_in_error(raw: &str) -> String {
    let upper = raw.to_uppercase();
    if upper.contains("PHONE_CODE_INVALID") {
        "The code is incorrect.".to_string()
    } else if upper.contains("PHONE_CODE_EXPIRED") {
        "The code has expired. Request a new one.".to_string()
    } else if upper.contains("PASSWORD_HASH_INVALID") {
        "The two-step verification password is incorrect.".to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_names_are_sanitized() {
        assert_eq!(session_file_from_name(""), DEFAULT_SESSION_FILE);
        assert_eq!(session_file_from_name("work"), "sessions/work.session");
        assert_eq!(session_file_from_name("../evil"), "sessions/evil.session");
        assert_eq!(session_file_from_name("///"), "sessions/account.session");
    }

    #[test]
    fn sign_in_errors_are_readable() {
        assert_eq!(
            friendly_sign_in_error("rpc error 400: PHONE_CODE_INVALID"),
            "The code is incorrect."
        );
    }
}
