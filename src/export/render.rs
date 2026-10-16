use super::model::{Archive, Format};
use anyhow::{Context, Result};
use std::path::Path;

fn esc(value: &str) -> String {
    crate::notify::escape_html(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn html(archive: &Archive) -> String {
    let custom = archive.options.method == super::model::Method::Custom;
    let mut out = format!(
        r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="referrer" content="no-referrer"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src 'self' data:; media-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'"><title>{}</title><style>{}</style><body><header><h1>{}</h1><p>Requested: {} UTC · Completed: {} UTC</p><input id="search" type="search" placeholder="Search messages" aria-label="Search messages"><button onclick="window.print()">Print / PDF</button></header><nav>"#,
        esc(&archive.account_name),
        include_str!("archive.css"),
        esc(&archive.account_name),
        date(archive.requested_at),
        date(archive.finished_at)
    );
    for chat in &archive.chats {
        out.push_str(&format!(
            "<a href=\"#chat-{}\">{}</a>",
            chat.id,
            esc(&chat.title)
        ));
    }
    out.push_str("</nav><main><div class=\"reader-controls\"><label><input id=\"deleted-only\" type=\"checkbox\"> Deleted messages only</label> <span id=\"matches\"></span> <a id=\"resume\" hidden>Resume reading</a></div>");
    if !archive.account.is_null() {
        if let Some(avatars) = archive.account["avatars"].as_array() {
            out.push_str("<div class=\"gallery\">");
            for path in avatars.iter().filter_map(|p| p.as_str()) {
                out.push_str(&format!("<a href=\"{}\"><img class=\"avatar\" loading=\"lazy\" src=\"{}\" alt=\"Account avatar\"></a>", esc(path), esc(path)));
            }
            out.push_str("</div>");
        }
        out.push_str(&format!("<details><summary>Account profile, settings and sessions</summary><pre>{}</pre></details>", esc(&serde_json::to_string_pretty(&archive.account).unwrap_or_default())));
    }
    for chat in &archive.chats {
        out.push_str(&format!(
            "<section id=\"chat-{}\"><h2>{}</h2><p class=\"details\">{} · {} · {} messages{}</p>",
            chat.id,
            esc(&chat.title),
            chat.id,
            esc(&chat.kind),
            chat.messages.len(),
            if chat.truncated {
                " · message limit reached"
            } else {
                ""
            }
        ));
        if let Some(path) = &chat.avatar {
            out.push_str(&format!(
                "<img class=\"avatar\" src=\"{}\" alt=\"Chat avatar\">",
                esc(path)
            ));
        }
        if let Some(username) = &chat.username {
            out.push_str(&format!("<p class=\"details\">@{}</p>", esc(username)));
        }
        for message in &chat.messages {
            out.push_str(&format!("<article id=\"m-{}-{}\" class=\"{} {}\"><div class=\"meta\"><b>{}</b> <time>{}</time> <a href=\"#m-{}-{}\">#{}</a></div>", chat.id, message.id, if message.outgoing && custom { "outgoing" } else { "" }, if message.deleted_at.is_some() { "deleted" } else { "" }, esc(&message.sender), esc(&message.date), chat.id, message.id, message.id));
            if let Some(id) = message.reply_to {
                out.push_str(&format!(
                    "<a href=\"#m-{}-{}\">Reply to #{}</a>",
                    chat.id, id, id
                ));
            }
            if let Some(at) = &message.deleted_at {
                out.push_str(&format!("<p class=\"badge\">Deleted · {}</p>", esc(at)));
            }
            if let Some(at) = message.edited_at {
                out.push_str(&format!(
                    "<p class=\"details\">Edited: {} UTC</p>",
                    date(i64::from(at))
                ));
            }
            out.push_str(&format!(
                "<div class=\"text\">{}</div>",
                message_text(message)
            ));
            if let Some(media) = &message.media {
                if let Some(path) = &media.path {
                    let path = esc(path);
                    let kind = match media.mime_type.as_deref() {
                        Some(mime) if mime.starts_with("image/") => "photo",
                        Some(mime) if mime.starts_with("video/") => "video",
                        _ => media.kind.as_str(),
                    };
                    match kind {
                        "photo" => out.push_str(&format!("<a href=\"{path}\"><img loading=\"lazy\" src=\"{path}\" alt=\"Photo\"></a>")),
                        "video" | "video_note" | "gif" => out.push_str(&format!("<video controls preload=\"none\" src=\"{path}\"></video>")),
                        "voice" | "audio" => out.push_str(&format!("<audio controls preload=\"none\" src=\"{path}\"></audio>")),
                        _ => out.push_str(&format!("<a download href=\"{path}\">{}</a>", esc(media.name.as_deref().unwrap_or(&media.kind)))),
                    }
                } else {
                    out.push_str(&format!(
                        "<div class=\"placeholder\">{} · {}</div>",
                        esc(&media.kind),
                        esc(media.omitted.as_deref().unwrap_or("unavailable"))
                    ));
                }
            }
            out.push_str("</article>");
        }
        out.push_str("</section>");
    }
    for warning in &archive.warnings {
        out.push_str(&format!("<p class=\"placeholder\">{}</p>", esc(warning)));
    }
    out.push_str("</main><script>");
    out.push_str(include_str!("archive.js"));
    out.push_str("</script></body></html>");
    out = out.replace(
        "<body>",
        &format!("<body data-archive=\"{}\">", esc(&archive.id)),
    );
    out
}

fn message_text(message: &super::model::Message) -> String {
    let utf16: Vec<_> = message.text.encode_utf16().collect();
    let mut entities: Vec<_> = message
        .entities
        .iter()
        .filter(|e| e.kind == "custom_emoji")
        .collect();
    entities.sort_by_key(|e| e.offset);
    let mut cursor = 0;
    let mut html = String::new();
    for entity in entities {
        let (Ok(start), Ok(length), Some(id)) = (
            usize::try_from(entity.offset),
            usize::try_from(entity.length),
            entity.document_id,
        ) else {
            continue;
        };
        let Some(end) = start.checked_add(length).filter(|end| *end <= utf16.len()) else {
            continue;
        };
        if start < cursor {
            continue;
        }
        let (Ok(before), Ok(emoji)) = (
            String::from_utf16(&utf16[cursor..start]),
            String::from_utf16(&utf16[start..end]),
        ) else {
            continue;
        };
        html.push_str(&esc(&before));
        html.push_str(&format!("<span class=\"custom-emoji\" data-document-id=\"{id}\" title=\"Telegram custom emoji {id}\">{}</span>", esc(&emoji)));
        cursor = end;
    }
    html.push_str(&esc(&String::from_utf16_lossy(&utf16[cursor..])));
    html
}

fn date(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub async fn write(archive: &Archive, root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    if matches!(archive.options.format, Format::Json | Format::Both) {
        crate::storage::atomic_write(
            &root.join("result.json"),
            &serde_json::to_vec_pretty(archive)?,
        )
        .await?;
        files.push("result.json".into());
    }
    if matches!(
        archive.options.format,
        Format::Html | Format::Both | Format::Pdf
    ) {
        crate::storage::atomic_write(&root.join("index.html"), html(archive).as_bytes()).await?;
        files.push("index.html".into());
    }
    if archive.options.format == Format::Pdf {
        let chromium = std::env::var("FLY_PDF_BROWSER").unwrap_or_else(|_| "chromium".into());
        let root = root.canonicalize()?;
        let profile = root.join(".pdf-profile");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            tokio::process::Command::new(chromium)
                .args([
                    "--headless",
                    "--disable-gpu",
                    "--no-pdf-header-footer",
                    "--disable-background-networking",
                ])
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg(format!(
                    "--print-to-pdf={}",
                    root.join("chat.pdf").display()
                ))
                .arg(root.join("index.html"))
                .kill_on_drop(true)
                .output(),
        )
        .await
        .context("PDF rendering timed out")?
        .context("PDF requires Chromium; set FLY_PDF_BROWSER to its executable")?;
        let _ = tokio::fs::remove_dir_all(profile).await;
        anyhow::ensure!(
            result.status.success() && root.join("chat.pdf").exists(),
            "PDF renderer failed"
        );
        files.push("chat.pdf".into());
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_active_content() {
        assert_eq!(esc("<script>\"'&"), "&lt;script&gt;&quot;&#39;&amp;");
    }

    #[test]
    fn custom_emoji_uses_utf16_offsets_and_escapes_surrounding_text() {
        let message = super::super::model::Message {
            text: "🚀⭐<script>".into(),
            entities: vec![super::super::model::Entity {
                kind: "custom_emoji".into(),
                offset: 2,
                length: 1,
                document_id: Some(123),
            }],
            ..Default::default()
        };
        let rendered = message_text(&message);
        assert!(rendered.starts_with("🚀<span"));
        assert!(rendered.contains("data-document-id=\"123\""));
        assert!(rendered.ends_with("⭐</span>&lt;script&gt;"));
    }
}
