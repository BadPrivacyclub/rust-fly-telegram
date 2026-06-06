use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex};
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::info;

use crate::client::auth::PendingAuth;
use crate::config::{db_key, DEFAULT_SESSION_FILE, SESSIONS_DIR, SOCKS5_SCHEME};
use crate::database::Database;
use crate::loader::manifest::ModuleInfo;
use crate::loader::Loader;
use crate::runtime::{AccountRuntime, RuntimeState};

#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    loader: Option<Arc<Loader>>,
    runtime: Option<Arc<RuntimeState>>,
    /// Holds a live Telegram connection and login token between the two web requests.
    pending: Arc<Mutex<Option<PendingAuth>>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

/// Starts the web authorization server.
///
/// Returns once the user completes sign-in, writing the authorized session to disk.
pub async fn run_until_authorized(db: Arc<Database>) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let state = AppState {
        db,
        loader: None,
        runtime: None,
        pending: Arc::new(Mutex::new(None)),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
    };

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/send_code", post(send_code_handler))
        .route("/sign_in", post(sign_in_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
    info!("web auth server at http://127.0.0.1:8080");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await?;

    Ok(())
}

/// Starts the local runtime dashboard.
pub async fn run_dashboard(
    db: Arc<Database>,
    loader: Arc<Loader>,
    runtime: Arc<RuntimeState>,
) -> Result<()> {
    let state = AppState {
        db,
        loader: Some(loader),
        runtime: Some(runtime),
        pending: Arc::new(Mutex::new(None)),
        shutdown_tx: Arc::new(Mutex::new(None)),
    };

    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/login", get(index_handler))
        .route("/send_code", post(send_code_handler))
        .route("/sign_in", post(sign_in_handler))
        .route("/api/antidelete", get(antidelete_handler))
        .route("/api/deletelog", get(delete_log_handler))
        .route("/api/status", get(status_handler))
        .route(
            "/api/settings",
            get(settings_handler).post(update_settings_handler),
        )
        .nest_service("/media", ServeDir::new("data/deleted_media"))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080").await?;
    info!("dashboard at http://127.0.0.1:8080");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("login.html"))
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct SendCodeRequest {
    phone: String,
    api_id: String,
    api_hash: String,
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    session_name: String,
}

#[derive(Deserialize)]
struct SignInRequest {
    code: String,
    #[serde(default)]
    password: String,
}

#[derive(Deserialize)]
struct UpdateSettingsRequest {
    #[serde(default)]
    proxy_url: String,
    #[serde(default)]
    master_password: String,
    #[serde(default)]
    clear_master_password: bool,
}

#[derive(Serialize)]
struct StatusResponse {
    uptime_seconds: u64,
    connected: bool,
    account_name: Option<String>,
    accounts: Vec<AccountRuntime>,
    updates_seen: u64,
    commands_seen: u64,
    modules: Vec<String>,
    module_details: Vec<ModuleInfo>,
    module_count: usize,
    db_encrypted: bool,
    proxy_url: Option<String>,
}

#[derive(Serialize)]
struct SettingsResponse {
    proxy_url: Option<String>,
    db_encrypted: bool,
}

#[derive(Serialize)]
struct ApiResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// True when 2FA is required after submitting the code.
    #[serde(skip_serializing_if = "Option::is_none")]
    need_password: Option<bool>,
    /// 2FA hint text shown to the user.
    #[serde(skip_serializing_if = "Option::is_none")]
    password_hint: Option<String>,
}

impl ApiResponse {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            need_password: None,
            password_hint: None,
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(msg.into()),
            need_password: None,
            password_hint: None,
        }
    }
}

fn json_response<T: Serialize>(value: T) -> (StatusCode, Json<serde_json::Value>) {
    match serde_json::to_value(value) {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error.to_string() })),
        ),
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// Saves credentials, connects to Telegram, and sends the login code.
async fn send_code_handler(
    State(state): State<AppState>,
    Json(body): Json<SendCodeRequest>,
) -> impl IntoResponse {
    let api_id: i32 = match body.api_id.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::err("api_id must be an integer")),
            );
        }
    };

    // Persist credentials for auth.rs to use on next launch (avoids re-entering them).
    let proxy_url = body.proxy_url.trim().to_string();
    if !proxy_url.is_empty() && !proxy_url.starts_with(SOCKS5_SCHEME) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("proxy must use socks5://")),
        );
    }

    let session_file = session_file_from_name(&body.session_name);

    let _ = state
        .db
        .set(
            db_key::API_ID,
            serde_json::Value::String(body.api_id.clone()),
        )
        .await;
    let _ = state
        .db
        .set(
            db_key::API_HASH,
            serde_json::Value::String(body.api_hash.clone()),
        )
        .await;
    let _ = state
        .db
        .set(db_key::PHONE, serde_json::Value::String(body.phone.clone()))
        .await;
    let _ = state
        .db
        .set(
            db_key::PROXY_URL,
            serde_json::Value::String(proxy_url.clone()),
        )
        .await;
    let _ = state
        .db
        .set(
            db_key::SESSION_FILE,
            serde_json::Value::String(session_file.clone()),
        )
        .await;

    match crate::client::auth::connect_and_send_code(
        api_id,
        &body.api_hash,
        &body.phone,
        &session_file,
        Some(proxy_url).filter(|value| !value.is_empty()),
    )
    .await
    {
        Ok(pending) => {
            *state.pending.lock().await = Some(pending);
            (StatusCode::OK, Json(ApiResponse::ok()))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(e.to_string())),
        ),
    }
}

fn session_file_from_name(name: &str) -> String {
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

/// Submits the login code (and optional 2FA password) to Telegram.
async fn sign_in_handler(
    State(state): State<AppState>,
    Json(body): Json<SignInRequest>,
) -> impl IntoResponse {
    let pending = state.pending.lock().await.take();
    let Some(pending) = pending else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("Call /send_code first")),
        );
    };

    match crate::client::auth::complete_sign_in(pending, &body.code, &body.password).await {
        Ok(session_file) => {
            if let Err(e) =
                crate::client::auth::remember_session_file(&state.db, &session_file).await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::err(e.to_string())),
                );
            }

            if let (Some(loader), Some(runtime)) = (&state.loader, &state.runtime) {
                crate::client::spawn_session(
                    Arc::clone(&state.db),
                    Arc::clone(loader),
                    Arc::clone(runtime),
                    session_file,
                );
            }

            // Shut down the web server — authorization is complete.
            if let Some(tx) = state.shutdown_tx.lock().await.take() {
                let _ = tx.send(());
            }
            (StatusCode::OK, Json(ApiResponse::ok()))
        }
        Err(crate::client::auth::SignInOutcome::NeedPassword { hint, pending }) => {
            // Give back the pending state so the user can retry with the password.
            *state.pending.lock().await = Some(pending);
            (
                StatusCode::OK,
                Json(ApiResponse {
                    ok: false,
                    error: None,
                    need_password: Some(true),
                    password_hint: hint,
                }),
            )
        }
        Err(crate::client::auth::SignInOutcome::Failed(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(e.to_string())),
        ),
    }
}

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let Some(runtime) = state.runtime.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "dashboard is not active" })),
        );
    };
    let modules = match state.loader.as_ref() {
        Some(loader) => loader.module_names().await,
        None => Vec::new(),
    };
    let module_details = match state.loader.as_ref() {
        Some(loader) => loader.module_info().await,
        None => Vec::new(),
    };
    let proxy_url = optional_db_string(&state.db, db_key::PROXY_URL).await;
    let response = StatusResponse {
        uptime_seconds: runtime.uptime_seconds(),
        connected: runtime.connected(),
        account_name: runtime.account_name().await,
        accounts: runtime.accounts().await,
        updates_seen: runtime.updates_seen(),
        commands_seen: runtime.commands_seen(),
        module_count: modules.len(),
        modules,
        module_details,
        db_encrypted: state.db.encryption_enabled().await,
        proxy_url,
    };

    json_response(response)
}

async fn antidelete_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(crate::anti_delete::store(&state.db).await),
    )
}

async fn delete_log_handler(
    State(state): State<AppState>,
    Query(filters): Query<crate::anti_delete::DeleteLogFilters>,
) -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(crate::anti_delete::delete_log(&state.db, filters).await),
    )
}

async fn settings_handler(State(state): State<AppState>) -> impl IntoResponse {
    let response = SettingsResponse {
        proxy_url: optional_db_string(&state.db, db_key::PROXY_URL).await,
        db_encrypted: state.db.encryption_enabled().await,
    };

    json_response(response)
}

async fn update_settings_handler(
    State(state): State<AppState>,
    Json(body): Json<UpdateSettingsRequest>,
) -> impl IntoResponse {
    let proxy_url = body.proxy_url.trim().to_string();
    if !proxy_url.is_empty() && !proxy_url.starts_with(SOCKS5_SCHEME) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::err("proxy must use socks5://")),
        );
    }

    if let Err(e) = state
        .db
        .set(db_key::PROXY_URL, serde_json::Value::String(proxy_url))
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(e.to_string())),
        );
    }

    let password = body.master_password.trim();
    let password_result = if body.clear_master_password || !password.is_empty() {
        let current_password = state.db.master_password().await;
        let next_password = if body.clear_master_password {
            None
        } else {
            Some(password.to_string())
        };

        match crate::anti_delete::rewrap_storage(
            current_password.as_deref(),
            next_password.as_deref(),
        )
        .await
        {
            Ok(()) => state.db.set_master_password(next_password).await,
            Err(e) => Err(e),
        }
    } else {
        Ok(())
    };

    match password_result {
        Ok(()) => (StatusCode::OK, Json(ApiResponse::ok())),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::err(e.to_string())),
        ),
    }
}

async fn optional_db_string(db: &Database, key: &str) -> Option<String> {
    db.get(key)
        .await
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::TestServer;
    use serde_json::json;
    use std::env;

    async fn test_app() -> TestServer {
        let path = env::temp_dir().join(format!("fly_web_test_{}.json", uuid::Uuid::new_v4()));
        let db = Arc::new(
            Database::load(&path)
                .await
                .expect("test database should be created in the temp directory"),
        );

        let (shutdown_tx, _shutdown_rx) = oneshot::channel::<()>();
        let state = AppState {
            db,
            loader: None,
            runtime: None,
            pending: Arc::new(Mutex::new(None)),
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        };

        let app = Router::new()
            .route("/", get(index_handler))
            .route("/send_code", post(send_code_handler))
            .route("/sign_in", post(sign_in_handler))
            .with_state(state);

        TestServer::new(app)
    }

    #[tokio::test]
    async fn send_code_rejects_non_integer_api_id() {
        let server = test_app().await;
        let res = server
            .post("/send_code")
            .json(&json!({ "api_id": "not_a_number", "api_hash": "abc", "phone": "+1" }))
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn sign_in_without_prior_send_code_returns_400() {
        let server = test_app().await;
        let res = server
            .post("/sign_in")
            .json(&json!({ "code": "12345" }))
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn index_returns_html() {
        let server = test_app().await;
        let res = server.get("/").await;
        assert_eq!(res.status_code(), 200);
        let text = res.text();
        assert!(text.contains("fly-telegram"));
    }
}
