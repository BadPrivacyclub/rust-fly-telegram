//! Account management: connected-account registry, dialog and contact listings,
//! smart cleanup, and bulk actions (read all, archive, mute, export, profile).
//!
//! Every bulk operation runs as a [`Job`] with pacing between requests and FLOOD_WAIT
//! handling, so mass actions stay within Telegram's limits.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use grammers_client::peer::Peer;
use grammers_client::{tl, Client};
use grammers_mtsender::InvocationError;
use grammers_session::types::PeerRef;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

use crate::jobs::{Job, JobStatus};
use crate::telegram::StoredPeer;

/// Delay between destructive requests. Slow on purpose: mass leaving or deleting is the
/// kind of activity Telegram rate-limits hardest.
const ACTION_DELAY: Duration = Duration::from_millis(1500);
const FLOOD_RETRIES: usize = 5;
const MAX_FLOOD_WAIT_SECS: u32 = 900;
const PLAN_TTL: Duration = Duration::from_secs(15 * 60);
const ARCHIVE_FOLDER: i32 = 1;

#[derive(Clone)]
pub struct AccountHandle {
    pub client: Client,
    pub session_file: String,
    pub user_id: i64,
    pub name: String,
}

struct PendingPlan {
    user_id: i64,
    options: CleanupOptions,
    plan: CleanupPlan,
    created: Instant,
}

/// Connected accounts and short-lived cleanup confirmations.
#[derive(Default)]
pub struct AccountRegistry {
    accounts: RwLock<HashMap<i64, AccountHandle>>,
    plans: Mutex<HashMap<String, PendingPlan>>,
}

impl AccountRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, handle: AccountHandle) {
        self.accounts.write().await.insert(handle.user_id, handle);
    }

    pub async fn remove(&self, user_id: i64) {
        self.accounts.write().await.remove(&user_id);
    }

    pub async fn get(&self, user_id: i64) -> Option<AccountHandle> {
        self.accounts.read().await.get(&user_id).cloned()
    }

    pub async fn by_session(&self, session_file: &str) -> Option<AccountHandle> {
        self.accounts
            .read()
            .await
            .values()
            .find(|handle| handle.session_file == session_file)
            .cloned()
    }

    pub async fn list(&self) -> Vec<AccountHandle> {
        let mut list = self
            .accounts
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    /// Resolves IDs from the panel; an empty list means "all connected accounts".
    pub async fn select(&self, ids: &[i64]) -> Result<Vec<AccountHandle>> {
        if ids.is_empty() {
            return Ok(self.list().await);
        }
        let mut out = Vec::new();
        for id in ids {
            out.push(
                self.get(*id)
                    .await
                    .with_context(|| format!("account {id} is not connected"))?,
            );
        }
        Ok(out)
    }

    /// Stores a previewed cleanup and returns the confirmation code needed to run it.
    pub async fn store_plan(
        &self,
        user_id: i64,
        options: CleanupOptions,
        plan: CleanupPlan,
    ) -> String {
        let code = confirmation_code();
        let mut plans = self.plans.lock().await;
        plans.retain(|_, pending| pending.created.elapsed() < PLAN_TTL);
        plans.insert(
            code.clone(),
            PendingPlan {
                user_id,
                options,
                plan,
                created: Instant::now(),
            },
        );
        code
    }

    pub async fn take_plan(&self, code: &str) -> Option<(i64, CleanupOptions, CleanupPlan)> {
        let mut plans = self.plans.lock().await;
        let pending = plans.remove(&code.trim().to_uppercase())?;
        if pending.created.elapsed() >= PLAN_TTL {
            return None;
        }
        Some((pending.user_id, pending.options, pending.plan))
    }
}

fn confirmation_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DialogKind {
    Saved,
    User,
    Bot,
    Group,
    Supergroup,
    Channel,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DialogInfo {
    pub id: i64,
    pub peer: StoredPeer,
    pub kind: DialogKind,
    pub title: String,
    pub username: Option<String>,
    pub last_date: Option<i64>,
    pub unread: i32,
    pub mentions: i32,
    pub pinned: bool,
    pub archived: bool,
    pub muted: bool,
    pub owner: bool,
    pub admin: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContactInfo {
    pub id: i64,
    pub access_hash: i64,
    pub name: String,
    pub username: Option<String>,
    pub phone: Option<String>,
}

pub async fn dialogs(handle: &AccountHandle) -> Result<Vec<DialogInfo>> {
    let now = now_secs() as i32;
    let mut out = Vec::new();
    let mut iter = handle.client.iter_dialogs();
    while let Some(dialog) = iter.next().await? {
        let tl::enums::Dialog::Dialog(raw) = &dialog.raw else {
            continue;
        };
        let peer = dialog.peer();
        let (kind, owner, admin) = match peer {
            Peer::User(user) => {
                let kind = if user.is_self() || user.id().bot_api_dialog_id() == handle.user_id {
                    DialogKind::Saved
                } else if user.is_bot() {
                    DialogKind::Bot
                } else {
                    DialogKind::User
                };
                (kind, false, false)
            }
            Peer::Group(group) => match &group.raw {
                tl::enums::Chat::Chat(chat) => {
                    (DialogKind::Group, chat.creator, chat.admin_rights.is_some())
                }
                tl::enums::Chat::Channel(channel) => (
                    DialogKind::Supergroup,
                    channel.creator,
                    channel.admin_rights.is_some(),
                ),
                _ => (DialogKind::Group, false, false),
            },
            Peer::Channel(channel) => (
                DialogKind::Channel,
                channel.raw.creator,
                channel.raw.admin_rights.is_some(),
            ),
        };
        let muted = match &raw.notify_settings {
            tl::enums::PeerNotifySettings::Settings(settings) => {
                settings.mute_until.is_some_and(|until| until > now)
            }
        };
        let title = match peer {
            Peer::User(user) => {
                let name = user.full_name();
                if name.trim().is_empty() {
                    "Deleted account".to_string()
                } else {
                    name
                }
            }
            other => other.name().unwrap_or("Untitled").to_string(),
        };
        out.push(DialogInfo {
            id: dialog.peer_id().bot_api_dialog_id(),
            peer: StoredPeer::from_ref(dialog.peer_ref()),
            kind,
            title,
            username: peer.username().map(str::to_string),
            last_date: dialog.last_message.as_ref().map(|m| m.date().timestamp()),
            unread: raw.unread_count,
            mentions: raw.unread_mentions_count,
            pinned: raw.pinned,
            archived: raw.folder_id == Some(ARCHIVE_FOLDER),
            muted,
            owner,
            admin,
        });
    }
    Ok(out)
}

pub async fn contacts(handle: &AccountHandle) -> Result<Vec<ContactInfo>> {
    let result = handle
        .client
        .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
        .await?;
    let tl::enums::contacts::Contacts::Contacts(contacts) = result else {
        return Ok(Vec::new());
    };
    Ok(contacts
        .users
        .into_iter()
        .filter_map(|user| match user {
            tl::enums::User::User(user) => Some(ContactInfo {
                id: user.id,
                access_hash: user.access_hash.unwrap_or_default(),
                name: [
                    user.first_name.unwrap_or_default(),
                    user.last_name.unwrap_or_default(),
                ]
                .join(" ")
                .trim()
                .to_string(),
                username: user.username,
                phone: user.phone,
            }),
            tl::enums::User::Empty(_) => None,
        })
        .collect())
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DialogCounts {
    pub users: usize,
    pub bots: usize,
    pub groups: usize,
    pub channels: usize,
    pub unread: i64,
    pub archived: usize,
    pub contacts: usize,
}

pub async fn counts(handle: &AccountHandle) -> Result<DialogCounts> {
    let list = dialogs(handle).await?;
    let mut counts = DialogCounts {
        contacts: contacts(handle).await.map(|c| c.len()).unwrap_or(0),
        ..DialogCounts::default()
    };
    for dialog in &list {
        match dialog.kind {
            DialogKind::User => counts.users += 1,
            DialogKind::Bot => counts.bots += 1,
            DialogKind::Group | DialogKind::Supergroup => counts.groups += 1,
            DialogKind::Channel => counts.channels += 1,
            DialogKind::Saved => {}
        }
        counts.unread += i64::from(dialog.unread);
        if dialog.archived {
            counts.archived += 1;
        }
    }
    Ok(counts)
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CleanupOptions {
    #[serde(default)]
    pub contacts: bool,
    #[serde(default)]
    pub groups: bool,
    #[serde(default)]
    pub channels: bool,
    #[serde(default)]
    pub private_chats: bool,
    #[serde(default)]
    pub bots: bool,
    /// Also delete private chat history for the other side.
    #[serde(default)]
    pub revoke: bool,
    /// Only chats without activity for this many days.
    #[serde(default)]
    pub inactive_days: Option<u32>,
    /// Chat or user IDs (Bot API format) that are never touched.
    #[serde(default)]
    pub keep_ids: Vec<i64>,
    /// Usernames (without @) that are never touched.
    #[serde(default)]
    pub keep_usernames: Vec<String>,
    /// Skip groups and channels where this account is an admin.
    #[serde(default = "yes")]
    pub keep_admin: bool,
    #[serde(default = "yes")]
    pub keep_pinned: bool,
    /// Only process archived chats.
    #[serde(default)]
    pub archived_only: bool,
}

impl Default for CleanupOptions {
    fn default() -> Self {
        Self {
            contacts: false,
            groups: false,
            channels: false,
            private_chats: false,
            bots: false,
            revoke: false,
            inactive_days: None,
            keep_ids: Vec::new(),
            keep_usernames: Vec::new(),
            keep_admin: true,
            keep_pinned: true,
            archived_only: false,
        }
    }
}

impl CleanupOptions {
    pub fn is_empty(&self) -> bool {
        !(self.contacts || self.groups || self.channels || self.private_chats || self.bots)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CleanupPlan {
    pub account_id: i64,
    pub account_name: String,
    pub contacts: Vec<ContactInfo>,
    pub dialogs: Vec<DialogInfo>,
    /// Dialogs matching the selected kinds that the filters kept.
    pub kept: usize,
    /// Owned groups and channels are never left automatically.
    pub owned_skipped: usize,
}

impl CleanupPlan {
    pub fn count(&self, kind: DialogKind) -> usize {
        self.dialogs.iter().filter(|d| d.kind == kind).count()
    }

    pub fn total(&self) -> usize {
        self.contacts.len() + self.dialogs.len()
    }
}

/// Pure selection logic, separated so it can be tested without Telegram.
pub fn select_dialogs(
    dialogs: &[DialogInfo],
    options: &CleanupOptions,
    now: i64,
) -> (Vec<DialogInfo>, usize, usize) {
    let keep_usernames = options
        .keep_usernames
        .iter()
        .map(|u| u.trim_start_matches('@').to_lowercase())
        .collect::<Vec<_>>();
    let mut selected = Vec::new();
    let mut kept = 0;
    let mut owned = 0;
    for dialog in dialogs {
        let wanted = match dialog.kind {
            DialogKind::Saved => false,
            DialogKind::User => options.private_chats,
            DialogKind::Bot => options.bots,
            DialogKind::Group | DialogKind::Supergroup => options.groups,
            DialogKind::Channel => options.channels,
        };
        if !wanted {
            continue;
        }
        if dialog.owner {
            owned += 1;
            continue;
        }
        let protected = options.keep_ids.contains(&dialog.id)
            || dialog
                .username
                .as_ref()
                .is_some_and(|u| keep_usernames.contains(&u.to_lowercase()))
            || (options.keep_admin && dialog.admin)
            || (options.keep_pinned && dialog.pinned)
            || (options.archived_only && !dialog.archived)
            || options.inactive_days.is_some_and(|days| {
                let cutoff = now - i64::from(days) * 86_400;
                dialog.last_date.is_some_and(|last| last > cutoff)
            });
        if protected {
            kept += 1;
        } else {
            selected.push(dialog.clone());
        }
    }
    (selected, kept, owned)
}

pub async fn plan_cleanup(handle: &AccountHandle, options: &CleanupOptions) -> Result<CleanupPlan> {
    let all = dialogs(handle).await?;
    let (selected, kept, owned) = select_dialogs(&all, options, now_secs());
    let contacts = if options.contacts {
        contacts(handle)
            .await?
            .into_iter()
            .filter(|c| !options.keep_ids.contains(&c.id))
            .filter(|c| {
                !c.username.as_ref().is_some_and(|u| {
                    options
                        .keep_usernames
                        .iter()
                        .any(|k| k.trim_start_matches('@').eq_ignore_ascii_case(u))
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(CleanupPlan {
        account_id: handle.user_id,
        account_name: handle.name.clone(),
        contacts,
        dialogs: selected,
        kept,
        owned_skipped: owned,
    })
}

/// Saves everything a cleanup is about to remove, so contacts can be re-added later.
pub async fn export_plan(plan: &CleanupPlan) -> Result<PathBuf> {
    let dir = PathBuf::from(crate::backup::BACKUP_DIR).join("exports");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!(
        "cleanup-{}-{}.json",
        plan.account_id,
        crate::backup::timestamp()
    ));
    tokio::fs::write(&path, serde_json::to_vec_pretty(plan)?).await?;
    Ok(path)
}

/// Retries a request after FLOOD_WAIT, honoring cancellation while waiting.
async fn with_flood<T, F, Fut>(job: &Job, mut call: F) -> Result<T, InvocationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, InvocationError>>,
{
    let mut attempt = 0;
    loop {
        match call().await {
            Err(InvocationError::Rpc(error))
                if error.name.starts_with("FLOOD") && attempt < FLOOD_RETRIES =>
            {
                attempt += 1;
                let wait = error.value.unwrap_or(5).min(MAX_FLOOD_WAIT_SECS);
                job.log(format!("⏳ Telegram asked to wait {wait} s")).await;
                for _ in 0..wait {
                    if job.is_cancelled() {
                        return Err(InvocationError::Rpc(error));
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            other => return other,
        }
    }
}

async fn pause(job: &Job) {
    // Sleep in small steps so cancellation feels immediate.
    let steps = ACTION_DELAY.as_millis() / 100;
    for _ in 0..steps {
        if job.is_cancelled() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn input_user(contact: &ContactInfo) -> tl::enums::InputUser {
    tl::enums::InputUser::User(tl::types::InputUser {
        user_id: contact.id,
        access_hash: contact.access_hash,
    })
}

async fn delete_history(
    client: &Client,
    peer: PeerRef,
    revoke: bool,
) -> Result<(), InvocationError> {
    // Telegram deletes in chunks and reports a non-zero offset while more remain.
    for _ in 0..50 {
        let result = client
            .invoke(&tl::functions::messages::DeleteHistory {
                just_clear: false,
                revoke,
                peer: peer.into(),
                max_id: 0,
                min_date: None,
                max_date: None,
            })
            .await?;
        let tl::enums::messages::AffectedHistory::History(affected) = result;
        if affected.offset <= 0 {
            break;
        }
    }
    Ok(())
}

pub async fn run_cleanup(
    handle: AccountHandle,
    plan: CleanupPlan,
    options: CleanupOptions,
    job: Job,
) {
    let total = plan.total() as u64;
    job.set_total(total).await;
    match export_plan(&plan).await {
        Ok(path) => {
            job.log(format!("💾 Export saved: {}", path.display()))
                .await
        }
        Err(error) => {
            job.finish(
                JobStatus::Failed,
                format!("could not save the export, nothing was deleted: {error}"),
            )
            .await;
            return;
        }
    }

    let mut ok = 0usize;
    let mut failed = 0usize;

    for chunk in plan.contacts.chunks(50) {
        if job.is_cancelled() {
            break;
        }
        let request = tl::functions::contacts::DeleteContacts {
            id: chunk.iter().map(input_user).collect(),
        };
        let result = with_flood(&job, || handle.client.invoke(&request)).await;
        for _ in chunk {
            job.advance().await;
        }
        match result {
            Ok(_) => {
                ok += chunk.len();
                job.log(format!("👤 Deleted {} contact(s)", chunk.len()))
                    .await;
            }
            Err(error) => {
                failed += chunk.len();
                job.log(format!("⚠️ Contacts: {error}")).await;
            }
        }
        pause(&job).await;
    }

    for dialog in &plan.dialogs {
        if job.is_cancelled() {
            break;
        }
        let Some(peer) = dialog.peer.to_ref() else {
            job.advance().await;
            failed += 1;
            continue;
        };
        let client = &handle.client;
        let result = match dialog.kind {
            DialogKind::User | DialogKind::Bot => {
                let revoke = options.revoke && dialog.kind == DialogKind::User;
                with_flood(&job, || delete_history(client, peer, revoke)).await
            }
            DialogKind::Group | DialogKind::Supergroup | DialogKind::Channel => {
                with_flood(&job, || client.delete_dialog(peer)).await
            }
            DialogKind::Saved => Ok(()),
        };
        job.advance().await;
        let verb = match dialog.kind {
            DialogKind::User | DialogKind::Bot => "🗑 Deleted chat",
            _ => "🚪 Left",
        };
        match result {
            Ok(()) => {
                ok += 1;
                job.log(format!("{verb}: {}", dialog.title)).await;
            }
            Err(error) => {
                failed += 1;
                job.log(format!("⚠️ {}: {error}", dialog.title)).await;
            }
        }
        pause(&job).await;
    }

    let summary = format!("{ok} done, {failed} failed, {} kept", plan.kept);
    let status = if job.is_cancelled() {
        JobStatus::Cancelled
    } else if failed > 0 && ok == 0 {
        JobStatus::Failed
    } else {
        JobStatus::Done
    };
    job.finish(status, summary).await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkAction {
    ReadAll,
    ArchiveInactive,
    MuteAll,
    Export,
}

impl BulkAction {
    pub fn title(self) -> &'static str {
        match self {
            BulkAction::ReadAll => "Read all chats",
            BulkAction::ArchiveInactive => "Archive inactive chats",
            BulkAction::MuteAll => "Mute all chats",
            BulkAction::Export => "Export contacts and chats",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().replace('-', "_").as_str() {
            "read_all" | "readall" | "read" => Some(BulkAction::ReadAll),
            "archive_inactive" | "archive" => Some(BulkAction::ArchiveInactive),
            "mute_all" | "muteall" | "mute" => Some(BulkAction::MuteAll),
            "export" => Some(BulkAction::Export),
            _ => None,
        }
    }
}

pub async fn run_bulk(handle: AccountHandle, action: BulkAction, days: u32, job: Job) {
    let list = match dialogs(&handle).await {
        Ok(list) => list,
        Err(error) => {
            job.finish(JobStatus::Failed, error.to_string()).await;
            return;
        }
    };
    let client = handle.client.clone();
    let mut done = 0usize;
    let mut failed = 0usize;

    match action {
        BulkAction::Export => {
            let plan = CleanupPlan {
                account_id: handle.user_id,
                account_name: handle.name.clone(),
                contacts: contacts(&handle).await.unwrap_or_default(),
                dialogs: list,
                kept: 0,
                owned_skipped: 0,
            };
            match export_plan(&plan).await {
                Ok(path) => {
                    job.log(format!("💾 {}", path.display())).await;
                    job.finish(
                        JobStatus::Done,
                        format!(
                            "{} contacts, {} chats exported to {}",
                            plan.contacts.len(),
                            plan.dialogs.len(),
                            path.display()
                        ),
                    )
                    .await;
                }
                Err(error) => job.finish(JobStatus::Failed, error.to_string()).await,
            }
            return;
        }
        BulkAction::ReadAll => {
            let targets = list
                .iter()
                .filter(|d| d.unread > 0 || d.mentions > 0)
                .collect::<Vec<_>>();
            job.set_total(targets.len() as u64).await;
            for dialog in targets {
                if job.is_cancelled() {
                    break;
                }
                let Some(peer) = dialog.peer.to_ref() else {
                    continue;
                };
                let mut result = with_flood(&job, || client.mark_as_read(peer)).await;
                if result.is_ok() && dialog.mentions > 0 {
                    result = with_flood(&job, || async {
                        client
                            .invoke(&tl::functions::messages::ReadMentions {
                                peer: peer.into(),
                                top_msg_id: None,
                            })
                            .await
                            .map(drop)
                    })
                    .await;
                }
                job.advance().await;
                match result {
                    Ok(()) => done += 1,
                    Err(error) => {
                        failed += 1;
                        job.log(format!("⚠️ {}: {error}", dialog.title)).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(350)).await;
            }
        }
        BulkAction::ArchiveInactive => {
            let cutoff = now_secs() - i64::from(days.max(1)) * 86_400;
            let targets = list
                .iter()
                .filter(|d| {
                    !d.archived
                        && !d.pinned
                        && d.kind != DialogKind::Saved
                        && d.last_date.is_some_and(|last| last < cutoff)
                })
                .collect::<Vec<_>>();
            job.set_total(targets.len() as u64).await;
            for chunk in targets.chunks(50) {
                if job.is_cancelled() {
                    break;
                }
                let folder_peers = chunk
                    .iter()
                    .filter_map(|d| d.peer.to_ref())
                    .map(|peer| {
                        tl::enums::InputFolderPeer::Peer(tl::types::InputFolderPeer {
                            peer: peer.into(),
                            folder_id: ARCHIVE_FOLDER,
                        })
                    })
                    .collect::<Vec<_>>();
                let request = tl::functions::folders::EditPeerFolders { folder_peers };
                let result = with_flood(&job, || client.invoke(&request)).await;
                for dialog in chunk {
                    job.advance().await;
                    if result.is_ok() {
                        job.log(format!("📦 {}", dialog.title)).await;
                    }
                }
                match result {
                    Ok(_) => done += chunk.len(),
                    Err(error) => {
                        failed += chunk.len();
                        job.log(format!("⚠️ {error}")).await;
                    }
                }
                pause(&job).await;
            }
        }
        BulkAction::MuteAll => {
            let targets = list
                .iter()
                .filter(|d| !d.muted && d.kind != DialogKind::Saved)
                .collect::<Vec<_>>();
            job.set_total(targets.len() as u64).await;
            for dialog in targets {
                if job.is_cancelled() {
                    break;
                }
                let Some(peer) = dialog.peer.to_ref() else {
                    continue;
                };
                let request = tl::functions::account::UpdateNotifySettings {
                    peer: tl::enums::InputNotifyPeer::Peer(tl::types::InputNotifyPeer {
                        peer: peer.into(),
                    }),
                    settings: tl::enums::InputPeerNotifySettings::Settings(
                        tl::types::InputPeerNotifySettings {
                            show_previews: None,
                            silent: None,
                            mute_until: Some(i32::MAX),
                            sound: None,
                            stories_muted: None,
                            stories_hide_sender: None,
                            stories_sound: None,
                        },
                    ),
                };
                let result = with_flood(&job, || client.invoke(&request)).await;
                job.advance().await;
                match result {
                    Ok(_) => done += 1,
                    Err(error) => {
                        failed += 1;
                        job.log(format!("⚠️ {}: {error}", dialog.title)).await;
                    }
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
        }
    }

    let status = if job.is_cancelled() {
        JobStatus::Cancelled
    } else {
        JobStatus::Done
    };
    job.finish(status, format!("{done} done, {failed} failed"))
        .await;
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProfileUpdate {
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub about: Option<String>,
    pub username: Option<String>,
}

pub async fn update_profile(handle: &AccountHandle, update: &ProfileUpdate) -> Result<()> {
    if update.first_name.is_some() || update.last_name.is_some() || update.about.is_some() {
        if update
            .first_name
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
        {
            anyhow::bail!("first name cannot be empty");
        }
        handle
            .client
            .invoke(&tl::functions::account::UpdateProfile {
                first_name: update.first_name.clone(),
                last_name: update.last_name.clone(),
                about: update.about.clone(),
            })
            .await?;
    }
    if let Some(username) = &update.username {
        handle
            .client
            .invoke(&tl::functions::account::UpdateUsername {
                username: username.trim().trim_start_matches('@').to_string(),
            })
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(id: i64, kind: DialogKind) -> DialogInfo {
        DialogInfo {
            id,
            peer: StoredPeer {
                id,
                hash: 0,
                is_self: false,
            },
            kind,
            title: format!("chat {id}"),
            username: None,
            last_date: Some(0),
            unread: 0,
            mentions: 0,
            pinned: false,
            archived: false,
            muted: false,
            owner: false,
            admin: false,
        }
    }

    #[test]
    fn selects_only_requested_kinds_and_protects_owned_chats() {
        let mut owned = dialog(3, DialogKind::Channel);
        owned.owner = true;
        let dialogs = vec![
            dialog(1, DialogKind::User),
            dialog(2, DialogKind::Group),
            owned,
            dialog(4, DialogKind::Channel),
            dialog(5, DialogKind::Saved),
            dialog(6, DialogKind::Bot),
        ];
        let options = CleanupOptions {
            channels: true,
            groups: true,
            ..CleanupOptions::default()
        };
        let (selected, kept, owned_count) = select_dialogs(&dialogs, &options, 1_000_000);
        let ids = selected.iter().map(|d| d.id).collect::<Vec<_>>();
        assert_eq!(ids, vec![2, 4]);
        assert_eq!(kept, 0);
        assert_eq!(owned_count, 1);
    }

    #[test]
    fn filters_keep_admin_pinned_whitelist_and_recent() {
        let now = 100 * 86_400;
        let mut admin = dialog(1, DialogKind::Supergroup);
        admin.admin = true;
        let mut pinned = dialog(2, DialogKind::User);
        pinned.pinned = true;
        let mut recent = dialog(3, DialogKind::User);
        recent.last_date = Some(now - 86_400);
        let mut named = dialog(4, DialogKind::User);
        named.username = Some("Friend".into());
        let whitelisted = dialog(5, DialogKind::User);
        let old = dialog(6, DialogKind::User);
        let dialogs = vec![admin, pinned, recent, named, whitelisted, old];
        let options = CleanupOptions {
            groups: true,
            private_chats: true,
            inactive_days: Some(30),
            keep_ids: vec![5],
            keep_usernames: vec!["@friend".into()],
            ..CleanupOptions::default()
        };
        let (selected, kept, _) = select_dialogs(&dialogs, &options, now);
        assert_eq!(selected.iter().map(|d| d.id).collect::<Vec<_>>(), vec![6]);
        assert_eq!(kept, 5);
    }

    #[test]
    fn options_default_to_safe_values() {
        let options: CleanupOptions = serde_json::from_str(r#"{"groups": true}"#).unwrap();
        assert!(options.keep_admin && options.keep_pinned && !options.revoke);
        assert!(!options.is_empty());
        assert!(CleanupOptions::default().is_empty());
    }

    #[tokio::test]
    async fn plans_require_matching_code() {
        let registry = AccountRegistry::new();
        let code = registry
            .store_plan(7, CleanupOptions::default(), CleanupPlan::default())
            .await;
        assert_eq!(code.len(), 6);
        assert!(registry.take_plan("NOPE00").await.is_none());
        let (user, _, _) = registry.take_plan(&code.to_lowercase()).await.unwrap();
        assert_eq!(user, 7);
        assert!(
            registry.take_plan(&code).await.is_none(),
            "codes are single-use"
        );
    }

    #[test]
    fn bulk_action_names() {
        assert_eq!(BulkAction::parse("read-all"), Some(BulkAction::ReadAll));
        assert_eq!(
            BulkAction::parse("archive"),
            Some(BulkAction::ArchiveInactive)
        );
        assert_eq!(BulkAction::parse("nope"), None);
    }
}
