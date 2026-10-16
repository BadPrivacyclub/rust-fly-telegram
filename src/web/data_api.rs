use super::{
    api::{api_error, api_ok},
    AppState,
};
use crate::export::{
    self,
    model::{Options, Rules, Schedule},
};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub async fn export_state(State(state): State<AppState>) -> Response {
    match export::records().await {
        Ok(records) => Json(json!({"rules":export::rules(&state.services).await, "schedules":export::schedules(&state.services).await, "exports":records})).into_response(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn chats(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    let Some(handle) = state.services.accounts.get(id).await else {
        return api_error(StatusCode::NOT_FOUND, "account is not connected");
    };
    match crate::account_tools::dialogs(&handle).await {
        Ok(dialogs) => {
            let mut chats: Vec<serde_json::Value> = dialogs
                .into_iter()
                .map(|d| json!({"id":d.id,"title":d.title}))
                .collect();
            let account = match handle.client.get_me().await {
                Ok(account) => account.id().to_string(),
                Err(e) => return api_error(StatusCode::BAD_GATEWAY, e),
            };
            match crate::anti_delete::export_catalog(state.db(), &account).await {
                Ok(archived) => {
                    for chat in archived {
                        if let Ok(id) = chat.id.parse::<i64>() {
                            if !chats.iter().any(|c| c["id"] == id) {
                                chats
                                    .push(json!({"id":id,"title":chat.title,"archived_only":true}));
                            }
                        }
                    }
                }
                Err(e) => return api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
            }
            Json(chats).into_response()
        }
        Err(e) => api_error(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
pub struct ExportRequest {
    accounts: Vec<i64>,
    options: Option<Options>,
}

pub async fn start_export(
    State(state): State<AppState>,
    Json(body): Json<ExportRequest>,
) -> Response {
    if body.accounts.is_empty() || body.accounts.len() > 100 {
        return api_error(StatusCode::BAD_REQUEST, "select 1 to 100 accounts");
    }
    let rules = export::rules(&state.services).await;
    let mut plans = Vec::new();
    for id in body.accounts {
        if plans.iter().any(|(account, _)| *account == id) {
            continue;
        }
        let options = body.options.clone().unwrap_or_else(|| {
            rules
                .accounts
                .get(&id.to_string())
                .unwrap_or(&rules.defaults)
                .clone()
        });
        if let Err(e) = options.validate() {
            return api_error(StatusCode::BAD_REQUEST, e);
        }
        if state.services.accounts.get(id).await.is_none() {
            return api_error(
                StatusCode::BAD_REQUEST,
                format!("account {id} is not connected"),
            );
        }
        plans.push((id, options));
    }
    let mut jobs = Vec::new();
    for (id, options) in plans {
        match export::start(Arc::clone(&state.services), id, options).await {
            Ok(job) => jobs.push(job),
            Err(e) => return api_error(StatusCode::BAD_REQUEST, e),
        }
    }
    Json(json!({"ok":true,"jobs":jobs})).into_response()
}

pub async fn save_rules(State(state): State<AppState>, Json(rules): Json<Rules>) -> Response {
    for options in std::iter::once(&rules.defaults).chain(rules.accounts.values()) {
        if let Err(e) = options.validate() {
            return api_error(StatusCode::BAD_REQUEST, e);
        }
    }
    match state
        .db()
        .set(
            "export.rules",
            serde_json::to_value(rules).unwrap_or_default(),
        )
        .await
    {
        Ok(()) => api_ok(),
        Err(e) => api_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

pub async fn save_schedules(
    State(state): State<AppState>,
    Json(schedules): Json<Vec<Schedule>>,
) -> Response {
    match export::save_schedules(&state.services, schedules).await {
        Ok(()) => api_ok(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}

pub async fn download(State(_state): State<AppState>, Path(id): Path<String>) -> Response {
    let root = match export::archive_path(&id) {
        Ok(path) => path,
        Err(e) => return api_error(StatusCode::BAD_REQUEST, e),
    };
    if !root.join("manifest.json").exists() {
        return api_error(StatusCode::NOT_FOUND, "export is not ready");
    }
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<std::path::PathBuf> {
        let target =
            std::env::temp_dir().join(format!("fly-export-{}.tar.gz", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> anyhow::Result<()> {
            let file = options.open(&target)?;
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
            let mut tar = tar::Builder::new(encoder);
            tar.follow_symlinks(false);
            tar.append_dir_all("export", &root)?;
            tar.into_inner()?.finish()?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&target);
            return Err(error);
        }
        Ok(target)
    })
    .await;
    match result {
        Ok(Ok(path)) => match tokio::fs::File::open(&path).await {
            Ok(file) => {
                let stream = DeleteAfterStream {
                    inner: Some(tokio_util::io::ReaderStream::new(file)),
                    path,
                };
                (
                    [
                        (header::CONTENT_TYPE, "application/gzip"),
                        (
                            header::CONTENT_DISPOSITION,
                            "attachment; filename=telegram-export.tar.gz",
                        ),
                    ],
                    axum::body::Body::from_stream(stream),
                )
                    .into_response()
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(path).await;
                api_error(StatusCode::INTERNAL_SERVER_ERROR, e)
            }
        },
        other => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cannot package export: {other:?}"),
        ),
    }
}

struct DeleteAfterStream {
    inner: Option<tokio_util::io::ReaderStream<tokio::fs::File>>,
    path: std::path::PathBuf,
}
impl futures_core::Stream for DeleteAfterStream {
    type Item = std::io::Result<bytes::Bytes>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.inner.as_mut() {
            Some(inner) => futures_core::Stream::poll_next(std::pin::Pin::new(inner), cx),
            None => std::task::Poll::Ready(None),
        }
    }
}
impl Drop for DeleteAfterStream {
    fn drop(&mut self) {
        drop(self.inner.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn recovery_state(State(state): State<AppState>) -> Response {
    Json(state.services.recovery.status(&state.services).await).into_response()
}

#[derive(Deserialize)]
pub struct Password {
    password: String,
}
pub async fn unlock(State(state): State<AppState>, Json(body): Json<Password>) -> Response {
    if !state.services.panel_auth.allow_login_attempt().await {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "try again later");
    }
    match state.services.recovery.unlock(body.password).await {
        Ok(()) => api_ok(),
        Err(e) => {
            state.services.panel_auth.record_failed_login().await;
            api_error(StatusCode::BAD_REQUEST, e)
        }
    }
}
pub async fn lock(State(state): State<AppState>) -> Response {
    state.services.recovery.lock().await;
    api_ok()
}
pub async fn recovery_policy(
    State(state): State<AppState>,
    Json(policy): Json<crate::recovery::Policy>,
) -> Response {
    if !std::path::Path::new("keys/recovery.enc").exists()
        && (policy.enabled || policy.accounts.values().any(|v| *v))
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "create the password-protected vault first",
        );
    }
    match state
        .db()
        .set(
            "recovery.policy",
            serde_json::to_value(policy).unwrap_or_default(),
        )
        .await
    {
        Ok(()) => api_ok(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}
pub async fn provision(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match state.services.recovery.provision(&state.services, id).await {
        Ok(()) => api_ok(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}
pub async fn restore(State(state): State<AppState>, Path(id): Path<i64>) -> Response {
    match state.services.recovery.restore(&state.services, id).await {
        Ok(value) => {
            if let (Some(loader), Some(session)) = (&state.loader, value["session"].as_str()) {
                crate::client::spawn_session(Arc::clone(loader), session.into());
            }
            Json(value).into_response()
        }
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}
#[derive(Deserialize)]
pub struct TwoFactor {
    token: String,
    password: String,
}
pub async fn two_factor(State(state): State<AppState>, Json(body): Json<TwoFactor>) -> Response {
    if !state.services.panel_auth.allow_login_attempt().await {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "try again later");
    }
    match state
        .services
        .recovery
        .password(&state.services, &body.token, body.password)
        .await
    {
        Ok(session) => {
            if let Some(loader) = &state.loader {
                crate::client::spawn_session(Arc::clone(loader), session);
            }
            api_ok()
        }
        Err(e) => {
            state.services.panel_auth.record_failed_login().await;
            api_error(StatusCode::BAD_REQUEST, e)
        }
    }
}
#[derive(Deserialize)]
pub struct DeleteAccount {
    confirmation: String,
}
pub async fn delete_account(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<DeleteAccount>,
) -> Response {
    match state
        .services
        .recovery
        .delete(&state.services, id, &body.confirmation)
        .await
    {
        Ok(revoked) => Json(json!({"ok":true,"server_passkey_revoked":revoked})).into_response(),
        Err(e) => api_error(StatusCode::BAD_REQUEST, e),
    }
}
