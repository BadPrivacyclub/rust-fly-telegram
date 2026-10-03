//! Local web panel: first-run setup wizard and the dashboard.

use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::DefaultBodyLimit,
    http::{header, StatusCode},
    middleware,
    response::{Html, IntoResponse, Response},
    routing::{delete, get, post},
    Router,
};
use tokio::sync::{oneshot, Mutex};
use tower_http::services::ServeDir;
use tracing::info;

mod accounts_api;
mod api;
pub mod auth;
mod setup;

use crate::app::Services;
use crate::bot::BotManager;
use crate::client::auth::PendingAuth;
use crate::loader::Loader;

#[derive(Clone)]
pub(crate) struct AppState {
    pub services: Arc<Services>,
    pub loader: Option<Arc<Loader>>,
    pub bot: Option<Arc<BotManager>>,
    pub pending: Arc<Mutex<Option<PendingAuth>>>,
    pub shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl AppState {
    fn db(&self) -> &crate::database::Database {
        &self.services.db
    }
}

/// Serves the setup wizard until a Telegram account is authorized.
pub async fn run_until_authorized(services: Arc<Services>) -> Result<()> {
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let web = services.config.web.clone();
    let state = AppState {
        services,
        loader: None,
        bot: None,
        pending: Arc::new(Mutex::new(None)),
        shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
    };

    let app = setup_router()
        .layer(middleware::from_fn(auth::guard))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(web.bind_addr()).await?;
    info!("setup wizard: open {} in your browser", web.public_url());
    if web.open_browser {
        crate::desktop::open_browser(&web.public_url());
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await?;

    Ok(())
}

fn setup_router() -> Router<AppState> {
    Router::new()
        .route("/", get(setup_page))
        .route("/assets/{file}", get(asset_handler))
        .route("/api/setup/state", get(setup::state))
        .route("/api/setup/prefs", post(setup::save_prefs))
        .route("/send_code", post(setup::send_code))
        .route("/sign_in", post(setup::sign_in))
}

pub async fn run_dashboard(
    services: Arc<Services>,
    loader: Arc<Loader>,
    bot: Arc<BotManager>,
) -> Result<()> {
    let web = services.config.web.clone();
    let panel_auth = Arc::clone(&services.panel_auth);
    let state = AppState {
        services,
        loader: Some(loader),
        bot: Some(bot),
        pending: Arc::new(Mutex::new(None)),
        shutdown_tx: Arc::new(Mutex::new(None)),
    };

    let listener = tokio::net::TcpListener::bind(web.bind_addr()).await?;
    let token = panel_auth.issue_startup_token().await;
    info!("web panel at {}", web.public_url());
    info!(
        "one-time panel login link (valid 1 hour): {}/auth/token?t={token}",
        web.public_url()
    );

    axum::serve(listener, dashboard_router(state, panel_auth)).await?;
    Ok(())
}

fn dashboard_router(state: AppState, panel_auth: Arc<auth::PanelAuth>) -> Router {
    let public = Router::new()
        .route("/assets/{file}", get(asset_handler))
        .route("/auth", get(auth_page))
        .route("/auth/login", post(api::auth_login))
        .route("/auth/token", get(api::auth_token))
        .route("/auth/logout", post(api::auth_logout));

    let protected = Router::new()
        .route("/", get(dashboard_page))
        .route("/account", get(setup_page))
        .route("/api/setup/state", get(setup::state))
        .route("/send_code", post(setup::send_code))
        .route("/sign_in", post(setup::sign_in))
        .route("/api/status", get(api::status))
        .route("/api/modules", get(api::modules))
        .route("/api/modules/install", post(api::install_module))
        .route("/api/modules/restore", post(api::restore_modules))
        .route("/api/modules/{name}", delete(api::remove_module))
        .route("/api/modules/{name}/enabled", post(api::set_module_enabled))
        .route("/api/modules/{name}/reload", post(api::reload_module))
        .route("/api/modules/{name}/source", get(api::module_source))
        .route("/api/modules/{name}/config", post(api::set_module_config))
        .route(
            "/api/modules/{name}/permissions",
            post(api::set_module_permissions),
        )
        .route("/api/logs", get(api::logs))
        .route("/api/antidelete", get(api::antidelete))
        .route("/api/deletelog", get(api::delete_log))
        .route(
            "/api/settings",
            get(api::settings).post(api::update_settings),
        )
        .route("/api/backup", get(api::download_backup))
        .route(
            "/api/restore",
            post(api::restore_backup).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route("/api/restart", post(api::restart))
        .route("/api/accounts", get(accounts_api::list))
        .route("/api/accounts/{id}/counts", get(accounts_api::counts))
        .route("/api/accounts/{id}/profile", post(accounts_api::profile))
        .route(
            "/api/accounts/cleanup/preview",
            post(accounts_api::cleanup_preview),
        )
        .route("/api/accounts/cleanup/run", post(accounts_api::cleanup_run))
        .route("/api/accounts/bulk", post(accounts_api::bulk))
        .route(
            "/api/automations",
            get(accounts_api::automations).post(accounts_api::save_automation),
        )
        .route(
            "/api/automations/{id}",
            delete(accounts_api::delete_automation),
        )
        .route(
            "/api/automations/{id}/enabled",
            post(accounts_api::toggle_automation),
        )
        .route(
            "/api/profiles",
            get(accounts_api::profiles).post(accounts_api::update_profiles),
        )
        .route("/api/profiles/assign", post(accounts_api::assign_profile))
        .route("/api/profiles/{id}", delete(accounts_api::delete_profile))
        .route("/api/jobs", get(accounts_api::jobs))
        .route("/api/jobs/{id}/cancel", post(accounts_api::cancel_job))
        .route(
            "/api/update",
            get(api::check_update).post(api::install_update),
        )
        .nest_service("/media", ServeDir::new("data/deleted_media"))
        .nest_service("/avatars", ServeDir::new("data/avatars"))
        .layer(middleware::from_fn_with_state(
            Arc::clone(&panel_auth),
            auth::require_session,
        ));

    public
        .merge(protected)
        .layer(middleware::from_fn(auth::guard))
        .with_state(state)
}

async fn setup_page() -> Html<&'static str> {
    Html(include_str!("setup.html"))
}

async fn auth_page() -> Html<&'static str> {
    Html(include_str!("auth.html"))
}

async fn dashboard_page() -> Html<&'static str> {
    Html(include_str!("dashboard.html"))
}

async fn asset_handler(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    let (content_type, body): (&str, &'static str) = match file.as_str() {
        "app.css" => ("text/css; charset=utf-8", include_str!("assets/app.css")),
        "common.js" => (
            "text/javascript; charset=utf-8",
            include_str!("assets/common.js"),
        ),
        "icon.svg" => ("image/svg+xml", include_str!("assets/icon.svg")),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_test::TestServer;
    use serde_json::json;

    fn local(server: &TestServer, method: &str, path: &str) -> axum_test::TestRequest {
        let request = match method {
            "POST" => server.post(path),
            "DELETE" => server.delete(path),
            _ => server.get(path),
        };
        request
            .add_header("host", "127.0.0.1:8080")
            .add_header("x-fly-request", "1")
    }

    async fn setup_server() -> TestServer {
        let services = crate::app::test_services().await;
        let (shutdown_tx, _rx) = oneshot::channel::<()>();
        let state = AppState {
            services,
            loader: None,
            bot: None,
            pending: Arc::new(Mutex::new(None)),
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        };
        TestServer::new(
            setup_router()
                .layer(middleware::from_fn(auth::guard))
                .with_state(state),
        )
    }

    async fn dashboard_server() -> (TestServer, Arc<Services>) {
        let services = crate::app::test_services().await;
        let dir = std::env::temp_dir().join(format!("fly_web_mods_{}", uuid::Uuid::new_v4()));
        crate::bundled::sync(&dir).unwrap();
        let loader = Arc::new(Loader::new(Arc::clone(&services), &dir).await.unwrap());
        loader.load_all().await.unwrap();
        let bot = BotManager::new(Arc::clone(&loader));
        let state = AppState {
            services: Arc::clone(&services),
            loader: Some(loader),
            bot: Some(bot),
            pending: Arc::new(Mutex::new(None)),
            shutdown_tx: Arc::new(Mutex::new(None)),
        };
        let router = dashboard_router(state, Arc::clone(&services.panel_auth));
        (TestServer::new(router), services)
    }

    async fn login(server: &TestServer, services: &Services) -> String {
        let token = services.panel_auth.issue_link_token().await;
        let response = local(server, "GET", &format!("/auth/token?t={token}")).await;
        response
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .expect("login sets a cookie")
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    /// Serves the dashboard on FLY_PREVIEW_PORT with sample data for visual checks:
    /// `FLY_PREVIEW_PORT=18181 cargo test preview_dashboard -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "manual preview server"]
    async fn preview_dashboard() {
        let port: u16 = std::env::var("FLY_PREVIEW_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(18181);
        let services = crate::app::test_services().await;
        let dir = std::env::temp_dir().join(format!("fly_preview_{}", uuid::Uuid::new_v4()));
        crate::bundled::sync(&dir).unwrap();
        std::fs::write(
            dir.join("thirdparty.lua"),
            "local M = {} M.commands = { hello = 'h' } function M.h(ctx) ctx:http_get('https://x') end return M",
        )
        .unwrap();
        let loader = Arc::new(Loader::new(Arc::clone(&services), &dir).await.unwrap());
        loader.load_all().await.unwrap();
        let runtime = &services.runtime;
        runtime
            .set_account_connected(
                "fly-telegram.session".into(),
                "123456789".into(),
                "Alex".into(),
            )
            .await;
        for i in 0..40 {
            runtime.record_update();
            if i % 3 == 0 {
                runtime.record_command();
                runtime.record_command_name(["ping", "help", "calc", "remind"][i % 4]);
            }
        }
        tracing::info!("preview log line");
        services.logs.push(
            &tracing::Level::WARN,
            "fly_telegram",
            "sample warning".into(),
        );
        services.logs.push(
            &tracing::Level::ERROR,
            "fly_telegram",
            "module 'x' handler 'y': boom".into(),
        );
        for (kind, value) in [
            (
                "away_reply",
                serde_json::json!({ "name": "Night mode", "type": "away_reply", "text": "🌙 Sleeping", "from": "23:00", "to": "08:00" }),
            ),
            (
                "profile_clock",
                serde_json::json!({ "name": "Clock", "type": "profile_clock", "field": "name", "template": "Alex {clock} {time}" }),
            ),
            (
                "schedule",
                serde_json::json!({ "name": "Daily plan", "type": "schedule", "chat": "me", "text": "Plan the day", "at": "09:00", "enabled": false }),
            ),
        ] {
            let rule: crate::automations::Rule = serde_json::from_value(value).unwrap();
            crate::automations::upsert_rule(&services, rule)
                .await
                .unwrap_or_else(|e| panic!("{kind}: {e}"));
        }
        let job = services
            .jobs
            .start("cleanup", "Account cleanup", "Alex")
            .await;
        job.set_total(40).await;
        for i in 0..17 {
            job.advance().await;
            job.log(format!("🚪 Left: Group {i}")).await;
        }
        let done = services.jobs.start("bulk", "Read all chats", "Alex").await;
        done.set_total(12).await;
        done.finish(crate::jobs::JobStatus::Done, "12 done, 0 failed")
            .await;
        let token = services.panel_auth.issue_startup_token().await;
        println!("PREVIEW http://127.0.0.1:{port}/auth/token?t={token}");
        let bot = BotManager::new(Arc::clone(&loader));
        let state = AppState {
            services: Arc::clone(&services),
            loader: Some(loader),
            bot: Some(bot),
            pending: Arc::new(Mutex::new(None)),
            shutdown_tx: Arc::new(Mutex::new(None)),
        };
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        let router = dashboard_router(state, Arc::clone(&services.panel_auth))
            .merge(Router::new().route("/setup-preview", get(setup_page)));
        axum::serve(listener, router).await.unwrap();
    }

    #[tokio::test]
    async fn send_code_rejects_non_integer_api_id() {
        let server = setup_server().await;
        let res = local(&server, "POST", "/send_code")
            .json(&json!({ "api_id": "not_a_number", "api_hash": "abc", "phone": "+1" }))
            .await;
        assert_eq!(res.status_code(), 400);
        let body: serde_json::Value = res.json();
        assert_eq!(body["ok"], false);
    }

    #[tokio::test]
    async fn sign_in_without_prior_send_code_returns_400() {
        let server = setup_server().await;
        let res = local(&server, "POST", "/sign_in")
            .json(&json!({ "code": "12345" }))
            .await;
        assert_eq!(res.status_code(), 400);
    }

    #[tokio::test]
    async fn setup_page_and_guard() {
        let server = setup_server().await;
        let res = local(&server, "GET", "/").await;
        assert_eq!(res.status_code(), 200);
        assert!(res.text().contains("fly-telegram"));

        let foreign = server.get("/").add_header("host", "evil.example").await;
        assert_eq!(foreign.status_code(), 403);

        let no_csrf = server
            .post("/send_code")
            .add_header("host", "localhost:8080")
            .json(&json!({}))
            .await;
        assert_eq!(no_csrf.status_code(), 403);

        let cross_origin = local(&server, "POST", "/api/setup/prefs")
            .add_header("origin", "https://evil.example")
            .json(&json!({}))
            .await;
        assert_eq!(cross_origin.status_code(), 403);
    }

    #[tokio::test]
    async fn setup_prefs_validate_and_store() {
        let server = setup_server().await;
        let res = local(&server, "POST", "/api/setup/prefs")
            .json(&json!({ "panel_password": "123" }))
            .await;
        assert_eq!(res.status_code(), 400);

        let res = local(&server, "POST", "/api/setup/prefs")
            .json(&json!({ "panel_password": "secret-pass", "prefix": "!", "language": "ru" }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());

        let state: serde_json::Value = local(&server, "GET", "/api/setup/state").await.json();
        assert_eq!(state["panel_password_set"], true);
        assert_eq!(state["language"], "ru");
    }

    #[tokio::test]
    async fn dashboard_requires_session() {
        let (server, services) = dashboard_server().await;
        assert_eq!(
            local(&server, "GET", "/api/status").await.status_code(),
            401
        );
        let page = local(&server, "GET", "/").await;
        assert_eq!(page.status_code(), 303);
        assert_eq!(local(&server, "GET", "/auth").await.status_code(), 200);

        let cookie = login(&server, &services).await;
        let status = local(&server, "GET", "/api/status")
            .add_header("cookie", cookie.clone())
            .await;
        assert_eq!(status.status_code(), 200);
        let body: serde_json::Value = status.json();
        assert_eq!(body["version"], crate::VERSION);
        assert_eq!(body["activity"].as_array().map(Vec::len), Some(60));

        let logout = local(&server, "POST", "/auth/logout")
            .add_header("cookie", cookie.clone())
            .await;
        assert_eq!(logout.status_code(), 200);
        let after = local(&server, "GET", "/api/status")
            .add_header("cookie", cookie)
            .await;
        assert_eq!(after.status_code(), 401);
    }

    #[tokio::test]
    async fn password_login_is_checked() {
        let (server, services) = dashboard_server().await;
        auth::set_password(&services.db, Some("hunter22"))
            .await
            .unwrap();
        let wrong = local(&server, "POST", "/auth/login")
            .json(&json!({ "password": "nope" }))
            .await;
        assert_eq!(wrong.status_code(), 401);
        let right = local(&server, "POST", "/auth/login")
            .json(&json!({ "password": "hunter22" }))
            .await;
        assert_eq!(right.status_code(), 200);
        assert!(right.headers().get("set-cookie").is_some());
    }

    #[tokio::test]
    async fn modules_and_settings_api() {
        let (server, services) = dashboard_server().await;
        let cookie = login(&server, &services).await;
        let get =
            |path: &'static str| local(&server, "GET", path).add_header("cookie", cookie.clone());
        let post =
            |path: &'static str| local(&server, "POST", path).add_header("cookie", cookie.clone());

        let modules: serde_json::Value = get("/api/modules").await.json();
        let list = modules.as_array().unwrap();
        assert!(list.iter().any(|m| m["name"] == "utils"));

        let res = post("/api/modules/utils/enabled")
            .json(&json!({ "enabled": false }))
            .await;
        assert_eq!(res.status_code(), 200);
        let res = post("/api/modules/utils/config")
            .json(&json!({ "key": "units", "value": "imperial" }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let res = post("/api/modules/utils/config")
            .json(&json!({ "key": "units", "value": "kelvin" }))
            .await;
        assert_eq!(res.status_code(), 400);

        let source = get("/api/modules/core/source").await;
        assert!(source.text().contains("ping_cmd"));

        let res = post("/api/settings")
            .json(&json!({
                "prefixes": [".", "!"],
                "language": "ru",
                "toggles": { "handlers.afk.enabled": true, "bogus.key": true },
                "sudo_users": [42],
            }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let settings: serde_json::Value = get("/api/settings").await.json();
        assert_eq!(settings["language"], "ru");
        assert_eq!(settings["sudo_users"], json!([42]));
        assert_eq!(settings["prefixes"], json!([".", "!"]));
        let afk = settings["toggles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["key"] == "handlers.afk.enabled")
            .unwrap()
            .clone();
        assert_eq!(afk["enabled"], true);
        assert!(services.db.get("bogus.key").await.is_null());

        let res = post("/api/settings")
            .json(&json!({ "proxy_url": "http://nope" }))
            .await;
        assert_eq!(res.status_code(), 400);

        let logs = get("/api/logs").await;
        assert_eq!(logs.status_code(), 200);

        // Account endpoints validate input without a connected account.
        let accounts: serde_json::Value = get("/api/accounts").await.json();
        assert_eq!(accounts, json!([]));
        let res = post("/api/accounts/cleanup/preview")
            .json(&json!({ "options": {} }))
            .await;
        assert_eq!(res.status_code(), 400);
        let res = post("/api/accounts/cleanup/preview")
            .json(&json!({ "accounts": [42], "options": { "groups": true } }))
            .await;
        assert_eq!(res.status_code(), 400);
        let res = post("/api/accounts/cleanup/run")
            .json(&json!({ "codes": ["NOPE00"] }))
            .await;
        assert_eq!(res.status_code(), 400);
        let res = post("/api/accounts/bulk")
            .json(&json!({ "action": "explode" }))
            .await;
        assert_eq!(res.status_code(), 400);
        let jobs: serde_json::Value = get("/api/jobs").await.json();
        assert_eq!(jobs, json!([]));

        // Automations CRUD with validation.
        let res = post("/api/automations")
            .json(&json!({ "type": "schedule", "chat": "me", "text": "hi" }))
            .await;
        assert_eq!(res.status_code(), 400, "needs an interval or a time");
        let res = post("/api/automations")
            .json(&json!({ "type": "away_reply", "text": "zz", "from": "22:00", "to": "07:00" }))
            .await;
        assert_eq!(res.status_code(), 200, "{}", res.text());
        let saved: serde_json::Value = res.json();
        let id = saved["rule"]["id"].as_str().unwrap().to_string();
        let rules: serde_json::Value = get("/api/automations").await.json();
        assert_eq!(rules[0]["type"], "away_reply");
        let res = local(&server, "POST", &format!("/api/automations/{id}/enabled"))
            .add_header("cookie", cookie.clone())
            .json(&json!({ "enabled": false }))
            .await;
        assert_eq!(res.status_code(), 200);
        let res = local(&server, "DELETE", &format!("/api/automations/{id}"))
            .add_header("cookie", cookie.clone())
            .await;
        assert_eq!(res.status_code(), 200);
    }
}
