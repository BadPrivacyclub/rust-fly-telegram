use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use chrono::{Local, TimeZone};

use super::fetch::{capitalize, human_bytes, human_duration};
use super::model::*;
use super::rules::MediaKind;
use super::text::{escape_html as esc, to_html};

const CSS: &str = include_str!("assets/export.css");
const JS: &str = include_str!("assets/export.js");

pub async fn write_assets(root: &Path) -> Result<()> {
    let dir = root.join("assets");
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join("export.css"), CSS).await?;
    tokio::fs::write(dir.join("export.js"), JS).await?;
    Ok(())
}

fn icon(name: &str) -> String {
    let body = match name {
        "back" => r#"<path d="M15 18l-6-6 6-6"/>"#,
        "theme" => r#"<path d="M12 3a9 9 0 1 0 9 9 7 7 0 0 1-9-9z"/>"#,
        "photo" => {
            r#"<rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="10" r="2"/><path d="m21 16-5-5-9 9"/>"#
        }
        "video" => {
            r#"<rect x="3" y="6" width="13" height="12" rx="3"/><path d="m16 10 5-3v10l-5-3z"/>"#
        }
        "round" => r#"<circle cx="12" cy="12" r="9"/><path d="m10 9 5 3-5 3z"/>"#,
        "voice" => {
            r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M5 11a7 7 0 0 0 14 0M12 18v3"/>"#
        }
        "audio" => {
            r#"<path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/>"#
        }
        "gif" => {
            r#"<rect x="3" y="5" width="18" height="14" rx="3"/><path d="M8 10H7a2 2 0 0 0 0 4h1v-2M12 10v4M16 14v-4h2"/>"#
        }
        "sticker" => {
            r#"<path d="M15 3H6a3 3 0 0 0-3 3v12a3 3 0 0 0 3 3h7l8-8V6a3 3 0 0 0-3-3z"/><path d="M13 21v-5a3 3 0 0 1 3-3h5"/>"#
        }
        "file" => {
            r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#
        }
        "location" => {
            r#"<path d="M12 21s-7-6-7-11a7 7 0 0 1 14 0c0 5-7 11-7 11z"/><circle cx="12" cy="10" r="2.5"/>"#
        }
        "poll" => r#"<path d="M5 20V10M12 20V4M19 20v-7"/>"#,
        "contact" => r#"<circle cx="12" cy="8" r="4"/><path d="M4 21c1-4 4-6 8-6s7 2 8 6"/>"#,
        "link" => {
            r#"<path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>"#
        }
        "check" => r#"<path d="m5 12 4 4 10-10"/>"#,
        "checks" => r#"<path d="m2 12 4 4 10-10M10 16l1 1 10-10"/>"#,
        "eye" => {
            r#"<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#
        }
        "trash" => r#"<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>"#,
        "edit" => r#"<path d="M4 20h4L19 9l-4-4L4 16z"/>"#,
        "forward" => r#"<path d="M14 5l7 7-7 7"/><path d="M21 12H9a6 6 0 0 0-6 6"/>"#,
        "pin" => r#"<path d="M9 4h6l-1 6 4 4H6l4-4zM12 14v7"/>"#,
        "users" => {
            r#"<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18 14.8c2 .7 3.2 2.4 3.5 5.2"/>"#
        }
        "chart" => r#"<path d="M3 3v18h18"/><path d="M7 15l4-4 3 3 5-6"/>"#,
        "clock" => r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#,
        "shield" => r#"<path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/>"#,
        "device" => r#"<rect x="6" y="2" width="12" height="20" rx="3"/><path d="M11 18h2"/>"#,
        "messages" => r#"<path d="M4 5h16v11H8l-4 4z"/>"#,
        "star" => {
            r#"<path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z"/>"#
        }
        _ => r#"<circle cx="12" cy="12" r="9"/>"#,
    };
    format!(r#"<svg class="i" viewBox="0 0 24 24" aria-hidden="true">{body}</svg>"#)
}

fn media_icon(media: &MediaExport) -> &'static str {
    match media.kind {
        Some(MediaKind::Photo) => "photo",
        Some(MediaKind::Video) => "video",
        Some(MediaKind::VideoNote) => "round",
        Some(MediaKind::Voice) => "voice",
        Some(MediaKind::Audio) => "audio",
        Some(MediaKind::Gif) => "gif",
        Some(MediaKind::Sticker) => "sticker",
        Some(MediaKind::File) => "file",
        None => match media.type_name.as_str() {
            "location" | "live location" | "venue" => "location",
            "poll" => "poll",
            "contact" => "contact",
            "link preview" => "link",
            _ => "file",
        },
    }
}

pub fn local_time(unix: i64, format: &str) -> String {
    Local
        .timestamp_opt(unix, 0)
        .single()
        .map(|time| time.format(format).to_string())
        .unwrap_or_default()
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

fn avatar(path: Option<&str>, name: &str, base: &str, class: &str) -> String {
    match path {
        Some(path) => format!(
            r#"<img class="avatar {class}" src="{base}{}" alt="" loading="lazy">"#,
            esc(path)
        ),
        None => format!(
            r#"<div class="avatar {class}" aria-hidden="true">{}</div>"#,
            esc(&initials(name))
        ),
    }
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "saved" => "Saved Messages",
        "bot" => "Bot",
        "private" => "Private chat",
        "group" => "Group",
        "supergroup" => "Supergroup",
        "channel" => "Channel",
        _ => "Chat",
    }
}

pub struct PageMeta<'a> {
    pub base: &'a str,
    pub back: Option<&'a str>,
    pub account_name: &'a str,
    pub method: ExportMethod,
    pub requested_at: i64,
}

fn page_head(title: &str, base: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="generator" content="fly-telegram {version}">
<title>{title}</title>
<link rel="stylesheet" href="{base}assets/export.css">
</head>
<body>
"#,
        version = crate::VERSION,
        title = esc(title),
    )
}

fn page_end(base: &str, meta: &PageMeta<'_>) -> String {
    format!(
        r#"<footer class="note">{} · {} · fly-telegram {} · requested {}</footer>
</div>
<script src="{base}assets/export.js"></script>
</body>
</html>
"#,
        esc(meta.account_name),
        esc(meta.method.title()),
        crate::VERSION,
        esc(&local_time(meta.requested_at, "%d.%m.%Y %H:%M")),
    )
}

pub fn chat_page(chat: &ChatExport, meta: &PageMeta<'_>) -> String {
    let base = meta.base;
    let mut out = page_head(&chat.title, base);
    let subtitle = [
        chat.username.as_ref().map(|u| format!("@{u}")),
        Some(kind_label(&chat.kind).to_string()),
        chat.members.map(|n| format!("{n} members")),
        Some(format!("{} messages", chat.messages.len())),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let _ = write!(
        out,
        r#"<header class="top"><div class="top-inner">{back}{avatar}<div class="grow"><h1>{title}</h1><p>{subtitle}</p></div><button class="icon-btn" id="theme" title="Theme" type="button">{theme}</button></div></header>
<div class="wrap">
<nav class="tabs">
<button class="on" data-tab="messages" type="button">{i_msg}Messages</button>
<button data-tab="overview" type="button">{i_chart}Overview</button>
<button data-tab="media" type="button">{i_photo}Media</button>
<button data-tab="deleted" type="button">{i_trash}Deleted {badge}</button>
</nav>
"#,
        back = meta
            .back
            .map(|href| format!(
                r#"<a class="icon-btn" href="{}" title="All chats">{}</a>"#,
                esc(href),
                icon("back")
            ))
            .unwrap_or_default(),
        avatar = avatar(chat.avatar.as_deref(), &chat.title, base, ""),
        title = esc(&chat.title),
        subtitle = esc(&subtitle),
        theme = icon("theme"),
        i_msg = icon("messages"),
        i_chart = icon("chart"),
        i_photo = icon("photo"),
        i_trash = icon("trash"),
        badge = if chat.stats.deleted > 0 {
            format!(r#"<span class="badge">{}</span>"#, chat.stats.deleted)
        } else {
            String::new()
        },
    );

    out.push_str(r#"<section data-panel="messages">
<div class="toolbar"><input class="input" id="q" type="search" placeholder="Search messages" autocomplete="off">
<select class="input" id="filter"><option value="">All messages</option><option value="mine">Sent by me</option><option value="theirs">Received</option><option value="media">With media</option><option value="deleted">Deleted</option><option value="edited">Edited</option><option value="unread">Unread</option><option value="reply">Replies</option><option value="forwarded">Forwarded</option></select>
<span class="faint" id="count"></span></div>
<div class="timeline">
"#);
    let lookup = chat
        .messages
        .iter()
        .map(|m| (m.id, m))
        .collect::<HashMap<_, _>>();
    let group_like = matches!(chat.kind.as_str(), "group" | "supergroup");
    let mut last_day = String::new();
    let mut last_sender: Option<(Option<i64>, i64)> = None;
    for message in &chat.messages {
        let day = local_time(message.date, "%d %B %Y");
        if day != last_day {
            let _ = write!(out, r#"<div class="day">{}</div>"#, esc(&day));
            last_day = day;
            last_sender = None;
        }
        let continued = last_sender
            .is_some_and(|(from, date)| from == message.from_id && message.date - date < 300);
        last_sender = Some((message.from_id, message.date));
        out.push_str(&message_html(
            message, chat, &lookup, base, group_like, continued,
        ));
    }
    if chat.messages.is_empty() {
        out.push_str(r#"<p class="empty">No messages match the export rules.</p>"#);
    }
    out.push_str("</div>\n</section>\n");

    out.push_str(&overview_html(chat, base));
    out.push_str(&media_html(chat, base));
    out.push_str(&deleted_html(chat, base));
    out.push_str(&page_end(base, meta));
    out
}

fn message_html(
    message: &MessageExport,
    chat: &ChatExport,
    lookup: &HashMap<i32, &MessageExport>,
    base: &str,
    group_like: bool,
    continued: bool,
) -> String {
    let mut flags = vec![if message.out { "mine" } else { "theirs" }];
    if message.media.is_some() {
        flags.push("media");
    }
    if message.deleted.is_some() {
        flags.push("deleted");
    }
    if message.edited.is_some() {
        flags.push("edited");
    }
    if message.reply_to.is_some() {
        flags.push("reply");
    }
    if message.forwarded_from.is_some() {
        flags.push("forwarded");
    }
    let unread = !message.out && message.service.is_none() && message.id > chat.read_inbox_max_id;
    if unread {
        flags.push("unread");
    }
    let search = format!(
        "{} {} {}",
        message.text,
        message.from_name.as_deref().unwrap_or(""),
        message
            .media
            .as_ref()
            .map(|m| format!(
                "{} {}",
                m.name.as_deref().unwrap_or(""),
                m.details.as_deref().unwrap_or("")
            ))
            .unwrap_or_default()
    )
    .to_lowercase();

    if let Some(action) = &message.service {
        return format!(
            r#"<div class="service{}" id="m{}" data-search="{}" data-flags="{}">{}{} · {}{}</div>
"#,
            if message.deleted.is_some() {
                " deleted"
            } else {
                ""
            },
            message.id,
            esc(&search),
            flags.join(" "),
            message
                .from_name
                .as_ref()
                .map(|n| format!("<b>{}</b>: ", esc(n)))
                .unwrap_or_default(),
            esc(action),
            esc(&local_time(message.date, "%H:%M")),
            deleted_badge(message),
        );
    }

    let participant = message.from_id.and_then(|id| chat.participants.get(&id));
    let show_avatar = group_like && !message.out;
    let mut out = format!(
        r#"<article class="msg {}{}{}" id="m{}" data-search="{}" data-flags="{}">"#,
        if message.out { "out" } else { "in" },
        if message.deleted.is_some() {
            " deleted"
        } else {
            ""
        },
        if continued { " cont" } else { "" },
        message.id,
        esc(&search),
        flags.join(" "),
    );
    if show_avatar {
        out.push_str(&avatar(
            participant.and_then(|p| p.avatar.as_deref()),
            message.from_name.as_deref().unwrap_or("?"),
            base,
            "sm",
        ));
    }
    out.push_str(r#"<div class="bubble">"#);
    if (group_like || chat.kind == "channel") && !message.out && !continued {
        let _ = write!(
            out,
            r#"<div class="sender">{}</div>"#,
            esc(message.from_name.as_deref().unwrap_or("Unknown"))
        );
    }
    if let Some(forwarded) = &message.forwarded_from {
        let _ = write!(
            out,
            r#"<div class="fwd">{}Forwarded from <b>{}</b></div>"#,
            icon("forward"),
            esc(forwarded)
        );
    }
    if let Some(reply_id) = message.reply_to {
        match lookup.get(&reply_id) {
            Some(original) => {
                let snippet = if original.text.is_empty() {
                    original
                        .media
                        .as_ref()
                        .map(|m| capitalize(&m.type_name))
                        .or_else(|| original.service.clone())
                        .unwrap_or_default()
                } else {
                    original.text.chars().take(90).collect()
                };
                let _ = write!(
                    out,
                    r##"<a class="reply" href="#m{reply_id}" data-jump="m{reply_id}"><b>{}</b>{}</a>"##,
                    esc(original.from_name.as_deref().unwrap_or("Message")),
                    esc(&snippet)
                );
            }
            None => {
                let _ = write!(
                    out,
                    r#"<span class="reply"><b>Reply</b>to a message outside this export</span>"#
                );
            }
        }
    }
    if let Some(media) = &message.media {
        out.push_str(&media_block(media, base));
    }
    if !message.text.is_empty() {
        let _ = write!(
            out,
            r#"<div class="text">{}</div>"#,
            to_html(&message.text, &message.entities)
        );
    }
    if !message.reactions.is_empty() {
        out.push_str(r#"<div class="reactions">"#);
        for reaction in &message.reactions {
            let _ = write!(
                out,
                r#"<span class="{}">{} {}</span>"#,
                if reaction.chosen { "chosen" } else { "" },
                esc(&reaction.emoji),
                reaction.count
            );
        }
        out.push_str("</div>");
    }
    out.push_str(r#"<div class="meta">"#);
    out.push_str(&deleted_badge(message));
    if message.pinned {
        let _ = write!(out, r#"<span title="Pinned">{}</span>"#, icon("pin"));
    }
    if let Some(views) = message.views {
        let _ = write!(out, r#"<span>{} {views}</span>"#, icon("eye"));
    }
    if let Some(edited) = message.edited {
        let _ = write!(
            out,
            r#"<span title="Edited {}">edited</span>"#,
            esc(&local_time(edited, "%d.%m.%Y %H:%M"))
        );
    }
    if unread {
        out.push_str(r#"<span class="unread-dot" title="Unread"></span>"#);
    }
    let _ = write!(
        out,
        r#"<time title="{}">{}</time>"#,
        esc(&local_time(message.date, "%d.%m.%Y %H:%M:%S")),
        esc(&local_time(message.date, "%H:%M"))
    );
    if let Some(read) = message.read {
        let _ = write!(
            out,
            r#"<span class="ticks{}" title="{}">{}</span>"#,
            if read { " read" } else { "" },
            if read {
                "Read"
            } else {
                "Delivered, not read yet"
            },
            icon(if read { "checks" } else { "check" })
        );
    }
    out.push_str("</div></div></article>\n");
    out
}

fn deleted_badge(message: &MessageExport) -> String {
    match &message.deleted {
        Some(mark) => format!(
            r#"<span class="deleted-badge" title="Restored from the anti-delete archive">{}Deleted {}</span>"#,
            icon("trash"),
            esc(&local_time(mark.deleted_at, "%d.%m.%Y %H:%M"))
        ),
        None => String::new(),
    }
}

fn media_block(media: &MediaExport, base: &str) -> String {
    let summary = media_summary(media);
    let Some(file) = media.file.as_deref() else {
        let reason = media
            .skipped
            .clone()
            .or_else(|| media.details.clone())
            .unwrap_or_default();
        if media.kind.is_none() {
            return format!(
                r#"<div class="media"><div class="file-card">{}<div><b>{}</b><div class="details">{}</div></div></div></div>"#,
                format_args!(r#"<span class="ico">{}</span>"#, icon(media_icon(media))),
                esc(&capitalize(&media.type_name)),
                esc(&reason)
            );
        }
        return format!(
            r#"<div class="media"><div class="placeholder"><span class="ico">{}</span><div><b>{}</b><small>{}</small><br><small>{}</small></div></div></div>"#,
            icon(media_icon(media)),
            esc(&capitalize(&media.type_name)),
            esc(&summary),
            esc(&reason)
        );
    };
    let src = format!("{base}{}", esc(file));
    let is_tgs = file.ends_with(".tgs");
    match media.kind {
        Some(MediaKind::Photo) => format!(
            r#"<div class="media"><a href="{src}"><img src="{src}" alt="Photo" loading="lazy"></a></div>"#
        ),
        Some(MediaKind::Sticker) if !is_tgs && !file.ends_with(".webm") => format!(
            r#"<div class="media sticker"><img src="{src}" alt="{}" loading="lazy"></div>"#,
            esc(media.emoji.as_deref().unwrap_or("Sticker"))
        ),
        Some(MediaKind::Sticker) if file.ends_with(".webm") => format!(
            r#"<div class="media sticker"><video src="{src}" autoplay loop muted playsinline></video></div>"#
        ),
        Some(MediaKind::Gif) => format!(
            r#"<div class="media"><video src="{src}" autoplay loop muted playsinline preload="metadata"></video></div>"#
        ),
        Some(MediaKind::Video) => format!(
            r#"<div class="media"><video src="{src}" controls preload="metadata"></video></div>"#
        ),
        Some(MediaKind::VideoNote) => format!(
            r#"<div class="media round"><video src="{src}" controls preload="metadata"></video></div>"#
        ),
        Some(MediaKind::Voice | MediaKind::Audio) => format!(
            r#"<div class="media"><audio src="{src}" controls preload="none"></audio><div class="faint">{}</div></div>"#,
            esc(&summary)
        ),
        _ if media.is_visual() && !is_tgs => format!(
            r#"<div class="media"><a href="{src}"><img src="{src}" alt="" loading="lazy"></a></div>"#
        ),
        _ => format!(
            r#"<div class="media"><a class="file-card" href="{src}" download><span class="ico">{}</span><div><b>{}</b><small>{}</small></div></a></div>"#,
            icon(media_icon(media)),
            esc(media
                .name
                .as_deref()
                .unwrap_or(if is_tgs { "Animated sticker" } else { "File" })),
            esc(&summary)
        ),
    }
}

fn media_summary(media: &MediaExport) -> String {
    [
        media.emoji.clone(),
        media.details.clone().filter(|_| media.kind.is_some()),
        media.duration.map(human_duration),
        media.size.map(human_bytes),
        match (media.width, media.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => Some(format!("{w}×{h}")),
            _ => None,
        },
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ")
}

fn overview_html(chat: &ChatExport, base: &str) -> String {
    let stats = &chat.stats;
    let mut out = String::from(r#"<section data-panel="overview" class="hidden stack">"#);
    let stat = |value: String, label: &str| {
        format!(
            r#"<div class="card stat"><b>{}</b><span>{}</span></div>"#,
            esc(&value),
            esc(label)
        )
    };
    out.push_str(r#"<div class="grid">"#);
    out.push_str(&stat(stats.total.to_string(), "messages"));
    out.push_str(&stat(stats.outgoing.to_string(), "sent by me"));
    out.push_str(&stat(stats.incoming.to_string(), "received"));
    out.push_str(&stat(stats.active_days.to_string(), "active days"));
    out.push_str(&stat(stats.edited.to_string(), "edited"));
    out.push_str(&stat(stats.deleted.to_string(), "deleted, restored"));
    out.push_str(&stat(stats.replies.to_string(), "replies"));
    out.push_str(&stat(stats.words.to_string(), "words"));
    out.push_str("</div>");

    out.push_str(r#"<div class="grid wide">"#);
    let _ = write!(
        out,
        r#"<div class="card"><h2>{}About</h2><dl class="kv">"#,
        icon("users")
    );
    let mut kv = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let _ = write!(out, "<dt>{}</dt><dd>{}</dd>", esc(label), esc(&value));
        }
    };
    kv("Type", Some(kind_label(&chat.kind).to_string()));
    kv("ID", Some(chat.id.to_string()));
    kv("Username", chat.username.as_ref().map(|u| format!("@{u}")));
    kv("Members", chat.members.map(|n| n.to_string()));
    kv("Description", chat.about.clone());
    kv(
        "First message",
        stats.first_date.map(|d| local_time(d, "%d.%m.%Y %H:%M")),
    );
    kv(
        "Last message",
        stats.last_date.map(|d| local_time(d, "%d.%m.%Y %H:%M")),
    );
    kv("Unread", Some(chat.unread.to_string()));
    kv(
        "State",
        Some(
            [
                chat.pinned.then_some("pinned"),
                chat.archived.then_some("archived"),
                chat.muted.then_some("muted"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", "),
        ),
    );
    out.push_str("</dl></div>");

    let _ = write!(
        out,
        r#"<div class="card"><h2>{}Interaction</h2><dl class="kv">"#,
        icon("clock")
    );
    let mut kv = |label: &str, value: Option<String>| {
        if let Some(value) = value {
            let _ = write!(out, "<dt>{}</dt><dd>{}</dd>", esc(label), esc(&value));
        }
    };
    kv(
        "My typical reply time",
        stats.my_reply_secs.map(readable_gap),
    );
    kv(
        "Their typical reply time",
        stats.their_reply_secs.map(readable_gap),
    );
    kv(
        "Share of my messages",
        (stats.total > 0).then(|| format!("{}%", stats.outgoing * 100 / stats.total)),
    );
    kv("Forwarded", Some(stats.forwarded.to_string()));
    kv("Service events", Some(stats.service.to_string()));
    kv(
        "Read by them",
        Some(match chat.kind.as_str() {
            "private" | "bot" => {
                let mine = chat.messages.iter().filter(|m| m.out).count();
                let read = chat
                    .messages
                    .iter()
                    .filter(|m| m.read == Some(true))
                    .count();
                format!("{read} of {mine} sent messages")
            }
            _ => "not tracked for groups".to_string(),
        }),
    );
    out.push_str("</dl></div></div>");

    let max_hour = stats.by_hour.iter().copied().max().unwrap_or(0).max(1);
    let _ = write!(
        out,
        r#"<div class="card"><h2>{}Activity by hour</h2><div class="hours">"#,
        icon("chart")
    );
    for (hour, count) in stats.by_hour.iter().enumerate() {
        let _ = write!(
            out,
            r#"<div style="height:{}%" title="{hour}:00 · {count}"></div>"#,
            count * 100 / max_hour
        );
    }
    out.push_str(r#"</div><div class="hours-legend">"#);
    for hour in 0..24 {
        let _ = write!(
            out,
            "<span>{}</span>",
            if hour % 3 == 0 {
                hour.to_string()
            } else {
                String::new()
            }
        );
    }
    out.push_str("</div></div>");

    out.push_str(r#"<div class="grid wide">"#);
    let days = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    out.push_str(&bars_card(
        "By weekday",
        &days
            .iter()
            .zip(stats.by_weekday.iter())
            .map(|(d, n)| (d.to_string(), *n as i64))
            .collect::<Vec<_>>(),
    ));
    out.push_str(&bars_card(
        "Top senders",
        &stats
            .top_senders
            .iter()
            .map(|(id, n)| {
                (
                    chat.participants
                        .get(id)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| id.to_string()),
                    *n as i64,
                )
            })
            .collect::<Vec<_>>(),
    ));
    out.push_str(&bars_card(
        "Media",
        &stats
            .media
            .iter()
            .map(|(k, n)| (capitalize(k), *n as i64))
            .collect::<Vec<_>>(),
    ));
    let mut reactions = stats
        .reactions
        .iter()
        .map(|(k, n)| (k.clone(), *n))
        .collect::<Vec<_>>();
    reactions.sort_by_key(|r| std::cmp::Reverse(r.1));
    reactions.truncate(10);
    out.push_str(&bars_card("Reactions", &reactions));
    out.push_str("</div>");

    if !chat.participants.is_empty() && chat.kind != "private" {
        let _ = write!(
            out,
            r#"<div class="card"><h2>{}Participants in this export</h2><div class="grid">"#,
            icon("users")
        );
        for participant in chat.participants.values().take(200) {
            let _ = write!(
                out,
                r#"<div class="chat-card">{}<div class="grow"><b>{}</b><span class="faint">{}</span></div></div>"#,
                avatar(participant.avatar.as_deref(), &participant.name, base, "sm"),
                esc(&participant.name),
                esc(&participant
                    .username
                    .as_ref()
                    .map(|u| format!("@{u}"))
                    .unwrap_or_else(|| if participant.bot {
                        "bot".into()
                    } else {
                        String::new()
                    }))
            );
        }
        out.push_str("</div></div>");
    }
    out.push_str("</section>\n");
    out
}

fn readable_gap(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

fn bars_card(title: &str, rows: &[(String, i64)]) -> String {
    let mut out = format!(
        r#"<div class="card"><h2>{}</h2><div class="bars">"#,
        esc(title)
    );
    if rows.is_empty() || rows.iter().all(|(_, n)| *n == 0) {
        out.push_str(r#"<span class="faint">Nothing yet</span>"#);
    }
    let max = rows.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
    for (label, value) in rows {
        let _ = write!(
            out,
            r#"<div class="bar"><span>{}</span><div class="track"><div class="fill" style="width:{}%"></div></div><span class="n">{value}</span></div>"#,
            esc(label),
            value * 100 / max
        );
    }
    out.push_str("</div></div>");
    out
}

fn media_html(chat: &ChatExport, base: &str) -> String {
    let mut out = String::from(r#"<section data-panel="media" class="hidden">"#);
    let items = chat
        .messages
        .iter()
        .filter_map(|m| m.media.as_ref().map(|media| (m, media)))
        .filter(|(_, media)| media.file.is_some())
        .collect::<Vec<_>>();
    if items.is_empty() {
        out.push_str(r#"<p class="empty">No saved media in this chat.</p>"#);
    } else {
        out.push_str(r#"<div class="gallery">"#);
        for (message, media) in items {
            let file = esc(media.file.as_deref().unwrap_or_default());
            let preview = match media.kind {
                Some(MediaKind::Photo | MediaKind::Sticker)
                    if !file.ends_with(".tgs") && !file.ends_with(".webm") =>
                {
                    format!(r#"<img src="{base}{file}" alt="" loading="lazy">"#)
                }
                Some(MediaKind::Video | MediaKind::Gif | MediaKind::VideoNote) => {
                    format!(r#"<video src="{base}{file}" preload="metadata" muted></video>"#)
                }
                _ => format!(
                    r#"<div class="file-card" style="height:100%;border-radius:0"><span class="ico">{}</span><div><b>{}</b></div></div>"#,
                    icon(media_icon(media)),
                    esc(media.name.as_deref().unwrap_or(&media.type_name))
                ),
            };
            let _ = write!(
                out,
                r##"<a href="#m{id}" data-jump="m{id}">{preview}<span class="kind">{}</span></a>"##,
                esc(&capitalize(&media.type_name)),
                id = message.id
            );
        }
        out.push_str("</div>");
    }
    out.push_str("</section>\n");
    out
}

fn deleted_html(chat: &ChatExport, _base: &str) -> String {
    let mut out = String::from(r#"<section data-panel="deleted" class="hidden stack">"#);
    let deleted = chat
        .messages
        .iter()
        .filter(|m| m.deleted.is_some())
        .collect::<Vec<_>>();
    if deleted.is_empty() {
        out.push_str(r#"<p class="empty">The anti-delete archive has nothing for this chat in the exported period.</p>"#);
    }
    for message in deleted {
        let _ = write!(
            out,
            r##"<div class="card"><div class="row"><b>{}</b> <span class="faint">sent {} · deleted {}</span></div><div class="text">{}</div>{}<p class="faint"><a href="#m{id}" data-jump="m{id}">Show in the chat</a></p></div>"##,
            esc(message.from_name.as_deref().unwrap_or("Unknown")),
            esc(&local_time(message.date, "%d.%m.%Y %H:%M")),
            esc(&message
                .deleted
                .as_ref()
                .map(|d| local_time(d.deleted_at, "%d.%m.%Y %H:%M"))
                .unwrap_or_default()),
            esc(&message.text),
            message
                .media
                .as_ref()
                .map(|m| format!(
                    r#"<p class="faint">{}: {}</p>"#,
                    esc(&capitalize(&m.type_name)),
                    esc(m.file.as_deref().or(m.skipped.as_deref()).unwrap_or(""))
                ))
                .unwrap_or_default(),
            id = message.id
        );
    }
    out.push_str("</section>\n");
    out
}

pub struct ChatCard {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub username: Option<String>,
    pub avatar: Option<String>,
    pub page: String,
    pub messages: usize,
    pub deleted: usize,
    pub media: usize,
    pub last_date: Option<i64>,
}

pub fn index_page(
    manifest: &ExportManifest,
    account: Option<&AccountInfo>,
    chats: &[ChatCard],
) -> String {
    let base = "";
    let meta = PageMeta {
        base,
        back: None,
        account_name: &manifest.account_name,
        method: manifest.method,
        requested_at: manifest.requested_at,
    };
    let mut out = page_head(&format!("{} · export", manifest.account_name), base);
    let _ = write!(
        out,
        r#"<header class="top"><div class="top-inner"><div class="grow"><h1>{}</h1><p>{} · {}</p></div><button class="icon-btn" id="theme" title="Theme" type="button">{}</button></div></header>
<div class="wrap">
"#,
        esc(&manifest.title),
        esc(manifest.method.title()),
        esc(&local_time(manifest.requested_at, "%d.%m.%Y %H:%M")),
        icon("theme")
    );
    let has_account = account.is_some();
    let _ = writeln!(
        out,
        r#"<nav class="tabs"><button class="on" data-tab="chats" type="button">{}Chats</button>{}<button data-tab="about" type="button">{}This export</button></nav>"#,
        icon("messages"),
        if has_account {
            format!(
                r#"<button data-tab="profile" type="button">{}Profile</button><button data-tab="sessions" type="button">{}Sessions</button><button data-tab="contacts" type="button">{}Contacts</button><button data-tab="privacy" type="button">{}Privacy</button>"#,
                icon("contact"),
                icon("device"),
                icon("users"),
                icon("shield")
            )
        } else {
            String::new()
        },
        icon("chart")
    );

    out.push_str(r#"<section data-panel="chats"><div class="toolbar"><input class="input" id="chat-q" type="search" placeholder="Search chats" autocomplete="off"><select class="input" id="chat-kind"><option value="">All chats</option><option value="private">Private</option><option value="group">Groups</option><option value="channel">Channels</option><option value="bot">Bots</option><option value="saved">Saved</option><option value="deleted">With deleted messages</option></select><span class="faint" id="chat-count"></span></div><div class="grid wide">"#);
    for chat in chats {
        let mut flags = vec![match chat.kind.as_str() {
            "supergroup" => "group",
            other => other,
        }];
        if chat.deleted > 0 {
            flags.push("deleted");
        }
        let _ = write!(
            out,
            r#"<a class="card chat-card" id="chat{id}" href="{page}" data-chat data-search="{search}" data-flags="{flags}">{avatar}<div class="grow"><b>{title}</b><span class="faint">{kind}{last}</span><div class="chips"><span class="chip">{messages} messages</span>{media}{deleted}</div></div></a>"#,
            id = chat.id,
            page = esc(&chat.page),
            search = esc(
                &format!("{} {}", chat.title, chat.username.as_deref().unwrap_or(""))
                    .to_lowercase()
            ),
            flags = flags.join(" "),
            avatar = avatar(chat.avatar.as_deref(), &chat.title, base, ""),
            title = esc(&chat.title),
            kind = kind_label(&chat.kind),
            last = chat
                .last_date
                .map(|d| format!(" · {}", local_time(d, "%d.%m.%Y")))
                .unwrap_or_default(),
            messages = chat.messages,
            media = if chat.media > 0 {
                format!(r#"<span class="chip">{} media</span>"#, chat.media)
            } else {
                String::new()
            },
            deleted = if chat.deleted > 0 {
                format!(
                    r#"<span class="chip danger">{} deleted</span>"#,
                    chat.deleted
                )
            } else {
                String::new()
            },
        );
    }
    if chats.is_empty() {
        out.push_str(r#"<p class="empty">No chats matched the export rules.</p>"#);
    }
    out.push_str("</div></section>\n");

    if let Some(account) = account {
        out.push_str(&profile_html(account));
        out.push_str(&sessions_html(account));
        out.push_str(&contacts_html(account));
        out.push_str(&privacy_html(account));
    }
    out.push_str(&about_export_html(manifest));
    out.push_str(&page_end(base, &meta));
    out
}

fn profile_html(account: &AccountInfo) -> String {
    let name = format!("{} {}", account.first_name, account.last_name)
        .trim()
        .to_string();
    let latest = account.avatars.iter().find_map(|a| a.file.as_deref());
    let mut out = format!(
        r#"<section data-panel="profile" class="hidden stack"><div class="card hero">{}<div><h1>{}</h1><p class="muted">{}</p><div class="chips">{}</div></div></div><div class="card"><h2>Details</h2><dl class="kv">"#,
        avatar(latest, &name, "", "lg"),
        esc(&name),
        esc(&account
            .username
            .as_ref()
            .map(|u| format!("@{u}"))
            .unwrap_or_default()),
        if account.premium {
            format!(r#"<span class="chip ok">{}Premium</span>"#, icon("star"))
        } else {
            String::new()
        }
    );
    for (label, value) in [
        ("User ID", Some(account.id.to_string())),
        ("Phone", account.phone.clone()),
        ("Bio", account.about.clone()),
        ("Birthday", account.birthday.clone()),
        (
            "Delete the account if away for",
            account.account_ttl_days.map(|d| format!("{d} days")),
        ),
    ] {
        if let Some(value) = value.filter(|v| !v.is_empty()) {
            let _ = write!(out, "<dt>{}</dt><dd>{}</dd>", esc(label), esc(&value));
        }
    }
    out.push_str("</dl></div>");
    if !account.avatars.is_empty() {
        let _ = write!(
            out,
            r#"<div class="card"><h2>{}Profile photos ({})</h2><div class="gallery">"#,
            icon("photo"),
            account.avatars.len()
        );
        for avatar in &account.avatars {
            match &avatar.file {
                Some(file) => {
                    let _ = write!(
                        out,
                        r#"<a href="{0}"><img src="{0}" alt="" loading="lazy"><span class="kind">{1}</span></a>"#,
                        esc(file),
                        esc(&local_time(avatar.date, "%d.%m.%Y"))
                    );
                }
                None => {
                    let _ = write!(
                        out,
                        r#"<a><div class="placeholder" style="height:100%">{}</div><span class="kind">{}</span></a>"#,
                        icon("photo"),
                        esc(&local_time(avatar.date, "%d.%m.%Y"))
                    );
                }
            }
        }
        out.push_str("</div></div>");
    }
    out.push_str("</section>\n");
    out
}

fn sessions_html(account: &AccountInfo) -> String {
    let mut out = String::from(
        r#"<section data-panel="sessions" class="hidden"><div class="card table-wrap"><table><thead><tr><th>Device</th><th>App</th><th>Location</th><th>Created</th><th>Last active</th></tr></thead><tbody>"#,
    );
    for session in &account.sessions {
        let _ = write!(
            out,
            r#"<tr class="{}"><td><b>{}</b><br><span class="faint">{} {}</span></td><td>{}{}</td><td>{}<br><span class="faint">{}</span></td><td>{}</td><td>{}</td></tr>"#,
            if session.current { "current" } else { "" },
            esc(&session.device),
            esc(&session.platform),
            esc(&session.system),
            esc(&session.app),
            if session.official_app {
                ""
            } else {
                " (unofficial)"
            },
            esc(&session.location),
            esc(&session.ip),
            esc(&local_time(session.created, "%d.%m.%Y %H:%M")),
            if session.current {
                "this session".to_string()
            } else {
                esc(&local_time(session.active, "%d.%m.%Y %H:%M"))
            },
        );
    }
    if account.sessions.is_empty() {
        out.push_str(r#"<tr><td colspan="5" class="faint">Sessions were not included.</td></tr>"#);
    }
    out.push_str("</tbody></table></div></section>\n");
    out
}

fn contacts_html(account: &AccountInfo) -> String {
    let mut out = format!(
        r#"<section data-panel="contacts" class="hidden"><div class="toolbar"><input class="input" id="contact-q" type="search" placeholder="Search {} contacts" autocomplete="off"></div><div class="card table-wrap"><table><thead><tr><th>Name</th><th>Username</th><th>Phone</th><th>Mutual</th></tr></thead><tbody>"#,
        account.contacts.len()
    );
    for contact in &account.contacts {
        let _ = write!(
            out,
            r#"<tr data-contact data-search="{}"><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>"#,
            esc(&format!(
                "{} {} {}",
                contact.name,
                contact.username.as_deref().unwrap_or(""),
                contact.phone.as_deref().unwrap_or("")
            )
            .to_lowercase()),
            esc(&contact.name),
            esc(&contact
                .username
                .as_ref()
                .map(|u| format!("@{u}"))
                .unwrap_or_default()),
            esc(contact.phone.as_deref().unwrap_or("")),
            if contact.mutual {
                icon("check")
            } else {
                String::new()
            }
        );
    }
    out.push_str("</tbody></table></div></section>\n");
    out
}

fn privacy_html(account: &AccountInfo) -> String {
    let mut out = String::from(
        r#"<section data-panel="privacy" class="hidden"><div class="card"><dl class="kv">"#,
    );
    for entry in &account.privacy {
        let _ = write!(
            out,
            "<dt>{}</dt><dd>{}</dd>",
            esc(&entry.key),
            esc(&entry.rules.join("; "))
        );
    }
    if account.privacy.is_empty() {
        out.push_str(r#"<dt>Settings</dt><dd>Not included in this export.</dd>"#);
    }
    out.push_str("</dl></div></section>\n");
    out
}

fn about_export_html(manifest: &ExportManifest) -> String {
    let rules = &manifest.rules;
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for (enabled, label) in [
        (rules.media.photos, "photos"),
        (rules.media.videos, "videos"),
        (rules.media.video_notes, "video messages"),
        (rules.media.voice, "voice messages"),
        (rules.media.audio, "audio"),
        (rules.media.gifs, "GIFs"),
        (rules.media.stickers, "stickers"),
        (rules.media.files, "files"),
    ] {
        if enabled {
            kept.push(label)
        } else {
            dropped.push(label)
        }
    }
    let limit = rules
        .media
        .max_file_bytes()
        .map(human_bytes)
        .unwrap_or_else(|| "no limit".into());
    let total = rules
        .media
        .max_total_bytes()
        .map(human_bytes)
        .unwrap_or_else(|| "no limit".into());
    let mut out = String::from(
        r#"<section data-panel="about" class="hidden"><div class="card"><dl class="kv">"#,
    );
    for (label, value) in [
        ("Method", manifest.method.title().to_string()),
        ("Account", manifest.account_name.clone()),
        (
            "Requested",
            local_time(manifest.requested_at, "%d.%m.%Y %H:%M:%S"),
        ),
        (
            "Completed",
            manifest
                .finished_at
                .map(|t| local_time(t, "%d.%m.%Y %H:%M:%S"))
                .unwrap_or_else(|| "in progress".into()),
        ),
        ("Chats", manifest.chats.to_string()),
        ("Messages", manifest.messages.to_string()),
        (
            "Deleted messages marked",
            manifest.deleted_marked.to_string(),
        ),
        (
            "Files saved",
            format!("{} ({})", manifest.media_saved, human_bytes(manifest.bytes)),
        ),
        ("Files left out", manifest.media_skipped.to_string()),
        (
            "Media kept",
            if kept.is_empty() {
                "none".into()
            } else {
                kept.join(", ")
            },
        ),
        (
            "Media replaced by placeholders",
            if dropped.is_empty() {
                "none".into()
            } else {
                dropped.join(", ")
            },
        ),
        ("File size limit", limit),
        ("Total size limit", total),
        (
            "Skipped words",
            if rules.messages.skip_words.is_empty() {
                "none".into()
            } else {
                rules.messages.skip_words.join(", ")
            },
        ),
    ] {
        let _ = write!(out, "<dt>{}</dt><dd>{}</dd>", esc(label), esc(&value));
    }
    out.push_str("</dl></div></section>\n");
    out
}

/// I render a plain chat page in the style of Telegram Desktop's HTML export.
pub fn desktop_chat_page(chat: &ChatExport, back: &str) -> String {
    let mut out = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>{0}</title><style>{1}</style></head><body><div class="page_header"><a href="{2}">&larr; Exported Data</a><h1>{0}</h1></div><div class="history">"#,
        esc(&chat.title),
        DESKTOP_CSS,
        esc(back)
    );
    let mut last_day = String::new();
    for message in &chat.messages {
        let day = local_time(message.date, "%d %B %Y");
        if day != last_day {
            let _ = write!(out, r#"<div class="service date">{}</div>"#, esc(&day));
            last_day = day;
        }
        if let Some(action) = &message.service {
            let _ = write!(
                out,
                r#"<div class="service" id="message{}">{}</div>"#,
                message.id,
                esc(action)
            );
            continue;
        }
        let _ = write!(
            out,
            r#"<div class="message" id="message{}"><div class="from"><b>{}</b> <span class="date" title="{}">{}</span></div>"#,
            message.id,
            esc(message.from_name.as_deref().unwrap_or(&chat.title)),
            esc(&local_time(message.date, "%d.%m.%Y %H:%M:%S")),
            esc(&local_time(message.date, "%H:%M"))
        );
        if let Some(forwarded) = &message.forwarded_from {
            let _ = write!(
                out,
                r#"<div class="forwarded">Forwarded from {}</div>"#,
                esc(forwarded)
            );
        }
        if let Some(reply) = message.reply_to {
            let _ = write!(
                out,
                r##"<div class="reply">In reply to <a href="#message{reply}">this message</a></div>"##
            );
        }
        if let Some(media) = &message.media {
            match &media.file {
                Some(file) if media.kind == Some(MediaKind::Photo) => {
                    let _ = write!(
                        out,
                        r#"<a href="{0}"><img class="photo" src="{0}" alt=""></a>"#,
                        esc(file)
                    );
                }
                Some(file) => {
                    let _ = write!(
                        out,
                        r#"<div class="media"><a href="{}">{}</a> {}</div>"#,
                        esc(file),
                        esc(media
                            .name
                            .as_deref()
                            .unwrap_or(&capitalize(&media.type_name))),
                        esc(&media_summary(media))
                    );
                }
                None if media.kind.is_some() => {
                    let _ = write!(
                        out,
                        r#"<div class="media missing">{}<br><small>Not included, change data exporting settings to download.</small></div>"#,
                        esc(&capitalize(&media.type_name))
                    );
                }
                None => {
                    let _ = write!(
                        out,
                        r#"<div class="media">{} {}</div>"#,
                        esc(&capitalize(&media.type_name)),
                        esc(media.details.as_deref().unwrap_or(""))
                    );
                }
            }
        }
        if !message.text.is_empty() {
            let _ = write!(
                out,
                r#"<div class="text">{}</div>"#,
                to_html(&message.text, &message.entities)
            );
        }
        if message.edited.is_some() {
            out.push_str(r#"<div class="details">edited</div>"#);
        }
        out.push_str("</div>");
    }
    out.push_str("</div></body></html>\n");
    out
}

pub fn desktop_index(manifest: &ExportManifest, chats: &[ChatCard]) -> String {
    let mut out = format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Exported Data</title><style>{}</style></head><body><div class="page_header"><h1>Exported Data</h1><p>{} · {}</p></div><div class="history">"#,
        DESKTOP_CSS,
        esc(&manifest.account_name),
        esc(&local_time(manifest.requested_at, "%d.%m.%Y %H:%M"))
    );
    for chat in chats {
        let _ = write!(
            out,
            r#"<div class="message"><a href="{}"><b>{}</b></a> <span class="date">{} messages</span></div>"#,
            esc(&chat.page),
            esc(&chat.title),
            chat.messages
        );
    }
    out.push_str("</div></body></html>\n");
    out
}

const DESKTOP_CSS: &str = "body{margin:0;font:13px/1.4 -apple-system,'Segoe UI',Roboto,sans-serif;background:#fff;color:#000}a{color:#168acd;text-decoration:none}.page_header{position:sticky;top:0;background:#fff;border-bottom:1px solid #e3e6e8;padding:10px 20px}.page_header h1{font-size:16px;margin:4px 0}.history{max-width:760px;margin:0 auto;padding:10px 20px 40px}.message{padding:8px 0;border-bottom:1px solid #f0f2f4}.from b{color:#3892db}.date{color:#999;font-size:12px}.text{white-space:pre-wrap;margin-top:3px}.service{text-align:center;color:#999;padding:8px 0}.service.date{font-weight:bold}.photo{max-width:300px;max-height:300px;border-radius:6px;margin-top:4px}.media{margin-top:4px;color:#555}.media.missing{color:#999;border:1px dashed #ccc;border-radius:6px;padding:6px 10px;display:inline-block}.forwarded,.reply,.details{color:#999;font-size:12px}pre,code{background:#f5f5f5;border-radius:4px}";

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_chat() -> ChatExport {
        let mut chat = ChatExport {
            id: 42,
            title: "Alex <script>".into(),
            kind: "private".into(),
            read_inbox_max_id: 1,
            read_outbox_max_id: 2,
            ..ChatExport::default()
        };
        chat.messages = vec![
            MessageExport {
                id: 1,
                date: 1_700_000_000,
                text: "hello".into(),
                from_name: Some("Alex".into()),
                ..MessageExport::default()
            },
            MessageExport {
                id: 2,
                date: 1_700_000_060,
                out: true,
                text: "hi".into(),
                reply_to: Some(1),
                read: Some(true),
                ..MessageExport::default()
            },
            MessageExport {
                id: 3,
                date: 1_700_000_120,
                text: "gone".into(),
                deleted: Some(DeletedMark {
                    deleted_at: 1_700_000_500,
                    restored_from_archive: true,
                }),
                media: Some(MediaExport {
                    type_name: "sticker".into(),
                    kind: Some(MediaKind::Sticker),
                    skipped: Some("Sticker excluded by export rules".into()),
                    ..MediaExport::default()
                }),
                ..MessageExport::default()
            },
        ];
        chat.stats = ChatStats::build(&chat);
        chat
    }

    #[test]
    fn chat_page_marks_deleted_and_escapes() {
        let chat = sample_chat();
        let page = chat_page(
            &chat,
            &PageMeta {
                base: "../../",
                back: Some("../../index.html"),
                account_name: "me",
                method: ExportMethod::Custom,
                requested_at: 1_700_000_000,
            },
        );
        assert!(!page.contains("<script>"));
        assert!(page.contains("Alex &lt;script&gt;"));
        assert!(page.contains("deleted-badge"));
        assert!(page.contains("Sticker excluded by export rules"));
        assert!(page.contains(r#"data-jump="m1""#));
        assert!(page.contains("ticks read"));
        assert_eq!(chat.stats.deleted, 1);
        assert_eq!(chat.stats.my_reply_secs, Some(60));
    }

    #[test]
    fn desktop_page_uses_the_official_placeholder() {
        let page = desktop_chat_page(&sample_chat(), "../../export_results.html");
        assert!(page.contains("Not included, change data exporting settings to download."));
    }
}
