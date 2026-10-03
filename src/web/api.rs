//! Dashboard JSON API. Every route here sits behind the panel session middleware.

use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{auth, AppState};
use crate::config::{db_key, SOCKS5_SCHEME};
use crate::core_settings::{self, key, TOGGLES};
use crate::i18n::{self, Lang};

pub fn api_ok() -> Response {
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

pub fn api_error(status: StatusCode, error: impl std::fmt::Display) -> Response {
    (
        status,
        Json(json!({ "ok": false, "error": error.to_string() })),
    )
        .into_response()
}

macro_rules! try_loader {
    ($state:expr) => {
        match $state.loader.as_ref() {
            Some(loader) => loader,
            None => return api_error(StatusCode::SERVICE_UNAVAILABLE, "panel is not ready"),
        }
    };
}

#[derive(Deserialize)]
pub struct PanelLoginRequest {
    password: String,
}

fn with_session_cookie(mut response: Response, session: &str) -> Response {
    response
        .headers_mut()
        .insert(header::SET_COOKIE, auth::session_cookie(session));
    response
}

pub async fn auth_login(
    State(state): State<AppState>,
    Json(body): Json<PanelLoginRequest>,
) -> Response {
    let panel_auth = &state.services.panel_auth;
    if !panel_auth.allow_login_attempt().await {
        return api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many attempts, wait a minute",
        );
    }
    if !auth::password_is_set(state.db()).await {
        return api_error(
            StatusCode::BAD_REQUEST,
            "no panel password is set; use the login link from the console or .panel",
        );
    }
    if !auth::verify_password(state.db(), &body.password).await {
        panel_auth.record_failed_login().await;
        return api_error(StatusCode::UNAUTHORIZED, "wrong password");
    }
    let session = panel_auth.create_session().await;
    with_session_cookie(api_ok(), &session)
}

#[derive(Deserialize)]
pub struct TokenQuery {
    #[serde(default)]
    t: String,
}

pub async fn auth_token(
    State(state): State<AppState>,
    Query(query): Query<TokenQuery>,
) -> Response {
    let panel_auth = &state.services.panel_auth;
    if !panel_auth.redeem_token(query.t.trim()).await {
        return Redirect::to("/auth?expired=1").into_response();
    }
    let session = panel_auth.create_session().await;
    with_session_cookie(Redirect::to("/").into_response(), &session)
}

pub async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(session) = auth::session_from_headers(&headers) {
        state.services.panel_auth.revoke_session(&session).await;
    }
    let mut response = api_ok();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, auth::clear_cookie());
    response
}

pub async fn status(State(state): State<AppState>) -> Response {
    let loader = try_loader!(state);
    let runtime = &state.services.runtime;
    let summaries = loader.summaries().await;
    let bot_username = state.services.notifier.bot_username().await;
    let body = json!({
        "version": crate::VERSION,
        "target": crate::updater::TARGET,
        "uptime_seconds": runtime.uptime_seconds(),
        "connected": runtime.connected(),
        "account_name": runtime.account_name().await,
        "accounts": runtime
            .accounts()
            .await
            .into_iter()
            .map(|account| {
                let avatar = crate::anti_delete::account_avatar_url(&account.account_id);
                let mut value = serde_json::to_value(&account).unwrap_or_default();
                value["avatar"] = json!(avatar);
                value
            })
            .collect::<Vec<_>>(),
        "updates_seen": runtime.updates_seen(),
        "commands_seen": runtime.commands_seen(),
        "errors_seen": runtime.errors_seen(),
        "memory_bytes": crate::runtime::process_memory_bytes(),
        "cpu_percent": runtime.process_cpu_percent().await,
        "activity": runtime.activity(),
        "top_commands": runtime.top_commands(8),
        "module_count": summaries.len(),
        "modules_enabled": summaries.iter().filter(|m| m.enabled).count(),
        "modules_sandboxed": summaries.iter().filter(|m| !m.info.trusted).count(),
        "db_encrypted": state.db().encryption_enabled().await,
        "bot": { "running": bot_username.is_some(), "username": bot_username },
        "prefixes": core_settings::prefixes(state.db()).await,
        "source_checkout": crate::updater::is_source_checkout(),
        "jobs_running": state.services.jobs.running().await,
    });
    Json(body).into_response()
}

pub async fn modules(State(state): State<AppState>) -> Response {
    let loader = try_loader!(state);
    Json(loader.summaries().await).into_response()
}

#[derive(Deserialize)]
pub struct EnabledRequest {
    enabled: bool,
}

pub async fn set_module_enabled(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<EnabledRequest>,
) -> Response {
    let loader = try_loader!(state);
    match loader.set_enabled(&name, body.enabled).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn reload_module(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let loader = try_loader!(state);
    match loader.reload(&name).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

pub async fn remove_module(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let loader = try_loader!(state);
    match loader.remove(&name).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn module_source(State(state): State<AppState>, Path(name): Path<String>) -> Response {
    let loader = try_loader!(state);
    match loader.source(&name).await {
        Some(source) => (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            source,
        )
            .into_response(),
        None => api_error(StatusCode::NOT_FOUND, "unknown module"),
    }
}

#[derive(Deserialize)]
pub struct ConfigRequest {
    key: String,
    #[serde(default)]
    value: Value,
}

pub async fn set_module_config(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<ConfigRequest>,
) -> Response {
    let loader = try_loader!(state);
    match loader.set_config_value(&name, &body.key, body.value).await {
        Ok(_) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct PermissionsRequest {
    permissions: Vec<String>,
}

pub async fn set_module_permissions(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<PermissionsRequest>,
) -> Response {
    let loader = try_loader!(state);
    match loader.set_permissions(&name, body.permissions).await {
        Ok(()) => api_ok(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, format!("{error:#}")),
    }
}

#[derive(Deserialize)]
pub struct InstallRequest {
    url: String,
    #[serde(default)]
    name: Option<String>,
}

pub async fn install_module(
    State(state): State<AppState>,
    Json(body): Json<InstallRequest>,
) -> Response {
    let loader = try_loader!(state);
    let name = body.name.filter(|name| !name.trim().is_empty());
    match crate::loader::installer::install_from_url(loader.modules_dir(), &body.url, name).await {
        Ok(summary) => Json(json!({ "ok": true, "summary": summary })).into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, error),
    }
}

#[derive(Deserialize)]
pub struct RestoreModulesRequest {
    #[serde(default)]
    name: Option<String>,
}

pub async fn restore_modules(
    State(state): State<AppState>,
    Json(body): Json<RestoreModulesRequest>,
) -> Response {
    let loader = try_loader!(state);
    match crate::bundled::restore(loader.modules_dir(), body.name.as_deref()) {
        Ok(restored) => Json(json!({ "ok": true, "restored": restored })).into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

#[derive(Deserialize)]
pub struct LogsQuery {
    #[serde(default)]
    after: u64,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub async fn logs(State(state): State<AppState>, Query(query): Query<LogsQuery>) -> Response {
    let level = query
        .level
        .as_deref()
        .and_then(|level| level.parse::<tracing::Level>().ok());
    let records =
        state
            .services
            .logs
            .since(query.after, level, query.limit.unwrap_or(500).min(2000));
    Json(records).into_response()
}

pub async fn antidelete(State(state): State<AppState>) -> Response {
    Json(crate::anti_delete::store(state.db()).await).into_response()
}

pub async fn delete_log(
    State(state): State<AppState>,
    Query(filters): Query<crate::anti_delete::DeleteLogFilters>,
) -> Response {
    Json(crate::anti_delete::delete_log(state.db(), filters).await).into_response()
}

pub async fn settings(State(state): State<AppState>) -> Response {
    let db = state.db();
    let mut toggles = Vec::new();
    for (key, label, default) in TOGGLES {
        toggles.push(json!({
            "key": key,
            "label_en": i18n::tr(Lang::En, label),
            "label_ru": i18n::tr(Lang::Ru, label),
            "enabled": core_settings::flag(db, key, *default).await,
            "group": if key.starts_with("notify.") { "notifications" } else { "handlers" },
        }));
    }
    let bot_username = state.services.notifier.bot_username().await;
    Json(json!({
        "prefixes": core_settings::prefixes(db).await,
        "language": core_settings::language(db).await.code(),
        "proxy_url": db.get(db_key::PROXY_URL).await.as_str().unwrap_or_default(),
        "tor": crate::proxy::is_tor(db.get(db_key::PROXY_URL).await.as_str().unwrap_or_default()),
        "db_encrypted": db.encryption_enabled().await,
        "panel_password_set": auth::password_is_set(db).await,
        "bot": {
            "configured": core_settings::bot_token(db).await.is_some(),
            "running": bot_username.is_some(),
            "username": bot_username,
        },
        "sudo_users": core_settings::sudo_users(db).await,
        "toggles": toggles,
        "web": { "host": state.services.config.web.host, "port": state.services.config.web.port },
    }))
    .into_response()
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct SettingsUpdate {
    prefixes: Option<Vec<String>>,
    language: Option<String>,
    proxy_url: Option<String>,
    bot_token: Option<String>,
    toggles: Option<serde_json::Map<String, Value>>,
    sudo_users: Option<Vec<i64>>,
    panel_password: Option<String>,
    clear_panel_password: bool,
    master_password: Option<String>,
    clear_master_password: bool,
}

pub async fn update_settings(
    State(state): State<AppState>,
    Json(body): Json<SettingsUpdate>,
) -> Response {
    let db = state.db();

    if let Some(prefixes) = &body.prefixes {
        if let Err(error) = core_settings::set_prefixes(db, prefixes).await {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }
    if let Some(language) = &body.language {
        if let Err(error) = core_settings::set_language(db, Lang::parse(language)).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    if let Some(proxy_url) = &body.proxy_url {
        let proxy_url = proxy_url.trim();
        if !proxy_url.is_empty() && !proxy_url.starts_with(SOCKS5_SCHEME) {
            return api_error(StatusCode::BAD_REQUEST, "proxy must use socks5://");
        }
        if let Err(error) = db
            .set(db_key::PROXY_URL, Value::String(proxy_url.to_string()))
            .await
        {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    if let Some(toggles) = &body.toggles {
        for (key, value) in toggles {
            // Only known switches are writable from here.
            if core_settings::toggle_default(key).is_none() {
                continue;
            }
            if let Some(enabled) = value.as_bool() {
                if let Err(error) = db.set(key.as_str(), Value::Bool(enabled)).await {
                    return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
                }
            }
        }
    }
    if let Some(users) = &body.sudo_users {
        if let Err(error) = core_settings::set_sudo_users(db, users).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }
    if body.clear_panel_password {
        if let Err(error) = auth::set_password(db, None).await {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    } else if let Some(password) = body.panel_password.as_deref().filter(|p| !p.is_empty()) {
        if let Err(error) = auth::set_password(db, Some(password)).await {
            return api_error(StatusCode::BAD_REQUEST, error);
        }
    }

    if let Some(token) = &body.bot_token {
        let token = token.trim();
        if token.is_empty() {
            if let Err(error) = db.remove(key::BOT_TOKEN).await {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
            }
        } else {
            if let Err(error) = crate::bot::validate_token(token).await {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    format!("Telegram rejected the bot token: {error}"),
                );
            }
            if let Err(error) = db
                .set(key::BOT_TOKEN, Value::String(token.to_string()))
                .await
            {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
            }
        }
        if let Some(bot) = &state.bot {
            bot.restart().await;
        }
    }

    let next_master = if body.clear_master_password {
        Some(None)
    } else {
        body.master_password
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| Some(p.to_string()))
    };
    if let Some(next_password) = next_master {
        if next_password
            .as_ref()
            .is_some_and(|p| p.chars().count() < 8)
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                "master password must be at least 8 characters",
            );
        }
        let current = db.master_password().await;
        let result =
            match crate::anti_delete::rewrap_storage(current.as_deref(), next_password.as_deref())
                .await
            {
                Ok(()) => db.set_master_password(next_password).await,
                Err(error) => Err(error),
            };
        if let Err(error) = result {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, error);
        }
    }

    api_ok()
}

pub async fn download_backup() -> Response {
    let result = tokio::task::spawn_blocking(|| {
        let root = std::env::current_dir()?;
        let path = crate::backup::create_archive(&root)?;
        let bytes = std::fs::read(&path)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "fly-telegram-backup.tar.gz".to_string());
        anyhow::Ok((name, bytes))
    })
    .await;
    match result {
        Ok(Ok((name, bytes))) => (
            [
                (header::CONTENT_TYPE, "application/gzip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{name}\""),
                ),
            ],
            Body::from(bytes),
        )
            .into_response(),
        Ok(Err(error)) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

pub async fn restore_backup(mut multipart: Multipart) -> Response {
    let mut bytes = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            match field.bytes().await {
                Ok(data) => bytes = Some(data),
                Err(error) => return api_error(StatusCode::BAD_REQUEST, error),
            }
        }
    }
    let Some(bytes) = bytes else {
        return api_error(StatusCode::BAD_REQUEST, "no file uploaded");
    };
    let result = tokio::task::spawn_blocking(move || {
        let root = std::env::current_dir()?;
        let dir = root.join(crate::backup::BACKUP_DIR);
        std::fs::create_dir_all(&dir)?;
        let upload = dir.join(format!("upload-{}.tar.gz", crate::backup::timestamp()));
        std::fs::write(&upload, &bytes)?;
        let restored = crate::backup::restore_archive(&root, &upload);
        let _ = std::fs::remove_file(&upload);
        restored
    })
    .await;
    match result {
        Ok(Ok(restored)) => {
            schedule_restart();
            Json(json!({ "ok": true, "restored": restored, "restarting": true })).into_response()
        }
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, error),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

/// Restarts shortly after the response is sent so the browser receives it.
fn schedule_restart() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        crate::restart::restart_now();
    });
}

pub async fn restart() -> Response {
    schedule_restart();
    Json(json!({ "ok": true, "restarting": true })).into_response()
}

pub async fn check_update() -> Response {
    if crate::updater::is_source_checkout() {
        return Json(json!({
            "current": crate::VERSION,
            "source_checkout": true,
        }))
        .into_response();
    }
    match crate::updater::latest_release().await {
        Ok(release) => Json(json!({
            "current": crate::VERSION,
            "latest": release.version,
            "newer": crate::updater::is_newer(&release.version, crate::VERSION),
            "notes": release.notes,
            "url": release.page_url,
            "has_binary": release.binary_url.is_some(),
            "source_checkout": false,
        }))
        .into_response(),
        Err(error) => api_error(StatusCode::BAD_GATEWAY, error),
    }
}

pub async fn install_update() -> Response {
    if crate::updater::is_source_checkout() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "this is a source checkout; use .update in Telegram (git pull and cargo build)",
        );
    }
    let release = match crate::updater::latest_release().await {
        Ok(release) => release,
        Err(error) => return api_error(StatusCode::BAD_GATEWAY, error),
    };
    if !crate::updater::is_newer(&release.version, crate::VERSION) {
        return api_error(StatusCode::BAD_REQUEST, "already up to date");
    }
    match crate::updater::install(&release).await {
        Ok(_) => {
            schedule_restart();
            Json(json!({ "ok": true, "version": release.version, "restarting": true }))
                .into_response()
        }
        Err(error) => api_error(StatusCode::BAD_GATEWAY, format!("{error:#}")),
    }
}
