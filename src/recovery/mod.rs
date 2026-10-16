//! Session recovery with passkeys. For accounts where it is on, I register a passkey with
//! Telegram and keep its private key in the recovery vault. If the session is lost later,
//! the panel signs in again with that passkey; a 2FA password is never stored, so the
//! owner types it when Telegram asks for it.

pub mod vault;
pub mod webauthn;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use grammers_client::client::PasswordToken;
use grammers_client::{tl, Client, InvocationError, SignInError};
use grammers_session::storages::SqliteSession;
use grammers_session::types::{PeerAuth, PeerInfo};
use grammers_session::Session;
use serde::Serialize;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::account_tools::{now_secs, AccountHandle};
use crate::app::Services;
use crate::config::db_key;
use vault::{Secret, Vault};

const DEFAULT_KEY: &str = "recovery.default_enabled";
const ACCOUNT_PREFIX: &str = "recovery.account.";
const STATUS_PREFIX: &str = "recovery.status.";
const LOST_KEY: &str = "recovery.lost";
const KNOWN_PREFIX: &str = "recovery.known.";
/// Telegram refuses sensitive changes from sessions younger than about a day, so I wait this long.
const FRESH_SESSION_SECS: i64 = 24 * 3600 + 300;
const WORKER_TICK: Duration = Duration::from_secs(600);
const RETRY_AFTER_ERROR_SECS: i64 = 6 * 3600;

#[derive(Default)]
pub struct Recovery {
    pending: Mutex<HashMap<i64, PendingRestore>>,
    worker_lock: Mutex<()>,
}

/// A restore I keep while it waits for the 2FA password.
struct PendingRestore {
    client: Client,
    pool: tokio::task::JoinHandle<()>,
    session_file: String,
    hint: Option<String>,
    started: i64,
}

impl Recovery {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountStatus {
    pub user_id: i64,
    pub name: String,
    pub connected: bool,
    pub enabled: bool,
    /// I accept `on`, `off`, or `default`.
    pub mode: String,
    pub state: String,
    pub message: Option<String>,
    pub next_try: Option<i64>,
    pub key_created: Option<i64>,
    pub key_provider: Option<String>,
    pub lost: Option<Value>,
}

pub async fn default_enabled(services: &Services) -> bool {
    services
        .db
        .get(DEFAULT_KEY)
        .await
        .as_bool()
        .unwrap_or(false)
}

pub async fn set_default_enabled(services: &Services, enabled: bool) -> Result<()> {
    services.db.set(DEFAULT_KEY, Value::Bool(enabled)).await
}

async fn account_mode(services: &Services, user_id: i64) -> String {
    services
        .db
        .get(&format!("{ACCOUNT_PREFIX}{user_id}"))
        .await
        .as_str()
        .unwrap_or("default")
        .to_string()
}

pub async fn enabled_for(services: &Services, user_id: i64) -> bool {
    match account_mode(services, user_id).await.as_str() {
        "on" => true,
        "off" => false,
        _ => default_enabled(services).await,
    }
}

async fn set_status(
    services: &Services,
    user_id: i64,
    state: &str,
    message: Option<String>,
    next_try: Option<i64>,
) {
    let value =
        json!({ "state": state, "message": message, "next_try": next_try, "updated": now_secs() });
    let _ = services
        .db
        .set(format!("{STATUS_PREFIX}{user_id}"), value)
        .await;
}

/// I turn recovery on, off, or back to the default for one account.
pub async fn set_account_mode(services: &Arc<Services>, user_id: i64, mode: &str) -> Result<()> {
    let key = format!("{ACCOUNT_PREFIX}{user_id}");
    match mode {
        "on" | "off" => services.db.set(key, Value::String(mode.into())).await?,
        "default" => services.db.remove(&key).await?,
        _ => bail!("mode must be on, off, or default"),
    }
    if !enabled_for(services, user_id).await {
        revoke_key(services, user_id).await?;
    }
    kick(services);
    Ok(())
}

/// I remove the passkey from Telegram when the account is online, and from the vault.
async fn revoke_key(services: &Services, user_id: i64) -> Result<()> {
    let Some(mut vault) = Vault::load().await? else {
        return Ok(());
    };
    let Some(entry) = vault.remove(user_id) else {
        return Ok(());
    };
    if let Some(handle) = services.accounts.get(user_id).await {
        if let Err(error) = handle
            .client
            .invoke(&tl::functions::account::DeletePasskey {
                id: entry.passkey_id.clone(),
            })
            .await
        {
            warn!("could not delete the passkey of {}: {error}", entry.name);
        }
    }
    vault.save().await?;
    set_status(services, user_id, "off", None, None).await;
    Ok(())
}

pub async fn statuses(services: &Services) -> Vec<AccountStatus> {
    let vault = Vault::load().await.ok().flatten();
    let lost = lost_sessions(services).await;
    let mut ids = services
        .accounts
        .list()
        .await
        .into_iter()
        .map(|h| (h.user_id, h.name, true))
        .collect::<Vec<_>>();
    if let Some(vault) = &vault {
        for entry in vault.entries.values() {
            if !ids.iter().any(|(id, _, _)| *id == entry.user_id) {
                ids.push((entry.user_id, entry.name.clone(), false));
            }
        }
    }
    for (key, value) in lost.as_object().into_iter().flatten() {
        if let Ok(id) = key.parse::<i64>() {
            if !ids.iter().any(|(known, _, _)| *known == id) {
                ids.push((
                    id,
                    value["name"].as_str().unwrap_or("Account").to_string(),
                    false,
                ));
            }
        }
    }
    let mut out = Vec::new();
    for (user_id, name, connected) in ids {
        let status = services.db.get(&format!("{STATUS_PREFIX}{user_id}")).await;
        let entry = vault
            .as_ref()
            .and_then(|v| v.entries.get(&user_id.to_string()));
        let enabled = enabled_for(services, user_id).await;
        let state = if entry.is_some() {
            "ready".to_string()
        } else if !enabled {
            "off".to_string()
        } else if vault.is_none() {
            "needs_vault".to_string()
        } else {
            status["state"].as_str().unwrap_or("waiting").to_string()
        };
        out.push(AccountStatus {
            user_id,
            name,
            connected,
            enabled,
            mode: account_mode(services, user_id).await,
            state,
            message: status["message"].as_str().map(str::to_string),
            next_try: status["next_try"].as_i64(),
            key_created: entry.map(|e| e.created_at),
            key_provider: entry
                .map(|e| e.provider.clone())
                .filter(|provider| !provider.is_empty()),
            lost: lost.get(user_id.to_string()).cloned(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// I remember which account a session file belongs to, so I can name a later loss.
pub async fn remember_session(services: &Services, handle: &AccountHandle) {
    let value = json!({ "user_id": handle.user_id, "name": handle.name });
    let key = format!("{KNOWN_PREFIX}{}", handle.session_file);
    if services.db.get(&key).await != value {
        let _ = services.db.set(key, value).await;
    }
    let lost = lost_sessions(services).await;
    if lost.get(handle.user_id.to_string()).is_some() {
        let mut lost = lost;
        if let Some(map) = lost.as_object_mut() {
            map.remove(&handle.user_id.to_string());
        }
        let _ = services.db.set(LOST_KEY, lost).await;
    }
}

pub async fn lost_sessions(services: &Services) -> Value {
    let value = services.db.get(LOST_KEY).await;
    if value.is_object() {
        value
    } else {
        json!({})
    }
}

/// I record that a session stopped working. I return false when the file belongs to no
/// account I have seen, which happens for a brand-new session that was never signed in.
pub async fn mark_lost(services: &Services, session_file: &str, reason: &str) -> bool {
    let known = services
        .db
        .get(&format!("{KNOWN_PREFIX}{session_file}"))
        .await;
    let Some(user_id) = known["user_id"].as_i64() else {
        return false;
    };
    let name = known["name"].as_str().unwrap_or("Account").to_string();
    let mut lost = lost_sessions(services).await;
    lost[user_id.to_string()] = json!({
        "user_id": user_id,
        "name": name,
        "session_file": session_file,
        "lost_at": now_secs(),
        "reason": reason,
    });
    let _ = services.db.set(LOST_KEY, lost).await;
    warn!("session of {name} was lost: {reason}");
    let has_key = Vault::load()
        .await
        .ok()
        .flatten()
        .is_some_and(|v| v.entries.contains_key(&user_id.to_string()));
    let html = format!(
        "<b>Session lost</b>: {}\n{}\n\n{}",
        crate::notify::escape_html(&name),
        crate::notify::escape_html(reason),
        if has_key {
            "An access key is saved; restore the session from the panel, Accounts, Session recovery."
        } else {
            "No access key was saved for this account; sign in again from the panel."
        }
    );
    services
        .notifier
        .send(crate::notify::Topic::Errors, &html)
        .await;
    true
}

pub fn is_session_lost(error: &InvocationError) -> bool {
    matches!(error, InvocationError::Rpc(rpc) if rpc.code == 401
        || matches!(rpc.name.as_str(), "AUTH_KEY_UNREGISTERED" | "SESSION_REVOKED" | "SESSION_EXPIRED" | "USER_DEACTIVATED" | "USER_DEACTIVATED_BAN" | "AUTH_KEY_INVALID"))
}

fn kick(services: &Arc<Services>) {
    let services = Arc::clone(services);
    tokio::spawn(async move { run_worker_once(&services).await });
}

pub fn spawn(services: Arc<Services>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(45)).await;
        let mut interval = tokio::time::interval(WORKER_TICK);
        loop {
            interval.tick().await;
            run_worker_once(&services).await;
        }
    });
}

/// I create missing access keys for accounts that want one and are allowed to make it.
async fn run_worker_once(services: &Arc<Services>) {
    let Ok(_guard) = services.recovery.worker_lock.try_lock() else {
        return;
    };
    let vault = match Vault::load().await {
        Ok(Some(vault)) => vault,
        Ok(None) => return,
        Err(error) => {
            warn!("recovery vault unreadable: {error:#}");
            return;
        }
    };
    for handle in services.accounts.list().await {
        if !enabled_for(services, handle.user_id).await
            || vault.entries.contains_key(&handle.user_id.to_string())
        {
            continue;
        }
        let status = services
            .db
            .get(&format!("{STATUS_PREFIX}{}", handle.user_id))
            .await;
        if status["next_try"]
            .as_i64()
            .is_some_and(|next| next > now_secs())
        {
            continue;
        }
        if let Err(error) = create_key(services, &handle).await {
            warn!("access key for {}: {error:#}", handle.name);
        }
    }
}

async fn session_created_at(client: &Client) -> Option<i64> {
    let tl::enums::account::Authorizations::Authorizations(list) = client
        .invoke(&tl::functions::account::GetAuthorizations {})
        .await
        .ok()?;
    list.authorizations.iter().find_map(|auth| {
        let tl::enums::Authorization::Authorization(a) = auth;
        a.current.then_some(i64::from(a.date_created))
    })
}

async fn create_key(services: &Arc<Services>, handle: &AccountHandle) -> Result<()> {
    let user_id = handle.user_id;
    if let Some(created) = session_created_at(&handle.client).await {
        let ready_at = created + FRESH_SESSION_SECS;
        if ready_at > now_secs() {
            set_status(
                services,
                user_id,
                "waiting",
                Some("Telegram does not let a session younger than 24 hours add an access key; I will create it automatically.".into()),
                Some(ready_at),
            )
            .await;
            return Ok(());
        }
    }
    let options = match handle
        .client
        .invoke(&tl::functions::account::InitPasskeyRegistration {})
        .await
    {
        Ok(tl::enums::account::PasskeyRegistrationOptions::Options(options)) => {
            let tl::enums::DataJson::Json(data) = options.options;
            data.data
        }
        Err(error) => return registration_failed(services, user_id, error).await,
    };
    let registration = webauthn::register(&options)?;
    let credential =
        tl::enums::InputPasskeyCredential::PublicKey(tl::types::InputPasskeyCredentialPublicKey {
            id: registration.credential.credential_id.clone(),
            raw_id: registration.credential.credential_id.clone(),
            response: tl::enums::InputPasskeyResponse::Register(
                tl::types::InputPasskeyResponseRegister {
                    client_data: tl::enums::DataJson::Json(tl::types::DataJson {
                        data: registration.client_data_json.clone(),
                    }),
                    attestation_data: registration.attestation_object.clone(),
                },
            ),
        });
    let passkey = match handle
        .client
        .invoke(&tl::functions::account::RegisterPasskey { credential })
        .await
    {
        Ok(tl::enums::Passkey::Passkey(passkey)) => passkey,
        Err(error) => return registration_failed(services, user_id, error).await,
    };
    let me = handle.client.get_me().await.ok();
    let secret = Secret {
        credential: registration.credential,
        phone: me.as_ref().and_then(|u| u.phone().map(|p| format!("+{p}"))),
    };
    let saved = async {
        let mut vault = Vault::load().await?.context("the vault disappeared")?;
        vault.put(
            user_id,
            handle.name.clone(),
            me.as_ref().and_then(|u| u.username().map(str::to_string)),
            passkey.id.clone(),
            handle.session_file.clone(),
            &secret,
            now_secs(),
        )?;
        vault.save().await
    }
    .await;
    if let Err(error) = saved {
        let _ = handle
            .client
            .invoke(&tl::functions::account::DeletePasskey { id: passkey.id })
            .await;
        set_status(
            services,
            user_id,
            "error",
            Some(format!("Saving the key failed: {error:#}")),
            Some(now_secs() + RETRY_AFTER_ERROR_SECS),
        )
        .await;
        return Err(error);
    }
    set_status(services, user_id, "ready", None, None).await;
    info!("access key created for {}", handle.name);
    services
        .notifier
        .send(
            crate::notify::Topic::Jobs,
            &format!(
                "<b>Access key saved</b> for {}. A lost session can now be restored from the panel.",
                crate::notify::escape_html(&handle.name)
            ),
        )
        .await;
    Ok(())
}

async fn registration_failed(
    services: &Services,
    user_id: i64,
    error: InvocationError,
) -> Result<()> {
    let (state, message, retry) = match &error {
        InvocationError::Rpc(rpc)
            if rpc.name.starts_with("FRESH_") || rpc.name.contains("TOO_FRESH") =>
        {
            (
                "waiting",
                "Telegram does not allow this yet for a new session; I will try again later."
                    .to_string(),
                RETRY_AFTER_ERROR_SECS,
            )
        }
        InvocationError::Rpc(rpc) if rpc.name.starts_with("FLOOD") => (
            "waiting",
            "Telegram limited the request rate; I will try again later.".to_string(),
            i64::from(rpc.value.unwrap_or(3600)).max(600),
        ),
        InvocationError::Rpc(rpc) if rpc.name.starts_with("PASSKEY") => (
            "error",
            format!("Telegram refused the access key ({})", rpc.name),
            RETRY_AFTER_ERROR_SECS * 4,
        ),
        other => (
            "error",
            format!("Creating the access key failed: {other}"),
            RETRY_AFTER_ERROR_SECS,
        ),
    };
    set_status(
        services,
        user_id,
        state,
        Some(message),
        Some(now_secs() + retry),
    )
    .await;
    Err(error.into())
}

pub async fn create_vault(services: &Arc<Services>, password: &str) -> Result<()> {
    Vault::create(password, now_secs()).await?;
    kick(services);
    Ok(())
}

pub async fn change_vault_password(current: &str, next: &str) -> Result<()> {
    let mut vault = Vault::load()
        .await?
        .context("the vault does not exist yet")?;
    vault.change_password(current, next)?;
    vault.save().await
}

pub async fn vault_info() -> Value {
    match Vault::load().await {
        Ok(Some(vault)) => {
            json!({ "exists": true, "created_at": vault.created_at, "keys": vault.entries.len() })
        }
        Ok(None) => json!({ "exists": false }),
        Err(error) => json!({ "exists": true, "error": error.to_string() }),
    }
}

#[derive(Debug, Serialize)]
pub struct RestoreOutcome {
    pub ok: bool,
    pub need_password: bool,
    pub hint: Option<String>,
}

/// I sign in with the stored passkey. On success the session file works again and the
/// account reconnects; when 2FA is on, my outcome asks for the password.
pub async fn restore(
    services: &Arc<Services>,
    loader: &Arc<crate::loader::Loader>,
    user_id: i64,
    vault_password: &str,
) -> Result<RestoreOutcome> {
    if services.accounts.get(user_id).await.is_some() {
        bail!("this account is connected; nothing to restore");
    }
    let mut vault = Vault::load().await?.context("there is no recovery vault")?;
    let entry = vault
        .entries
        .get(&user_id.to_string())
        .cloned()
        .context("this account has no saved access key")?;
    let mut secret = vault.open(user_id, vault_password)?;
    if let Some(old) = services.recovery.pending.lock().await.remove(&user_id) {
        old.pool.abort();
    }

    let lost = lost_sessions(services).await;
    let session_file = lost[user_id.to_string()]["session_file"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| entry.session_file.clone());
    retire_session_file(&session_file).await?;

    let api_id: i32 = services
        .db
        .get(db_key::API_ID)
        .await
        .as_str()
        .context("API ID is not configured")?
        .parse()
        .context("API ID must be a number")?;
    let api_hash = services
        .db
        .get(db_key::API_HASH)
        .await
        .as_str()
        .context("API hash is not configured")?
        .to_string();
    let proxy = services
        .db
        .get(db_key::PROXY_URL)
        .await
        .as_str()
        .map(str::to_string)
        .filter(|p| !p.is_empty());
    // The restored session is the same account, so it keeps the profile it had.
    let profile = crate::device_profile::for_session(&services.db, &session_file).await;
    let (client, _updates, pool, session) = crate::client::auth::open_pool_with_session(
        api_id,
        &session_file,
        proxy,
        &profile,
    )
    .await?;

    let result = passkey_sign_in(&client, &session, &mut secret, api_id, &api_hash).await;
    vault.update_secret(user_id, &secret)?;
    vault.save().await?;
    match result {
        Ok(()) => {
            pool.abort();
            finish_restore(services, loader, user_id, &session_file).await;
            Ok(RestoreOutcome {
                ok: true,
                need_password: false,
                hint: None,
            })
        }
        Err(SignInStep::Password) => {
            let hint = PasswordToken::new(password_info(&client).await?)
                .hint()
                .map(str::to_string);
            services.recovery.pending.lock().await.insert(
                user_id,
                PendingRestore {
                    client,
                    pool,
                    session_file,
                    hint: hint.clone(),
                    started: now_secs(),
                },
            );
            Ok(RestoreOutcome {
                ok: false,
                need_password: true,
                hint,
            })
        }
        Err(SignInStep::Failed(error)) => {
            pool.abort();
            Err(error)
        }
    }
}

enum SignInStep {
    Password,
    Failed(anyhow::Error),
}

async fn password_info(client: &Client) -> Result<tl::types::account::Password> {
    let tl::enums::account::Password::Password(info) = client
        .invoke(&tl::functions::account::GetPassword {})
        .await?;
    Ok(info)
}

async fn passkey_sign_in(
    client: &Client,
    session: &Arc<SqliteSession>,
    secret: &mut Secret,
    api_id: i32,
    api_hash: &str,
) -> std::result::Result<(), SignInStep> {
    let fail = |error: anyhow::Error| SignInStep::Failed(error);
    let options = match client
        .invoke(&tl::functions::auth::InitPasskeyLogin {
            api_id,
            api_hash: api_hash.to_string(),
        })
        .await
    {
        Ok(tl::enums::auth::PasskeyLoginOptions::Options(options)) => {
            let tl::enums::DataJson::Json(data) = options.options;
            data.data
        }
        Err(error) => {
            return Err(fail(anyhow::anyhow!(
                "Telegram did not start a passkey login: {error}"
            )))
        }
    };
    let assertion = webauthn::assert(&mut secret.credential, &options).map_err(fail)?;
    let credential =
        tl::enums::InputPasskeyCredential::PublicKey(tl::types::InputPasskeyCredentialPublicKey {
            id: secret.credential.credential_id.clone(),
            raw_id: secret.credential.credential_id.clone(),
            response: tl::enums::InputPasskeyResponse::Login(
                tl::types::InputPasskeyResponseLogin {
                    client_data: tl::enums::DataJson::Json(tl::types::DataJson {
                        data: assertion.client_data_json,
                    }),
                    authenticator_data: assertion.authenticator_data,
                    signature: assertion.signature,
                    user_handle: assertion.user_handle,
                },
            ),
        });
    let mut request = tl::functions::auth::FinishPasskeyLogin {
        credential,
        from_dc_id: None,
        from_auth_key_id: None,
    };
    for _ in 0..2 {
        match client.invoke(&request).await {
            Ok(tl::enums::auth::Authorization::Authorization(auth)) => {
                cache_self(session, &auth).await;
                return Ok(());
            }
            Ok(tl::enums::auth::Authorization::SignUpRequired(_)) => {
                return Err(fail(anyhow::anyhow!(
                    "Telegram says this account no longer exists"
                )));
            }
            Err(InvocationError::Rpc(rpc)) if rpc.name == "SESSION_PASSWORD_NEEDED" => {
                return Err(SignInStep::Password);
            }
            Err(InvocationError::Rpc(rpc))
                if request.from_dc_id.is_none()
                    && matches!(
                        rpc.name.as_str(),
                        "USER_MIGRATE" | "PHONE_MIGRATE" | "NETWORK_MIGRATE"
                    ) =>
            {
                let new_dc = rpc.value.unwrap_or(0) as i32;
                let old_dc = session.home_dc_id();
                let auth_key_id = session
                    .dc_option(old_dc)
                    .and_then(|option| option.auth_key)
                    .map(|key| auth_key_id(&key));
                session.set_home_dc_id(new_dc).await;
                request.from_dc_id = Some(old_dc);
                request.from_auth_key_id = auth_key_id;
            }
            Err(error) => {
                return Err(fail(anyhow::anyhow!(
                    "Telegram rejected the access key: {error}"
                )));
            }
        }
    }
    Err(fail(anyhow::anyhow!(
        "Telegram kept moving the login to another data center"
    )))
}

fn auth_key_id(key: &[u8; 256]) -> i64 {
    let digest = Sha1::digest(key);
    i64::from_le_bytes(
        digest[12..20]
            .try_into()
            .expect("SHA-1 digests are 20 bytes"),
    )
}

async fn cache_self(session: &Arc<SqliteSession>, auth: &tl::types::auth::Authorization) {
    if let tl::enums::User::User(user) = &auth.user {
        session
            .cache_peer(&PeerInfo::User {
                id: user.id,
                auth: user.access_hash.map(PeerAuth::from_hash),
                bot: Some(user.bot),
                is_self: Some(true),
            })
            .await;
    }
}

pub async fn submit_password(
    services: &Arc<Services>,
    loader: &Arc<crate::loader::Loader>,
    user_id: i64,
    password: &str,
) -> Result<RestoreOutcome> {
    let pending = services
        .recovery
        .pending
        .lock()
        .await
        .remove(&user_id)
        .context("no restore is waiting for a password; start it again")?;
    if now_secs() - pending.started > 15 * 60 {
        pending.pool.abort();
        bail!("the restore took too long; start it again");
    }
    let token = PasswordToken::new(password_info(&pending.client).await?);
    match pending
        .client
        .check_password(token, password.as_bytes())
        .await
    {
        Ok(_) => {
            pending.pool.abort();
            finish_restore(services, loader, user_id, &pending.session_file).await;
            Ok(RestoreOutcome {
                ok: true,
                need_password: false,
                hint: None,
            })
        }
        Err(SignInError::InvalidPassword(token)) => {
            let hint = token.hint().map(str::to_string).or(pending.hint.clone());
            services.recovery.pending.lock().await.insert(
                user_id,
                PendingRestore {
                    hint: hint.clone(),
                    ..pending
                },
            );
            bail!(
                "wrong 2FA password{}",
                hint.map(|h| format!(" (hint: {h})")).unwrap_or_default()
            )
        }
        Err(error) => {
            pending.pool.abort();
            Err(anyhow::anyhow!("2FA check failed: {error}"))
        }
    }
}

async fn finish_restore(
    services: &Arc<Services>,
    loader: &Arc<crate::loader::Loader>,
    user_id: i64,
    session_file: &str,
) {
    let mut lost = lost_sessions(services).await;
    if let Some(map) = lost.as_object_mut() {
        map.remove(&user_id.to_string());
    }
    let _ = services.db.set(LOST_KEY, lost).await;
    let _ = crate::client::auth::remember_session_file(&services.db, session_file).await;
    info!("session restored with the access key: {session_file}");
    crate::client::spawn_session(Arc::clone(loader), session_file.to_string());
}

/// I move a dead session file aside so the restore starts from a clean one.
async fn retire_session_file(session_file: &str) -> Result<()> {
    let path = std::path::Path::new(session_file);
    if path.exists() {
        let mut retired = path.as_os_str().to_os_string();
        retired.push(format!(".revoked-{}", now_secs()));
        tokio::fs::rename(path, retired).await?;
    }
    Ok(())
}

/// Deletes everything I keep for an account: session files, access key, anti-delete archive,
/// export rules, and optionally its exports.
pub async fn purge(services: &Arc<Services>, user_id: i64, with_exports: bool) -> Result<Value> {
    if services.accounts.get(user_id).await.is_some() {
        bail!("this account is connected; sign it out in Telegram first or wait until the session is lost");
    }
    if let Some(pending) = services.recovery.pending.lock().await.remove(&user_id) {
        pending.pool.abort();
    }
    let lost = lost_sessions(services).await;
    let mut session_files = Vec::new();
    if let Some(file) = lost[user_id.to_string()]["session_file"].as_str() {
        session_files.push(file.to_string());
    }
    for key in services.db.keys_with_prefix(KNOWN_PREFIX).await {
        if services.db.get(&key).await["user_id"].as_i64() == Some(user_id) {
            session_files.push(key.trim_start_matches(KNOWN_PREFIX).to_string());
            services.db.remove(&key).await?;
        }
    }
    let mut vault_removed = false;
    if let Some(mut vault) = Vault::load().await? {
        if let Some(entry) = vault.remove(user_id) {
            session_files.push(entry.session_file);
            vault.save().await?;
            vault_removed = true;
        }
    }
    session_files.sort();
    session_files.dedup();
    let mut files_removed = 0;
    for file in &session_files {
        files_removed += remove_session_files(file).await;
    }
    forget_session_files(services, &session_files).await?;
    let archived = crate::anti_delete::purge_account(&services.db, user_id).await?;
    crate::export::rules::remove_account(&services.db, user_id).await?;
    if with_exports {
        crate::export::remove_account(user_id).await?;
    }
    let mut lost = lost;
    if let Some(map) = lost.as_object_mut() {
        map.remove(&user_id.to_string());
    }
    services.db.set(LOST_KEY, lost).await?;
    for key in [
        format!("{STATUS_PREFIX}{user_id}"),
        format!("{ACCOUNT_PREFIX}{user_id}"),
    ] {
        services.db.remove(&key).await?;
    }
    info!("account {user_id} was deleted from fly-telegram");
    Ok(json!({
        "session_files": files_removed,
        "access_key": vault_removed,
        "deleted_messages": archived,
        "exports": with_exports,
    }))
}

async fn remove_session_files(session_file: &str) -> usize {
    let mut removed = 0;
    let path = std::path::Path::new(session_file);
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_string()) else {
        return 0;
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let file = entry.file_name().to_string_lossy().to_string();
        if (file == name
            || file.starts_with(&format!("{name}."))
            || file.starts_with(&format!("{name}-")))
            && tokio::fs::remove_file(entry.path()).await.is_ok()
        {
            removed += 1;
        }
    }
    removed
}

async fn forget_session_files(services: &Services, files: &[String]) -> Result<()> {
    let normalized = files
        .iter()
        .map(|f| f.replace('\\', "/"))
        .collect::<Vec<_>>();
    if let Some(list) = services.db.get(db_key::SESSION_FILES).await.as_array() {
        let kept = list
            .iter()
            .filter(|value| {
                value
                    .as_str()
                    .is_none_or(|v| !normalized.contains(&v.replace('\\', "/")))
            })
            .cloned()
            .collect::<Vec<_>>();
        services
            .db
            .set(db_key::SESSION_FILES, Value::Array(kept))
            .await?;
    }
    if let Some(current) = services.db.get(db_key::SESSION_FILE).await.as_str() {
        if normalized.contains(&current.replace('\\', "/")) {
            services.db.remove(db_key::SESSION_FILE).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_key_id_uses_the_low_sha1_bytes() {
        let key = [7u8; 256];
        let digest = Sha1::digest(key);
        let expected = i64::from_le_bytes(digest[12..20].try_into().unwrap());
        assert_eq!(auth_key_id(&key), expected);
    }

    #[test]
    fn lost_errors_are_recognized() {
        let rpc = |name: &str, code: i32| {
            InvocationError::Rpc(grammers_client::sender::RpcError {
                code,
                name: name.into(),
                value: None,
                caused_by: None,
            })
        };
        assert!(is_session_lost(&rpc("AUTH_KEY_UNREGISTERED", 401)));
        assert!(is_session_lost(&rpc("SESSION_REVOKED", 401)));
        assert!(!is_session_lost(&rpc("FLOOD_WAIT", 420)));
    }
}
