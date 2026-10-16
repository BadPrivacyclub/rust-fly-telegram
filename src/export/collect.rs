use super::model::*;
use crate::account_tools::{self, AccountHandle};
use crate::app::Services;
use crate::jobs::Job;
use anyhow::{Context, Result};
use grammers_client::{tl, Client};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

async fn invoke<R>(
    client: &Client,
    takeout: Option<i64>,
    request: R,
    job: &Job,
) -> Result<R::Return>
where
    R: tl::RemoteCall + Clone + Send + Sync,
    R::Return: Send,
{
    for attempt in 0..=3 {
        anyhow::ensure!(!job.is_cancelled(), "cancelled");
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            if let Some(takeout_id) = takeout {
                client
                    .invoke(&tl::functions::InvokeWithTakeout {
                        takeout_id,
                        query: request.clone(),
                    })
                    .await
            } else {
                client.invoke(&request).await
            }
        })
        .await
        .context("Telegram request timed out")?;
        match result {
            Ok(value) => {
                tokio::time::sleep(Duration::from_millis(250)).await;
                return Ok(value);
            }
            Err(error) => {
                if let grammers_mtsender::InvocationError::Rpc(rpc) = &error {
                    if rpc.name == "FLOOD_WAIT" && attempt < 3 {
                        let wait = rpc.value.unwrap_or(1);
                        anyhow::ensure!(
                            wait <= 900,
                            "Telegram asks to wait {wait} seconds; retry the export later"
                        );
                        job.log(format!("Telegram rate limit: retry in {wait}s"))
                            .await;
                        for _ in 0..wait {
                            anyhow::ensure!(!job.is_cancelled(), "cancelled");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                        continue;
                    }
                }
                return Err(error.into());
            }
        }
    }
    anyhow::bail!("Telegram retry limit reached")
}

pub async fn collect(
    services: &Services,
    handle: &AccountHandle,
    options: Options,
    job: &Job,
    _root: &Path,
    id: &str,
) -> Result<Archive> {
    let requested_at = account_tools::now_secs();
    let takeout = if options.method == Method::Official {
        let result = handle
            .client
            .invoke(&tl::functions::account::InitTakeoutSession {
                contacts: options.contacts,
                message_users: true,
                message_chats: true,
                message_megagroups: true,
                message_channels: true,
                files: options.max_file_bytes > 0,
                file_max_size: (options.max_file_bytes > 0)
                    .then_some(options.max_file_bytes as i64),
            })
            .await;
        match result {
            Ok(tl::enums::account::Takeout::Takeout(value)) => Some(value.id),
            Err(error) => {
                if let grammers_mtsender::InvocationError::Rpc(rpc) = &error {
                    if rpc.name == "TAKEOUT_INIT_DELAY" {
                        let seconds = rpc.value.unwrap_or(86400);
                        anyhow::bail!("Telegram Takeout requires a delay of {seconds} seconds. Retry after Unix timestamp {}. Approve the export in Telegram if requested, or use Custom export.", requested_at + i64::from(seconds));
                    }
                }
                return Err(error.into());
            }
        }
    } else {
        None
    };
    let result = collect_inner(services, handle, options, job, id, requested_at, takeout).await;
    if let Some(takeout_id) = takeout {
        let finish = handle
            .client
            .invoke(&tl::functions::InvokeWithTakeout {
                takeout_id,
                query: tl::functions::account::FinishTakeoutSession {
                    success: result.is_ok(),
                },
            })
            .await;
        if let Err(error) = finish {
            job.log(format!("Could not finish Takeout session: {error}"))
                .await;
        }
    }
    result
}

async fn collect_inner(
    services: &Services,
    handle: &AccountHandle,
    options: Options,
    job: &Job,
    id: &str,
    requested_at: i64,
    takeout: Option<i64>,
) -> Result<Archive> {
    let dialogs = account_tools::dialogs(handle).await?;
    let account_key = handle.client.get_me().await?.id().to_string();
    let archived_chats = if options.method == Method::Custom && options.include_deleted {
        crate::anti_delete::export_catalog(&services.db, &account_key).await?
    } else {
        Vec::new()
    };
    let mut selected: Vec<_> = dialogs
        .into_iter()
        .filter(|d| {
            (options.chats.is_empty() || options.chats.contains(&d.id))
                && !options.excluded_chats.contains(&d.id)
        })
        .collect();
    selected.sort_by_key(|d| d.id);
    if !options.chats.is_empty() {
        for id in &options.chats {
            anyhow::ensure!(
                options.excluded_chats.contains(id)
                    || selected.iter().any(|d| d.id == *id)
                    || archived_chats.iter().any(|c| c.id == id.to_string()),
                "chat {id} is not available to this account"
            );
        }
    }
    job.set_total(selected.len() as u64).await;
    let mut archive = Archive {
        schema_version: 1,
        id: id.into(),
        account_id: handle.user_id,
        account_name: handle.name.clone(),
        requested_at,
        finished_at: 0,
        options: options.clone(),
        account: Value::Null,
        chats: vec![],
        warnings: vec![],
        bytes: 0,
    };
    if options.profile {
        let user = handle.client.get_me().await?;
        archive.account = json!({"id": handle.user_id, "first_name": user.first_name(), "last_name": user.last_name(), "username": user.username(), "phone": user.phone()});
        let full = invoke(
            &handle.client,
            takeout,
            tl::functions::users::GetFullUser {
                id: tl::enums::InputUser::UserSelf,
            },
            job,
        )
        .await?;
        archive.account["about"] = json!(full.full_user().about());
    } else {
        archive.account = json!({});
    }
    if options.contacts {
        archive.account["contacts"] = serde_json::to_value(account_tools::contacts(handle).await?)?;
    }
    if options.sessions {
        let tl::enums::account::Authorizations::Authorizations(value) = invoke(
            &handle.client,
            takeout,
            tl::functions::account::GetAuthorizations {},
            job,
        )
        .await?;
        archive.account["sessions"] = json!(value.authorizations.into_iter().map(|a| {
            let tl::enums::Authorization::Authorization(a) = a;
            json!({"current": a.current, "device_model": a.device_model, "platform": a.platform, "system_version": a.system_version,
                "app_name": a.app_name, "app_version": a.app_version, "date_created": a.date_created,
                "date_active": a.date_active, "ip": a.ip, "country": a.country, "region": a.region})
        }).collect::<Vec<_>>());
    }
    if options.settings {
        archive.account["settings"] = json!({"prefixes": crate::core_settings::prefixes(&services.db).await,
            "disabled_modules": crate::core_settings::disabled_modules(&services.db).await,
            "dialog_settings": selected});
        let tl::enums::GlobalPrivacySettings::Settings(privacy) = invoke(
            &handle.client,
            None,
            tl::functions::account::GetGlobalPrivacySettings {},
            job,
        )
        .await?;
        archive.account["settings"]["telegram_privacy"] = json!({
            "archive_and_mute_new_noncontact_peers":privacy.archive_and_mute_new_noncontact_peers,
            "keep_archived_unmuted":privacy.keep_archived_unmuted,
            "keep_archived_folders":privacy.keep_archived_folders,
            "hide_read_marks":privacy.hide_read_marks,
            "new_noncontact_peers_require_premium":privacy.new_noncontact_peers_require_premium,
        });
        let mut notifications = serde_json::Map::new();
        for (name, peer) in [
            ("users", tl::enums::InputNotifyPeer::Users),
            ("groups", tl::enums::InputNotifyPeer::Chats),
            ("channels", tl::enums::InputNotifyPeer::Broadcasts),
        ] {
            let tl::enums::PeerNotifySettings::Settings(settings) = invoke(
                &handle.client,
                None,
                tl::functions::account::GetNotifySettings { peer },
                job,
            )
            .await?;
            notifications.insert(name.into(), json!({"show_previews":settings.show_previews,"silent":settings.silent,"mute_until":settings.mute_until,"stories_muted":settings.stories_muted,"stories_hide_sender":settings.stories_hide_sender}));
        }
        archive.account["settings"]["telegram_notifications"] = json!(notifications);
    }
    let mut metadata_bytes = 0usize;
    for dialog in selected {
        let metadata_before_chat = metadata_bytes;
        anyhow::ensure!(!job.is_cancelled(), "cancelled");
        job.log(format!("Reading {}", dialog.title)).await;
        let peer = dialog.peer.to_ref().context("invalid chat ID")?;
        let mut chat = Chat {
            id: dialog.id,
            title: dialog.title,
            kind: format!("{:?}", dialog.kind).to_lowercase(),
            username: dialog.username,
            avatar: None,
            messages: vec![],
            truncated: false,
        };
        let mut offset = 0;
        let mut inspected = 0usize;
        let mut seen = std::collections::HashSet::new();
        loop {
            let result = invoke(
                &handle.client,
                takeout,
                tl::functions::messages::GetHistory {
                    peer: peer.into(),
                    offset_id: offset,
                    offset_date: 0,
                    add_offset: 0,
                    limit: 100,
                    max_id: 0,
                    min_id: 0,
                    hash: 0,
                },
                job,
            )
            .await?;
            let (messages, users) = match result {
                tl::enums::messages::Messages::Messages(v) => (v.messages, v.users),
                tl::enums::messages::Messages::Slice(v) => (v.messages, v.users),
                tl::enums::messages::Messages::ChannelMessages(v) => (v.messages, v.users),
                _ => break,
            };
            if messages.is_empty() {
                break;
            }
            let mut names = HashMap::new();
            for user in users {
                if let tl::enums::User::User(user) = user {
                    names.insert(
                        user.id,
                        format!(
                            "{} {}",
                            user.first_name.unwrap_or_default(),
                            user.last_name.unwrap_or_default()
                        )
                        .trim()
                        .to_string(),
                    );
                }
            }
            let previous = offset;
            let mut passed_date = false;
            for raw in messages {
                let (id, message) = convert_message(raw, &names);
                if id <= 0 {
                    continue;
                }
                offset = if offset == 0 { id } else { offset.min(id) };
                if !seen.insert(id) {
                    continue;
                }
                inspected += 1;
                if let Some(message) = message {
                    if options
                        .from_date
                        .is_some_and(|from| message.timestamp < from)
                    {
                        passed_date = true;
                    }
                    if options.includes(&message) {
                        metadata_bytes += serde_json::to_vec(&message)?.len();
                        anyhow::ensure!(
                            metadata_bytes
                                <= options.max_total_bytes.min(128 * 1024 * 1024) as usize,
                            "text metadata limit reached; narrow the chat selection or date range"
                        );
                        chat.messages.push(message);
                    }
                }
                if inspected >= options.max_messages_per_chat {
                    chat.truncated = true;
                    break;
                }
            }
            if chat.truncated || passed_date || offset == previous {
                break;
            }
        }
        if options.method == Method::Custom && options.include_deleted {
            let deleted = crate::anti_delete::export_chat(
                &services.db,
                &account_key,
                &peer.id.to_string(),
                options.max_messages_per_chat,
            )
            .await?;
            merge_deleted(&mut chat.messages, deleted, &options);
        }
        metadata_bytes = metadata_before_chat + serde_json::to_vec(&chat)?.len();
        anyhow::ensure!(
            metadata_bytes <= options.max_total_bytes.min(128 * 1024 * 1024) as usize,
            "text metadata limit reached; narrow the chat selection or date range"
        );
        chat.messages.sort_by_key(|m| (m.timestamp, m.id));
        if chat.messages.len() > options.max_messages_per_chat {
            chat.messages
                .drain(..chat.messages.len() - options.max_messages_per_chat);
            chat.truncated = true;
        }
        archive.chats.push(chat);
        job.advance().await;
    }
    for archived in archived_chats {
        let Ok(id) = archived.id.parse::<i64>() else {
            continue;
        };
        if archive.chats.iter().any(|c| c.id == id)
            || options.excluded_chats.contains(&id)
            || (!options.chats.is_empty() && !options.chats.contains(&id))
        {
            continue;
        }
        let rows = crate::anti_delete::export_chat(
            &services.db,
            &account_key,
            &archived.id,
            options.max_messages_per_chat,
        )
        .await?;
        let mut messages = Vec::new();
        merge_deleted(&mut messages, rows, &options);
        metadata_bytes += serde_json::to_vec(&messages)?.len();
        anyhow::ensure!(
            metadata_bytes <= options.max_total_bytes.min(128 * 1024 * 1024) as usize,
            "text metadata limit reached; narrow the chat selection or date range"
        );
        messages.sort_by_key(|m| (m.timestamp, m.id));
        archive.chats.push(Chat {
            id,
            title: archived.title,
            kind: archived.kind,
            username: archived.username,
            avatar: None,
            truncated: messages.len() == options.max_messages_per_chat,
            messages,
        });
    }
    archive.chats.sort_by_key(|c| c.id);
    Ok(archive)
}

fn peer_id(peer: &tl::enums::Peer) -> i64 {
    match peer {
        tl::enums::Peer::User(p) => p.user_id,
        tl::enums::Peer::Chat(p) => -p.chat_id,
        tl::enums::Peer::Channel(p) => -1_000_000_000_000 - p.channel_id,
    }
}

fn convert_message(
    raw: tl::enums::Message,
    names: &HashMap<i64, String>,
) -> (i32, Option<Message>) {
    match raw {
        tl::enums::Message::Message(m) => {
            let sender_id = m.from_id.as_ref().map(peer_id);
            let entities = m
                .entities
                .unwrap_or_default()
                .into_iter()
                .filter_map(|e| match e {
                    tl::enums::MessageEntity::CustomEmoji(e) => Some(Entity {
                        kind: "custom_emoji".into(),
                        offset: e.offset,
                        length: e.length,
                        document_id: Some(e.document_id),
                    }),
                    _ => None,
                })
                .collect();
            let reply_to = m.reply_to.as_ref().and_then(|r| match r {
                tl::enums::MessageReplyHeader::Header(r) => r.reply_to_msg_id,
                _ => None,
            });
            let media = m.media.as_ref().and_then(describe_media);
            let message = Message {
                id: m.id,
                timestamp: i64::from(m.date),
                date: date(i64::from(m.date)),
                sender: sender_id
                    .and_then(|id| names.get(&id).cloned())
                    .or_else(|| sender_id.map(|id| id.to_string()))
                    .unwrap_or_default(),
                sender_id,
                text: m.message,
                outgoing: m.out,
                reply_to,
                edited_at: m.edit_date,
                entities,
                media,
                deleted_at: None,
            };
            (m.id, Some(message))
        }
        tl::enums::Message::Service(m) => (
            m.id,
            Some(Message {
                id: m.id,
                timestamp: i64::from(m.date),
                date: date(i64::from(m.date)),
                sender_id: m.from_id.as_ref().map(peer_id),
                text: format!("{:?}", m.action),
                ..Default::default()
            }),
        ),
        tl::enums::Message::Empty(m) => (m.id, None),
    }
}

fn date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn describe_media(raw: &tl::enums::MessageMedia) -> Option<Media> {
    let mut media = Media::default();
    match raw {
        tl::enums::MessageMedia::Photo(_) => media.kind = "photo".into(),
        tl::enums::MessageMedia::Document(m) => {
            let Some(tl::enums::Document::Document(d)) = &m.document else {
                return None;
            };
            media.kind = "file".into();
            media.mime_type = Some(d.mime_type.clone());
            media.size = u64::try_from(d.size).ok();
            for attribute in &d.attributes {
                match attribute {
                    tl::enums::DocumentAttribute::Filename(a) => {
                        media.name = Some(a.file_name.clone())
                    }
                    tl::enums::DocumentAttribute::Sticker(_) => media.kind = "sticker".into(),
                    tl::enums::DocumentAttribute::CustomEmoji(_) => {
                        media.kind = "custom_emoji".into()
                    }
                    tl::enums::DocumentAttribute::Video(a) => {
                        media.kind = if a.round_message {
                            "video_note"
                        } else {
                            "video"
                        }
                        .into()
                    }
                    tl::enums::DocumentAttribute::Audio(a) => {
                        media.kind = if a.voice { "voice" } else { "audio" }.into()
                    }
                    _ => {}
                }
            }
            if d.attributes
                .iter()
                .any(|a| matches!(a, tl::enums::DocumentAttribute::Animated))
            {
                media.kind = "gif".into();
            }
            if d.mime_type == "image/gif" {
                media.kind = "gif".into();
            }
            if d.attributes
                .iter()
                .any(|a| matches!(a, tl::enums::DocumentAttribute::Sticker(_)))
            {
                media.kind = "sticker".into();
            }
            if d.attributes
                .iter()
                .any(|a| matches!(a, tl::enums::DocumentAttribute::CustomEmoji(_)))
            {
                media.kind = "custom_emoji".into();
            }
        }
        tl::enums::MessageMedia::Empty | tl::enums::MessageMedia::WebPage(_) => return None,
        _ => {
            media.kind = "other".into();
            media.omitted = Some("metadata-only attachment".into());
        }
    }
    Some(media)
}

fn merge_deleted(messages: &mut Vec<Message>, deleted: Vec<Value>, options: &Options) {
    let mut by_id: BTreeMap<i32, Message> = std::mem::take(messages)
        .into_iter()
        .map(|m| (m.id, m))
        .collect();
    let mut seen_deleted = std::collections::HashSet::new();
    for value in deleted {
        let id = value["id"].as_i64().unwrap_or_default() as i32;
        if !seen_deleted.insert(id) {
            continue;
        }
        by_id.remove(&id);
        let sent = value["date"].as_str().unwrap_or_default();
        let timestamp = chrono::DateTime::parse_from_rfc3339(sent)
            .map(|d| d.timestamp())
            .unwrap_or_default();
        let mut message = Message {
            id,
            timestamp,
            date: sent.into(),
            text: value["text"].as_str().unwrap_or_default().into(),
            sender_id: value["sender_id"].as_str().and_then(|s| s.parse().ok()),
            sender: value["sender_id"].as_str().unwrap_or_default().into(),
            deleted_at: value["deleted_at"].as_str().map(str::to_owned),
            ..Default::default()
        };
        if let Some(kind) = value["media_type"].as_str() {
            message.media = Some(Media {
                kind: if kind == "image" { "photo" } else { kind }.into(),
                name: value["media_name"].as_str().map(str::to_owned),
                size: value["media_size"].as_u64(),
                source_path: value["media_path"].as_str().map(str::to_owned),
                ..Default::default()
            });
        }
        if options.includes(&message) {
            by_id.insert(id, message);
        }
    }
    *messages = by_id.into_values().collect();
}

pub async fn materialize(
    handle: &AccountHandle,
    archive: &mut Archive,
    root: &Path,
    job: &Job,
) -> Result<()> {
    let dialogs = account_tools::dialogs(handle).await?;
    let options = &archive.options;
    for chat in &mut archive.chats {
        let peer = dialogs
            .iter()
            .find(|d| d.id == chat.id)
            .and_then(|d| d.peer.to_ref());
        if options.avatars {
            if let Some(peer) = peer {
                if let Ok(profile) = handle.client.resolve_peer(peer).await {
                    if let Some(photo) = profile.photo(true).await {
                        let relative = format!("media/chat-{}-avatar.jpg", chat.id);
                        let path = root.join(&relative);
                        let limit = options
                            .max_file_bytes
                            .min(options.max_total_bytes.saturating_sub(archive.bytes));
                        match save_photo(&handle.client, &photo, &path, limit, job).await {
                            Ok(bytes) => {
                                archive.bytes += bytes;
                                chat.avatar = Some(relative);
                            }
                            Err(error) => {
                                let _ = tokio::fs::remove_file(path).await;
                                archive
                                    .warnings
                                    .push(format!("Chat {} avatar: {error}", chat.id));
                            }
                        }
                    }
                }
            }
        }
        for message in &mut chat.messages {
            anyhow::ensure!(!job.is_cancelled(), "cancelled");
            let Some(media) = &mut message.media else {
                continue;
            };
            if !options.permits_media(&media.kind) {
                media.omitted = Some("excluded by export settings".into());
                continue;
            }
            if media.omitted.is_some() {
                continue;
            }
            let remaining = options.max_total_bytes.saturating_sub(archive.bytes);
            let limit = options.max_file_bytes.min(remaining);
            if limit == 0 || media.size.is_some_and(|s| s > limit) {
                media.omitted = Some("size limit".into());
                continue;
            }
            let ext = match media.mime_type.as_deref() {
                Some("image/gif") => "gif",
                Some("image/webp") => "webp",
                Some("image/png") => "png",
                Some("video/webm") => "webm",
                Some("audio/mpeg") => "mp3",
                Some("application/x-tgsticker") => "tgs",
                _ => match media.kind.as_str() {
                    "photo" => "jpg",
                    "video" | "video_note" | "gif" => "mp4",
                    "voice" | "audio" => "ogg",
                    _ => "bin",
                },
            };
            let relative = format!("media/{}-{}.{ext}", chat.id, message.id);
            let path = root.join(&relative);
            let result = async {
                if let Some(source) = &media.source_path {
                    let source = Path::new(source).canonicalize()?;
                    let allowed = Path::new("data/deleted_media").canonicalize()?;
                    anyhow::ensure!(source.starts_with(allowed), "invalid archived media path");
                    let bytes = tokio::fs::metadata(&source).await?.len();
                    anyhow::ensure!(bytes <= limit, "size limit");
                    tokio::fs::copy(source, &path).await?;
                    return Ok::<_, anyhow::Error>(bytes);
                }
                anyhow::ensure!(
                    message.deleted_at.is_none(),
                    "deleted attachment was not archived"
                );
                let peer = peer.context("chat is unavailable")?;
                let mut found = handle
                    .client
                    .get_messages_by_id(peer, &[message.id])
                    .await?;
                let message = found
                    .pop()
                    .flatten()
                    .context("message is no longer available")?;
                let attachment = message.media().context("attachment is unavailable")?;
                let mut stream = handle.client.iter_download(&attachment);
                let mut file = tokio::fs::File::create(&path).await?;
                let mut bytes = 0u64;
                while let Some(chunk) = stream.next().await? {
                    anyhow::ensure!(!job.is_cancelled(), "cancelled");
                    bytes += chunk.len() as u64;
                    anyhow::ensure!(bytes <= limit, "size limit");
                    file.write_all(&chunk).await?;
                }
                Ok(bytes)
            };
            match tokio::time::timeout(Duration::from_secs(300), result).await {
                Ok(Ok(bytes)) => {
                    archive.bytes += bytes;
                    media.size = Some(bytes);
                    media.path = Some(relative);
                }
                other => {
                    let _ = tokio::fs::remove_file(&path).await;
                    media.omitted = Some(match other {
                        Ok(Err(e)) => e.to_string(),
                        _ => "download timeout".into(),
                    });
                }
            }
        }
    }
    if options.avatars {
        let mut photos = handle
            .client
            .iter_profile_photos(crate::telegram::saved_messages());
        let mut paths = Vec::new();
        while let Some(photo) = tokio::time::timeout(Duration::from_secs(120), photos.next())
            .await
            .context("avatar request timed out")??
        {
            anyhow::ensure!(!job.is_cancelled(), "cancelled");
            let relative = format!("media/avatar-{}.jpg", paths.len());
            let path = root.join(&relative);
            let limit = options
                .max_file_bytes
                .min(options.max_total_bytes.saturating_sub(archive.bytes));
            let bytes = match save_photo(&handle.client, &photo, &path, limit, job).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let _ = tokio::fs::remove_file(path).await;
                    archive.warnings.push(format!("Avatar omitted: {error}"));
                    break;
                }
            };
            archive.bytes += bytes;
            paths.push(relative);
        }
        archive.account["avatars"] = json!(paths);
    }
    Ok(())
}

async fn save_photo(
    client: &Client,
    photo: &grammers_client::media::Photo,
    path: &Path,
    limit: u64,
    job: &Job,
) -> Result<u64> {
    anyhow::ensure!(limit > 0, "size limit");
    tokio::time::timeout(Duration::from_secs(120), async {
        let mut stream = client.iter_download(photo);
        let mut file = tokio::fs::File::create(path).await?;
        let mut bytes = 0u64;
        while let Some(chunk) = stream.next().await? {
            anyhow::ensure!(!job.is_cancelled(), "cancelled");
            bytes += chunk.len() as u64;
            anyhow::ensure!(bytes <= limit, "size limit");
            file.write_all(&chunk).await?;
        }
        Ok(bytes)
    })
    .await
    .context("avatar download timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deleted(id: i32, text: &str) -> Value {
        json!({"id":id,"text":text,"date":"2026-01-01T00:00:00Z","deleted_at":"2026-01-02T00:00:00Z","sender_id":"7","media_type":"image","media_path":"data/deleted_media/account/photo.jpg"})
    }

    #[test]
    fn deleted_snapshot_replaces_live_message_and_keeps_only_newest_version() {
        let mut messages = vec![Message {
            id: 42,
            text: "live".into(),
            ..Default::default()
        }];
        merge_deleted(
            &mut messages,
            vec![deleted(42, "latest"), deleted(42, "older")],
            &Options::default(),
        );
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "latest");
        assert!(messages[0].deleted_at.is_some());
        let media = messages[0].media.as_ref().unwrap();
        assert_eq!(media.kind, "photo");
        assert!(media.path.is_none());
        assert!(media.source_path.is_some());
    }

    #[test]
    fn rejected_deletion_does_not_reintroduce_an_older_version() {
        let mut messages = vec![Message {
            id: 42,
            text: "previous live text".into(),
            ..Default::default()
        }];
        let options = Options {
            exclude_text: vec!["secret".into()],
            ..Default::default()
        };
        merge_deleted(
            &mut messages,
            vec![deleted(42, "SECRET newest"), deleted(42, "older")],
            &options,
        );
        assert!(messages.is_empty());
    }
}
