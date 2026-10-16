//! Chat and account export: I offer Telegram's official takeout method and an extended format
//! of my own (rich HTML, JSON, single-chat PDF, deleted-message marks).

pub mod fetch;
pub mod html;
pub mod json;
pub mod model;
pub mod pdf;
pub mod rules;
pub mod text;
pub mod watch;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use grammers_client::{tl, InvocationError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::account_tools::{now_secs, AccountHandle};
use crate::app::Services;
use crate::jobs::{Job, JobStatus};
use fetch::{DialogEntry, Fetcher, PeerIndex};
use model::*;
use rules::ExportRules;

pub const EXPORTS_DIR: &str = "exports";
const TAKEOUT_KEY_PREFIX: &str = "export.takeout.pending.";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExportRequest {
    pub method: ExportMethod,
    /// With none I export every connected account.
    #[serde(default)]
    pub accounts: Vec<i64>,
    /// With none I export every chat the rules allow, plus the account data.
    #[serde(default)]
    pub chats: Vec<i64>,
    /// One-off changes I apply on top of the default and per-account rules.
    #[serde(default)]
    pub rules: Option<Value>,
}

/// I start one background job per account and return the job ids.
pub async fn start(services: &Arc<Services>, request: ExportRequest) -> Result<Vec<String>> {
    let handles = services.accounts.select(&request.accounts).await?;
    let mut prepared = Vec::new();
    for handle in handles {
        let rules = rules::effective_rules(&services.db, handle.user_id, request.rules.as_ref())
            .await
            .with_context(|| format!("export rules for {}", handle.name))?;
        if request.method == ExportMethod::Official {
            if let Some(pending) = takeout_pending(services, handle.user_id).await {
                bail!("{}: {}", handle.name, pending);
            }
        }
        prepared.push((handle, rules));
    }
    let mut jobs = Vec::new();
    for (handle, rules) in prepared {
        let title = match request.chats.len() {
            0 => format!("{}: whole account", request.method.title()),
            1 => format!("{}: one chat", request.method.title()),
            n => format!("{}: {n} chats", request.method.title()),
        };
        let job = services.jobs.start("export", &title, &handle.name).await;
        jobs.push(job.id().to_string());
        let services_task = Arc::clone(services);
        let request = request.clone();
        let job_task = job.clone();
        services.spawn_job(job, async move {
            run(services_task, handle, request, rules, job_task).await;
        });
    }
    Ok(jobs)
}

async fn run(
    services: Arc<Services>,
    handle: AccountHandle,
    request: ExportRequest,
    rules: ExportRules,
    job: Job,
) {
    match export(&services, &handle, &request, rules, &job).await {
        Ok(summary) if job.is_cancelled() => {
            job.finish(JobStatus::Cancelled, format!("Stopped. {summary}"))
                .await
        }
        Ok(summary) => job.finish(JobStatus::Done, summary).await,
        Err(error) => {
            tracing::warn!("export for {} failed: {error:#}", handle.name);
            job.log(format!("Error: {error:#}")).await;
            job.finish(JobStatus::Failed, format!("{error:#}")).await
        }
    }
}

struct Counters {
    chats: usize,
    messages: usize,
    deleted: usize,
    saved: usize,
    skipped: usize,
}

async fn export(
    services: &Arc<Services>,
    handle: &AccountHandle,
    request: &ExportRequest,
    mut rules: ExportRules,
    job: &Job,
) -> Result<String> {
    let method = request.method;
    let single_chat = request.chats.len() == 1;
    if !request.chats.is_empty() {
        rules.chats.only = request.chats.clone();
        rules.chats.exclude.clear();
    }
    if method == ExportMethod::Official {
        rules.include_deleted = false;
        rules.formats.pdf = false;
    } else if rules.formats.pdf && !single_chat {
        job.log("PDF is made for single-chat exports, so I skip it here")
            .await;
        rules.formats.pdf = false;
    }

    let requested_at = now_secs();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let id = format!(
        "{stamp}-{}{}",
        match method {
            ExportMethod::Official => "official",
            ExportMethod::Custom => "extended",
        },
        if single_chat { "-chat" } else { "" }
    );
    let root = PathBuf::from(EXPORTS_DIR)
        .join(handle.user_id.to_string())
        .join(&id);
    tokio::fs::create_dir_all(&root)
        .await
        .with_context(|| format!("creating {}", root.display()))?;

    let mut manifest = ExportManifest {
        id: id.clone(),
        method,
        account_id: handle.user_id,
        account_name: handle.name.clone(),
        chat_id: single_chat.then(|| request.chats[0]),
        title: format!("{} · {}", handle.name, method.title()),
        requested_at,
        finished_at: None,
        status: "running".into(),
        files: Vec::new(),
        chats: 0,
        messages: 0,
        deleted_marked: 0,
        media_saved: 0,
        media_skipped: 0,
        bytes: 0,
        error: None,
        rules: rules.clone(),
    };
    write_manifest(&root, &manifest).await?;

    let takeout = match method {
        ExportMethod::Official => Some(init_takeout(services, handle, &rules, job).await?),
        ExportMethod::Custom => None,
    };
    let fetcher = Fetcher::new(
        handle.client.clone(),
        takeout,
        job.clone(),
        root.clone(),
        handle.user_id,
    );
    let result = collect(services, &fetcher, &rules, &mut manifest, single_chat, job).await;
    if let Some(takeout_id) = takeout {
        let finished = fetcher
            .call(tl::functions::account::FinishTakeoutSession {
                success: result.is_ok() && !job.is_cancelled(),
            })
            .await;
        if let Err(error) = finished {
            tracing::debug!("finishing takeout {takeout_id}: {error}");
        }
    }
    manifest.finished_at = Some(now_secs());
    manifest.bytes = dir_size(&root).await;
    match &result {
        Ok(counters) => {
            manifest.status = if job.is_cancelled() {
                "cancelled"
            } else {
                "done"
            }
            .into();
            manifest.chats = counters.chats;
            manifest.messages = counters.messages;
            manifest.deleted_marked = counters.deleted;
            manifest.media_saved = counters.saved;
            manifest.media_skipped = counters.skipped;
        }
        Err(error) => {
            manifest.status = "failed".into();
            manifest.error = Some(format!("{error:#}"));
        }
    }
    write_manifest(&root, &manifest).await?;
    let counters = result?;
    Ok(format!(
        "{} chats, {} messages, {} files saved, {} left out{}, {}",
        counters.chats,
        counters.messages,
        counters.saved,
        counters.skipped,
        if counters.deleted > 0 {
            format!(", {} deleted messages marked", counters.deleted)
        } else {
            String::new()
        },
        fetch::human_bytes(manifest.bytes)
    ))
}

async fn collect(
    services: &Arc<Services>,
    fetcher: &Fetcher,
    rules: &ExportRules,
    manifest: &mut ExportManifest,
    single_chat: bool,
    job: &Job,
) -> Result<Counters> {
    let method = manifest.method;
    let root = fetcher.root.clone();
    let mut index = PeerIndex::default();
    let mut counters = Counters {
        chats: 0,
        messages: 0,
        deleted: 0,
        saved: 0,
        skipped: 0,
    };

    let account = if single_chat {
        None
    } else {
        job.log("Reading account information").await;
        match fetcher.account_info(rules, &mut index).await {
            Ok(info) => Some(info),
            Err(error) => {
                job.log(format!("Account information is incomplete: {error:#}"))
                    .await;
                None
            }
        }
    };

    fetcher.absorb_self(&mut index).await;
    job.log("Listing chats").await;
    let dialogs = fetcher
        .dialogs(&mut index)
        .await
        .map_err(|error| describe_rpc(error, "listing chats"))?;
    let selected = dialogs
        .into_iter()
        .filter(|dialog| fetch::chat_allowed(dialog, rules))
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("no chats match the export rules");
    }
    job.set_total(selected.len() as u64).await;
    job.log(format!("{} chats to export", selected.len())).await;

    let formats = &rules.formats;
    if method == ExportMethod::Custom && formats.html {
        html::write_assets(&root).await?;
    }
    let mut desktop = if method == ExportMethod::Official && formats.json {
        Some(json::DesktopWriter::create(&root, account.as_ref()).await?)
    } else {
        None
    };
    let skip_words = rules.skip_words_lower();
    let mut cards = Vec::new();
    let mut summaries = Vec::new();
    let mut pdf_file = None;

    for (position, dialog) in selected.iter().enumerate() {
        if job.is_cancelled() {
            break;
        }
        job.log(format!("Exporting {}", dialog.peer.title)).await;
        let chat_dir = match method {
            ExportMethod::Custom => {
                format!("chats/{}", fetch::safe_name(&dialog.peer.id.to_string()))
            }
            ExportMethod::Official => format!("chats/chat_{:03}", position + 1),
        };
        let mut chat = match export_chat(
            services,
            fetcher,
            dialog,
            rules,
            &skip_words,
            &mut index,
            &chat_dir,
            method,
        )
        .await
        {
            Ok(chat) => chat,
            Err(error) => {
                job.log(format!("{} was skipped: {error:#}", dialog.peer.title))
                    .await;
                job.advance().await;
                continue;
            }
        };
        for message in &chat.messages {
            if let Some(media) = &message.media {
                if media.file.is_some() {
                    counters.saved += 1;
                } else if media.kind.is_some() {
                    counters.skipped += 1;
                }
            }
        }
        counters.chats += 1;
        counters.messages += chat.messages.len();
        counters.deleted += chat.stats.deleted;

        let mut json_file = None;
        let mut page = None;
        match method {
            ExportMethod::Custom => {
                if formats.json {
                    json_file = Some(json::write_chat(&root, &chat_dir, &chat).await?);
                }
                if formats.html {
                    let (relative, base, back) = if single_chat {
                        ("index.html".to_string(), "", None)
                    } else {
                        (
                            format!("{chat_dir}/index.html"),
                            "../../",
                            Some("../../index.html"),
                        )
                    };
                    let body = html::chat_page(
                        &chat,
                        &html::PageMeta {
                            base,
                            back,
                            account_name: &manifest.account_name,
                            method,
                            requested_at: manifest.requested_at,
                        },
                    );
                    write_file(&root, &relative, body.as_bytes()).await?;
                    page = Some(relative);
                }
                if formats.pdf && single_chat {
                    job.log("Rendering the PDF").await;
                    let name = format!("{}.pdf", fetch::safe_name(&chat.title));
                    let stamp = pdf::PdfStamp {
                        account_name: manifest.account_name.clone(),
                        self_id: fetcher.self_id,
                        requested_at: manifest.requested_at,
                        completed_at: now_secs(),
                    };
                    let pdf_root = root.clone();
                    let chat_copy = chat.clone();
                    let bytes = tokio::task::spawn_blocking(move || {
                        pdf::render_chat(&chat_copy, &pdf_root, &stamp)
                    })
                    .await
                    .map_err(|error| anyhow!("PDF renderer stopped: {error}"))??;
                    write_file(&root, &name, &bytes).await?;
                    pdf_file = Some(name);
                }
            }
            ExportMethod::Official => {
                if let Some(writer) = desktop.as_mut() {
                    writer.add_chat(&chat).await?;
                }
                if formats.html {
                    let relative = format!("{chat_dir}/messages.html");
                    let body = html::desktop_chat_page(&chat, "../../export_results.html");
                    write_file(&root, &relative, body.as_bytes()).await?;
                    page = Some(relative);
                }
            }
        }
        summaries.push(json::chat_summary(
            &chat,
            json_file.as_deref(),
            page.as_deref(),
        ));
        cards.push(html::ChatCard {
            id: chat.id,
            title: chat.title.clone(),
            kind: chat.kind.clone(),
            username: chat.username.clone(),
            avatar: chat.avatar.clone(),
            page: page.unwrap_or_default(),
            messages: chat.messages.len(),
            deleted: chat.stats.deleted,
            media: chat
                .messages
                .iter()
                .filter(|m| m.media.as_ref().is_some_and(|x| x.file.is_some()))
                .count(),
            last_date: chat.stats.last_date,
        });
        chat.messages.clear();
        job.advance().await;
    }

    manifest.chats = counters.chats;
    manifest.messages = counters.messages;
    manifest.deleted_marked = counters.deleted;
    manifest.media_saved = counters.saved;
    manifest.media_skipped = counters.skipped;
    manifest.finished_at = Some(now_secs());
    manifest.bytes = fetcher.downloaded_bytes();

    let mut files = Vec::new();
    match method {
        ExportMethod::Custom => {
            if formats.html {
                if single_chat {
                    files.push("index.html".to_string());
                } else {
                    let body = html::index_page(manifest, account.as_ref(), &cards);
                    write_file(&root, "index.html", body.as_bytes()).await?;
                    files.push("index.html".to_string());
                }
            }
            if let Some(pdf) = pdf_file {
                files.push(pdf);
            }
            if formats.json {
                files.push(
                    json::write_summary(&root, manifest, account.as_ref(), &summaries).await?,
                );
            }
        }
        ExportMethod::Official => {
            if formats.html {
                let body = html::desktop_index(manifest, &cards);
                write_file(&root, "export_results.html", body.as_bytes()).await?;
                files.push("export_results.html".to_string());
            }
            if let Some(writer) = desktop {
                files.push(writer.finish().await?);
            }
        }
    }
    manifest.files = files;
    Ok(counters)
}

#[allow(clippy::too_many_arguments)]
async fn export_chat(
    services: &Arc<Services>,
    fetcher: &Fetcher,
    dialog: &DialogEntry,
    rules: &ExportRules,
    skip_words: &[String],
    index: &mut PeerIndex,
    chat_dir: &str,
    method: ExportMethod,
) -> Result<ChatExport> {
    let raw = fetcher
        .history(dialog, rules, index)
        .await
        .map_err(|error| describe_rpc(error, "reading the history"))?;
    let mut chat = fetch::chat_shell(dialog);
    let mut raw_media = HashMap::new();
    for message in raw.iter().rev() {
        let Some(converted) = fetch::convert_message(message, dialog, index, fetcher.self_id)
        else {
            continue;
        };
        if !fetch::keep_message(&converted, rules, skip_words) {
            continue;
        }
        if let tl::enums::Message::Message(m) = message {
            if let Some(media) = &m.media {
                raw_media.insert(m.id, media.clone());
            }
        }
        chat.messages.push(converted);
    }
    drop(raw);
    fetch::register_participants(&mut chat, index);

    if method == ExportMethod::Custom {
        let (about, members) = fetcher.chat_details(&dialog.peer).await;
        chat.about = about;
        chat.members = members;
        if rules.account.avatars || rules.formats.pdf {
            chat.avatar = fetcher.avatar(&dialog.peer, "avatars").await;
        }
        if rules.include_deleted {
            match crate::anti_delete::deleted_for_chat(
                &services.db,
                fetcher.self_id,
                dialog.peer.id,
            )
            .await
            {
                Ok(archived) if !archived.is_empty() => {
                    let added = fetch::merge_deleted(
                        &mut chat,
                        archived,
                        rules,
                        skip_words,
                        &fetcher.root,
                        chat_dir,
                    )
                    .await;
                    if added > 0 {
                        fetcher
                            .job
                            .log(format!(
                                "{}: {added} deleted messages marked",
                                dialog.peer.title
                            ))
                            .await;
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    fetcher
                        .job
                        .log(format!("Anti-delete archive unavailable: {error:#}"))
                        .await
                }
            }
        }
    }

    let total_media = raw_media.len();
    let mut done_media = 0;
    for message in chat.messages.iter_mut() {
        if fetcher.job.is_cancelled() {
            break;
        }
        let (Some(media), Some(raw)) = (message.media.as_mut(), raw_media.get(&message.id)) else {
            continue;
        };
        if message
            .deleted
            .as_ref()
            .is_some_and(|d| d.restored_from_archive)
        {
            continue;
        }
        fetcher
            .save_media(raw, media, rules, chat_dir, message.id)
            .await;
        done_media += 1;
        if done_media % 25 == 0 {
            fetcher
                .job
                .log(format!(
                    "{}: {done_media}/{total_media} attachments",
                    dialog.peer.title
                ))
                .await;
        }
    }
    chat.stats = ChatStats::build(&chat);
    // The PDF shows every sender with a photo, so I also fetch them for private chats.
    if method == ExportMethod::Custom && (rules.account.avatars || rules.formats.pdf) {
        fetcher.participant_avatars(&mut chat, index).await;
    }
    Ok(chat)
}

async fn init_takeout(
    services: &Arc<Services>,
    handle: &AccountHandle,
    rules: &ExportRules,
    job: &Job,
) -> Result<i64> {
    job.log("Asking Telegram to open a takeout session").await;
    let chats = &rules.chats;
    let request = tl::functions::account::InitTakeoutSession {
        contacts: rules.account.contacts,
        message_users: chats.private || chats.bots || chats.saved || !chats.only.is_empty(),
        message_chats: chats.groups || !chats.only.is_empty(),
        message_megagroups: chats.groups || !chats.only.is_empty(),
        message_channels: chats.channels || !chats.only.is_empty(),
        files: rules.media.any(),
        file_max_size: rules.media.any().then(|| {
            rules
                .media
                .max_file_bytes()
                .map(|b| b as i64)
                .unwrap_or(4 * 1024 * 1024 * 1024)
        }),
    };
    match handle.client.invoke(&request).await {
        Ok(tl::enums::account::Takeout::Takeout(takeout)) => {
            let _ = services.db.remove(&takeout_key(handle.user_id)).await;
            Ok(takeout.id)
        }
        Err(InvocationError::Rpc(error)) if error.name == "TAKEOUT_INIT_DELAY" => {
            let wait = i64::from(error.value.unwrap_or(86_400));
            let until = now_secs() + wait;
            services
                .db
                .set(
                    takeout_key(handle.user_id),
                    json!({ "until": until, "asked_at": now_secs() }),
                )
                .await?;
            bail!(
                "Telegram wants to confirm this export first. Open Telegram on a device where you are signed in and allow the data export request from the service notification, then start the export again. Without confirmation it unlocks at {}. The extended export works right away.",
                html::local_time(until, "%d.%m.%Y %H:%M")
            )
        }
        Err(error) => Err(describe_rpc(error, "opening the takeout session")),
    }
}

fn takeout_key(account_id: i64) -> String {
    format!("{TAKEOUT_KEY_PREFIX}{account_id}")
}

/// I explain why the official export is still locked for this account, if it is.
pub async fn takeout_pending(services: &Services, account_id: i64) -> Option<String> {
    let value = services.db.get(&takeout_key(account_id)).await;
    let until = value["until"].as_i64()?;
    if until <= now_secs() {
        return None;
    }
    Some(format!(
        "the official export unlocks at {} unless you confirm it in Telegram sooner; after confirming, clear the wait in the panel",
        html::local_time(until, "%d.%m.%Y %H:%M")
    ))
}

pub async fn clear_takeout_wait(services: &Services, account_id: i64) -> Result<()> {
    services.db.remove(&takeout_key(account_id)).await
}

pub async fn takeout_waits(services: &Services) -> Value {
    let mut out = serde_json::Map::new();
    for key in services.db.keys_with_prefix(TAKEOUT_KEY_PREFIX).await {
        let value = services.db.get(&key).await;
        if value["until"]
            .as_i64()
            .is_some_and(|until| until > now_secs())
        {
            out.insert(
                key.trim_start_matches(TAKEOUT_KEY_PREFIX).to_string(),
                value,
            );
        }
    }
    Value::Object(out)
}

fn describe_rpc(error: impl Into<anyhow::Error>, what: &str) -> anyhow::Error {
    let error = error.into();
    let Some(InvocationError::Rpc(rpc)) = error.downcast_ref::<InvocationError>() else {
        return anyhow!("{what}: {error:#}");
    };
    let hint = match rpc.name.as_str() {
        "TAKEOUT_INVALID" | "TAKEOUT_REQUIRED" => {
            "the takeout session expired; start the export again"
        }
        "CHANNEL_PRIVATE" | "CHANNEL_INVALID" => "this chat is no longer available to the account",
        "CHAT_ADMIN_REQUIRED" => "Telegram only shows this history to admins",
        "AUTH_KEY_UNREGISTERED" | "SESSION_REVOKED" | "USER_DEACTIVATED" => {
            "the session was signed out"
        }
        name if name.starts_with("FLOOD") => "Telegram limited the request rate; try again later",
        _ => "",
    };
    if hint.is_empty() {
        anyhow!("{what}: {}", rpc.name)
    } else {
        anyhow!("{what}: {hint} ({})", rpc.name)
    }
}

async fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("writing {}", path.display()))
}

async fn write_manifest(root: &Path, manifest: &ExportManifest) -> Result<()> {
    write_file(root, "manifest.json", &serde_json::to_vec_pretty(manifest)?).await
}

async fn dir_size(root: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            match entry.metadata().await {
                Ok(meta) if meta.is_dir() => stack.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// I list every export on disk, newest first.
pub async fn list() -> Vec<ExportManifest> {
    let mut out = Vec::new();
    let Ok(mut accounts) = tokio::fs::read_dir(EXPORTS_DIR).await else {
        return out;
    };
    while let Ok(Some(account)) = accounts.next_entry().await {
        let Ok(mut exports) = tokio::fs::read_dir(account.path()).await else {
            continue;
        };
        while let Ok(Some(entry)) = exports.next_entry().await {
            let path = entry.path().join("manifest.json");
            let Ok(raw) = tokio::fs::read(&path).await else {
                continue;
            };
            if let Ok(manifest) = serde_json::from_slice::<ExportManifest>(&raw) {
                out.push(manifest);
            }
        }
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.requested_at));
    out
}

/// I return the folder of one export and reject ids that could step outside the exports directory.
pub fn export_dir(account_id: i64, id: &str) -> Result<PathBuf> {
    let valid = !id.is_empty()
        && Path::new(id)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
    if !valid {
        bail!("invalid export id");
    }
    let dir = PathBuf::from(EXPORTS_DIR)
        .join(account_id.to_string())
        .join(id);
    if !dir.join("manifest.json").exists() {
        bail!("export not found");
    }
    Ok(dir)
}

pub async fn delete(account_id: i64, id: &str) -> Result<()> {
    let dir = export_dir(account_id, id)?;
    tokio::fs::remove_dir_all(&dir).await?;
    let archive = dir.with_extension("tar.gz");
    let _ = tokio::fs::remove_file(archive).await;
    Ok(())
}

/// I pack an export into `<id>.tar.gz` next to its folder and return the archive path.
pub async fn archive(account_id: i64, id: &str) -> Result<PathBuf> {
    let dir = export_dir(account_id, id)?;
    let target = dir.with_extension("tar.gz");
    let name = id.to_string();
    let source = dir.clone();
    let output = target.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::create(&output)?;
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder.append_dir_all(&name, &source)?;
        builder.into_inner()?.finish()?;
        Ok(())
    })
    .await
    .map_err(|error| anyhow!("archiving stopped: {error}"))??;
    Ok(target)
}

/// I remove every export of an account when the account is deleted for good.
pub async fn remove_account(account_id: i64) -> Result<()> {
    let dir = PathBuf::from(EXPORTS_DIR).join(account_id.to_string());
    if dir.exists() {
        tokio::fs::remove_dir_all(dir).await?;
    }
    Ok(())
}

/// I write a sample extended export into `root` for the manual panel preview.
#[cfg(test)]
pub async fn write_preview_sample(root: &Path, account_id: i64) -> Result<()> {
    use rules::MediaKind;
    let id = "20261015-120000-extended";
    let dir = root.join(EXPORTS_DIR).join(account_id.to_string()).join(id);
    tokio::fs::create_dir_all(&dir).await?;
    html::write_assets(&dir).await?;
    let names = ["Alex", "Maria", "Ivan"];
    let mut chats = Vec::new();
    for (index, title) in ["Maria", "Weekend trip", "Work"].into_iter().enumerate() {
        let kind = if index == 0 { "private" } else { "group" };
        let mut chat = ChatExport {
            id: 1000 + index as i64,
            title: title.into(),
            kind: kind.into(),
            username: (index == 0).then(|| "maria".to_string()),
            about: Some("Sample chat for the preview".into()),
            members: (index > 0).then_some(12),
            read_inbox_max_id: 40,
            read_outbox_max_id: 35,
            ..ChatExport::default()
        };
        for (n, name) in names.iter().enumerate() {
            chat.participants.insert(
                n as i64 + 1,
                Participant {
                    name: name.to_string(),
                    ..Participant::default()
                },
            );
        }
        chat.messages = (1..=48)
            .map(|i: i32| {
                let out = i % 3 == 0;
                MessageExport {
                    id: i,
                    date: 1_760_000_000 + i64::from(i) * 2_700,
                    out,
                    from_id: Some(if out { 1 } else { 2 + i64::from(i % 2) }),
                    from_name: Some(
                        if out {
                            "Alex"
                        } else {
                            names[1 + (i % 2) as usize]
                        }
                        .into(),
                    ),
                    text: match i % 7 {
                        0 => "Look at this **plan**, I think it works".into(),
                        3 => "https://example.com/route".into(),
                        _ => format!("Message {i}: shall we meet at the station at nine?"),
                    },
                    entities: if i % 7 == 3 {
                        vec![TextEntity {
                            kind: "url".into(),
                            offset: 0,
                            length: 25,
                            url: None,
                            user_id: None,
                            language: None,
                            document_id: None,
                        }]
                    } else {
                        Vec::new()
                    },
                    reply_to: (i % 5 == 0).then_some(i - 2),
                    edited: (i % 9 == 0).then_some(1_760_000_000 + i64::from(i) * 2_700 + 60),
                    reactions: if i % 4 == 0 {
                        vec![ReactionExport {
                            emoji: "👍".into(),
                            count: 2,
                            chosen: true,
                        }]
                    } else {
                        Vec::new()
                    },
                    read: out.then_some(i <= 35),
                    deleted: (i % 11 == 0).then_some(DeletedMark {
                        deleted_at: 1_760_200_000,
                        restored_from_archive: true,
                    }),
                    media: match i % 8 {
                        2 => Some(MediaExport {
                            type_name: "video".into(),
                            kind: Some(MediaKind::Video),
                            size: Some(84_000_000),
                            duration: Some(95.0),
                            skipped: Some("Larger than the 50.0 MB limit".into()),
                            ..MediaExport::default()
                        }),
                        6 => Some(MediaExport {
                            type_name: "sticker".into(),
                            kind: Some(MediaKind::Sticker),
                            emoji: Some("😄".into()),
                            skipped: Some("Sticker excluded by export rules".into()),
                            ..MediaExport::default()
                        }),
                        7 => Some(MediaExport {
                            type_name: "location".into(),
                            details: Some("55.75580, 37.61730".into()),
                            ..MediaExport::default()
                        }),
                        _ => None,
                    },
                    service: (i == 1 && index > 0)
                        .then(|| "Created the group «Weekend trip»".to_string()),
                    ..MessageExport::default()
                }
            })
            .collect();
        chat.stats = ChatStats::build(&chat);
        let relative = format!("chats/{}/index.html", chat.id);
        let body = html::chat_page(
            &chat,
            &html::PageMeta {
                base: "../../",
                back: Some("../../index.html"),
                account_name: "Alex",
                method: ExportMethod::Custom,
                requested_at: 1_760_300_000,
            },
        );
        write_file(&dir, &relative, body.as_bytes()).await?;
        chats.push(html::ChatCard {
            id: chat.id,
            title: chat.title.clone(),
            kind: chat.kind.clone(),
            username: chat.username.clone(),
            avatar: None,
            page: relative,
            messages: chat.messages.len(),
            deleted: chat.stats.deleted,
            media: 0,
            last_date: chat.stats.last_date,
        });
    }
    let manifest = ExportManifest {
        id: id.into(),
        method: ExportMethod::Custom,
        account_id,
        account_name: "Alex".into(),
        chat_id: None,
        title: "Alex · Extended export".into(),
        requested_at: 1_760_300_000,
        finished_at: Some(1_760_300_420),
        status: "done".into(),
        files: vec!["index.html".into()],
        chats: 3,
        messages: 144,
        deleted_marked: 12,
        media_saved: 0,
        media_skipped: 18,
        bytes: 1_234_567,
        error: None,
        rules: ExportRules::default(),
    };
    let account = AccountInfo {
        id: account_id,
        first_name: "Alex".into(),
        username: Some("alex".into()),
        phone: Some("+10000000000".into()),
        about: Some("Sample profile".into()),
        premium: true,
        sessions: vec![SessionInfo {
            current: true,
            official_app: false,
            app: "fly-telegram 0.2.2".into(),
            device: "Desktop".into(),
            platform: "Windows".into(),
            system: "11".into(),
            ip: "127.0.0.1".into(),
            location: "Local".into(),
            created: 1_760_000_000,
            active: 1_760_300_000,
        }],
        contacts: vec![ContactEntry {
            id: 2,
            name: "Maria".into(),
            username: Some("maria".into()),
            phone: None,
            mutual: true,
        }],
        privacy: vec![PrivacyEntry {
            key: "Last seen and online".into(),
            rules: vec!["My contacts".into()],
        }],
        ..AccountInfo::default()
    };
    write_file(
        &dir,
        "index.html",
        html::index_page(&manifest, Some(&account), &chats).as_bytes(),
    )
    .await?;
    write_manifest(&dir, &manifest).await?;
    let failed = ExportManifest {
        id: "20261014-090000-official".into(),
        method: ExportMethod::Official,
        status: "failed".into(),
        files: Vec::new(),
        error: Some("Telegram wants to confirm this export first. Open Telegram on a device where you are signed in and allow the data export request.".into()),
        requested_at: 1_760_200_000,
        finished_at: Some(1_760_200_010),
        ..manifest
    };
    write_manifest(
        &root
            .join(EXPORTS_DIR)
            .join(account_id.to_string())
            .join(&failed.id),
        &failed,
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_ids_cannot_escape() {
        assert!(export_dir(1, "../x").is_err());
        assert!(export_dir(1, "a/b").is_err());
        assert!(export_dir(1, "").is_err());
        assert!(
            export_dir(1, "20261015-120000-extended").is_err(),
            "missing on disk"
        );
    }
}
