use std::path::Path;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

use super::model::{AccountInfo, ChatExport, ExportManifest, MessageExport};
use super::rules::MediaKind;
use super::text::desktop_entities;

/// The placeholder Telegram Desktop writes for files left out of an export; I write the same.
pub const DESKTOP_SKIPPED: &str =
    "(File not included. Change data exporting settings to download.)";

pub async fn write_chat(root: &Path, chat_dir: &str, chat: &ChatExport) -> Result<String> {
    let relative = format!("{chat_dir}/messages.json");
    let path = root.join(&relative);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&path, serde_json::to_vec_pretty(chat)?).await?;
    Ok(relative)
}

pub fn chat_summary(chat: &ChatExport, file: Option<&str>, page: Option<&str>) -> Value {
    json!({
        "id": chat.id,
        "title": chat.title,
        "kind": chat.kind,
        "username": chat.username,
        "about": chat.about,
        "members": chat.members,
        "avatar": chat.avatar,
        "messages": chat.messages.len(),
        "stats": chat.stats,
        "json": file,
        "html": page,
    })
}

pub async fn write_summary(
    root: &Path,
    manifest: &ExportManifest,
    account: Option<&AccountInfo>,
    chats: &[Value],
) -> Result<String> {
    let value = json!({
        "generator": format!("fly-telegram {}", crate::VERSION),
        "export": manifest,
        "account": account,
        "chats": chats,
    });
    tokio::fs::write(root.join("export.json"), serde_json::to_vec_pretty(&value)?).await?;
    Ok("export.json".into())
}

/// I stream `result.json` in the Telegram Desktop layout so large histories never sit in memory.
pub struct DesktopWriter {
    file: tokio::fs::File,
    chats_written: usize,
}

impl DesktopWriter {
    pub async fn create(root: &Path, account: Option<&AccountInfo>) -> Result<Self> {
        let mut file = tokio::fs::File::create(root.join("result.json")).await?;
        let mut head = serde_json::Map::new();
        head.insert(
            "about".into(),
            "Here is the data you requested. Remember: Telegram is ad free, it doesn't sell your data to anyone.".into(),
        );
        if let Some(account) = account {
            head.insert(
                "personal_information".into(),
                json!({
                    "user_id": account.id,
                    "first_name": account.first_name,
                    "last_name": account.last_name,
                    "phone_number": account.phone.clone().unwrap_or_default(),
                    "username": account.username.as_ref().map(|u| format!("@{u}")).unwrap_or_default(),
                    "bio": account.about.clone().unwrap_or_default(),
                }),
            );
            head.insert(
                "profile_pictures".into(),
                Value::Array(
                    account
                        .avatars
                        .iter()
                        .map(|avatar| {
                            json!({
                                "date": desktop_date(avatar.date),
                                "date_unixtime": avatar.date.to_string(),
                                "photo": avatar.file.clone().unwrap_or_else(|| DESKTOP_SKIPPED.to_string()),
                            })
                        })
                        .collect(),
                ),
            );
            head.insert(
                "contacts".into(),
                json!({
                    "about": "This is your contact list.",
                    "list": account.contacts.iter().map(|contact| {
                        let (first, last) = contact.name.split_once(' ').unwrap_or((contact.name.as_str(), ""));
                        json!({
                            "user_id": contact.id,
                            "first_name": first,
                            "last_name": last,
                            "phone_number": contact.phone.clone().unwrap_or_default(),
                        })
                    }).collect::<Vec<_>>(),
                }),
            );
            head.insert(
                "sessions".into(),
                json!({
                    "about": "We store this to display your connected devices.",
                    "list": account.sessions.iter().map(|session| json!({
                        "last_active": desktop_date(session.active),
                        "last_active_unixtime": session.active.to_string(),
                        "last_ip": session.ip,
                        "last_country": session.location,
                        "application_name": session.app,
                        "device_model": session.device,
                        "platform": session.platform,
                        "system_version": session.system,
                        "created": desktop_date(session.created),
                        "created_unixtime": session.created.to_string(),
                    })).collect::<Vec<_>>(),
                }),
            );
        }
        let mut text = serde_json::to_string_pretty(&Value::Object(head))?;
        text.truncate(text.trim_end().len() - 1);
        text.push_str(",\n \"chats\": {\n  \"about\": \"This page lists all chats from this export.\",\n  \"list\": [\n");
        file.write_all(text.as_bytes()).await?;
        Ok(Self {
            file,
            chats_written: 0,
        })
    }

    pub async fn add_chat(&mut self, chat: &ChatExport) -> Result<()> {
        let value = json!({
            "name": chat.title,
            "type": desktop_chat_type(&chat.kind),
            "id": chat.id.abs() % 1_000_000_000_000,
            "messages": chat.messages.iter().map(|m| desktop_message(m, chat)).collect::<Vec<_>>(),
        });
        let mut text = String::new();
        if self.chats_written > 0 {
            text.push_str(",\n");
        }
        text.push_str(&serde_json::to_string_pretty(&value)?);
        self.file.write_all(text.as_bytes()).await?;
        self.chats_written += 1;
        Ok(())
    }

    pub async fn finish(mut self) -> Result<String> {
        self.file.write_all(b"\n  ]\n }\n}\n").await?;
        self.file.flush().await?;
        Ok("result.json".into())
    }
}

fn desktop_date(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default()
}

fn desktop_chat_type(kind: &str) -> &'static str {
    match kind {
        "saved" => "saved_messages",
        "bot" => "bot_chat",
        "private" => "personal_chat",
        "group" => "private_group",
        "supergroup" => "private_supergroup",
        "channel" => "public_channel",
        _ => "personal_chat",
    }
}

fn desktop_peer_id(id: i64) -> String {
    if id > 0 {
        format!("user{id}")
    } else if id < -1_000_000_000_000 {
        format!("channel{}", -id - 1_000_000_000_000)
    } else {
        format!("chat{}", -id)
    }
}

fn desktop_message(message: &MessageExport, chat: &ChatExport) -> Value {
    let mut value = json!({
        "id": message.id,
        "type": if message.service.is_some() { "service" } else { "message" },
        "date": desktop_date(message.date),
        "date_unixtime": message.date.to_string(),
    });
    let from = message
        .from_name
        .clone()
        .unwrap_or_else(|| chat.title.clone());
    if let Some(action) = &message.service {
        value["actor"] = from.into();
        if let Some(id) = message.from_id {
            value["actor_id"] = desktop_peer_id(id).into();
        }
        value["action"] = action.clone().into();
    } else {
        value["from"] = from.into();
        if let Some(id) = message.from_id {
            value["from_id"] = desktop_peer_id(id).into();
        }
    }
    if let Some(edited) = message.edited {
        value["edited"] = desktop_date(edited).into();
        value["edited_unixtime"] = edited.to_string().into();
    }
    if let Some(reply) = message.reply_to {
        value["reply_to_message_id"] = reply.into();
    }
    if let Some(forwarded) = &message.forwarded_from {
        value["forwarded_from"] = forwarded.clone().into();
    }
    if let Some(media) = &message.media {
        let file = media
            .file
            .clone()
            .unwrap_or_else(|| DESKTOP_SKIPPED.to_string());
        match media.kind {
            Some(MediaKind::Photo) => {
                value["photo"] = file.into();
                if let Some(w) = media.width {
                    value["width"] = w.into();
                }
                if let Some(h) = media.height {
                    value["height"] = h.into();
                }
            }
            Some(kind) => {
                value["file"] = file.into();
                if let Some(name) = &media.name {
                    value["file_name"] = name.clone().into();
                }
                if let Some(size) = media.size {
                    value["file_size"] = size.into();
                }
                let media_type = match kind {
                    MediaKind::Sticker => Some("sticker"),
                    MediaKind::Gif => Some("animation"),
                    MediaKind::Video => Some("video_file"),
                    MediaKind::VideoNote => Some("video_message"),
                    MediaKind::Voice => Some("voice_message"),
                    MediaKind::Audio => Some("audio_file"),
                    MediaKind::File | MediaKind::Photo => None,
                };
                if let Some(media_type) = media_type {
                    value["media_type"] = media_type.into();
                }
                if let Some(emoji) = &media.emoji {
                    value["sticker_emoji"] = emoji.clone().into();
                }
                if let Some(mime) = &media.mime {
                    value["mime_type"] = mime.clone().into();
                }
                if let Some(duration) = media.duration {
                    value["duration_seconds"] = (duration.round() as i64).into();
                }
                if let Some(w) = media.width {
                    value["width"] = w.into();
                }
                if let Some(h) = media.height {
                    value["height"] = h.into();
                }
            }
            None => {
                value["media_type"] = media.type_name.clone().into();
                if let Some(details) = &media.details {
                    value["media_details"] = details.clone().into();
                }
            }
        }
    }
    let entities = desktop_entities(&message.text, &message.entities);
    value["text"] = if message.entities.is_empty() {
        Value::String(message.text.clone())
    } else {
        Value::Array(
            entities
                .iter()
                .map(|e| {
                    if e["type"] == "plain" {
                        e["text"].clone()
                    } else {
                        e.clone()
                    }
                })
                .collect(),
        )
    };
    value["text_entities"] = Value::Array(entities);
    if !message.reactions.is_empty() {
        value["reactions"] = Value::Array(
            message
                .reactions
                .iter()
                .map(|r| json!({ "type": "emoji", "count": r.count, "emoji": r.emoji }))
                .collect(),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::model::MediaExport;

    #[test]
    fn skipped_files_use_the_desktop_placeholder() {
        let chat = ChatExport {
            title: "Alex".into(),
            kind: "private".into(),
            ..ChatExport::default()
        };
        let message = MessageExport {
            id: 5,
            date: 1_700_000_000,
            text: "hi".into(),
            media: Some(MediaExport {
                type_name: "video".into(),
                kind: Some(MediaKind::Video),
                skipped: Some("excluded".into()),
                ..MediaExport::default()
            }),
            ..MessageExport::default()
        };
        let value = desktop_message(&message, &chat);
        assert_eq!(value["file"], DESKTOP_SKIPPED);
        assert_eq!(value["media_type"], "video_file");
        assert_eq!(value["from"], "Alex");
    }

    #[tokio::test]
    async fn result_json_is_valid() {
        let root = std::env::temp_dir().join(format!("fly_td_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&root).await.unwrap();
        let mut writer = DesktopWriter::create(&root, Some(&AccountInfo::default()))
            .await
            .unwrap();
        for _ in 0..2 {
            writer.add_chat(&ChatExport::default()).await.unwrap();
        }
        writer.finish().await.unwrap();
        let raw = tokio::fs::read_to_string(root.join("result.json"))
            .await
            .unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["chats"]["list"].as_array().unwrap().len(), 2);
        let _ = tokio::fs::remove_dir_all(root).await;
    }
}
