use super::model::TextEntity;

/// A run of text with the entities I found covering all of it, outermost first.
pub struct Segment<'a> {
    pub text: String,
    pub entities: Vec<&'a TextEntity>,
}

/// I split text at every entity boundary. Telegram counts offsets in UTF-16 code units.
pub fn segments<'a>(text: &str, entities: &'a [TextEntity]) -> Vec<Segment<'a>> {
    let units = text.encode_utf16().collect::<Vec<_>>();
    let total = units.len() as i32;
    let mut cuts = vec![0, total];
    for entity in entities {
        let start = entity.offset.clamp(0, total);
        let end = (entity.offset + entity.length).clamp(0, total);
        cuts.push(start);
        cuts.push(end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start >= end {
            continue;
        }
        let piece = String::from_utf16_lossy(&units[start as usize..end as usize]);
        let mut covering = entities
            .iter()
            .filter(|e| e.offset <= start && e.offset + e.length >= end)
            .collect::<Vec<_>>();
        covering.sort_by_key(|e| std::cmp::Reverse(e.length));
        out.push(Segment {
            text: piece,
            entities: covering,
        });
    }
    out
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn safe_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    ["http://", "https://", "tg://", "mailto:", "tel:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
        .then(|| escape_html(trimmed))
}

/// I format a message body as HTML and keep links only with safe schemes.
pub fn to_html(text: &str, entities: &[TextEntity]) -> String {
    let mut out = String::new();
    for segment in segments(text, entities) {
        let mut open = String::new();
        let mut close = Vec::new();
        for entity in &segment.entities {
            let (start, end) = match entity.kind.as_str() {
                "bold" => ("<b>".to_string(), "</b>"),
                "italic" => ("<i>".to_string(), "</i>"),
                "underline" => ("<u>".to_string(), "</u>"),
                "strike" => ("<s>".to_string(), "</s>"),
                "spoiler" => (
                    "<span class=\"spoiler\" tabindex=\"0\">".to_string(),
                    "</span>",
                ),
                "code" => ("<code>".to_string(), "</code>"),
                "pre" => ("<pre>".to_string(), "</pre>"),
                "blockquote" => ("<blockquote>".to_string(), "</blockquote>"),
                "url" => match safe_url(&segment.text) {
                    Some(url) => (format!("<a href=\"{url}\" rel=\"noopener\">"), "</a>"),
                    None => continue,
                },
                "text_link" => match entity.url.as_deref().and_then(safe_url) {
                    Some(url) => (format!("<a href=\"{url}\" rel=\"noopener\">"), "</a>"),
                    None => continue,
                },
                "email" => (
                    format!("<a href=\"mailto:{}\">", escape_html(&segment.text)),
                    "</a>",
                ),
                "mention_name" => (
                    format!(
                        "<a class=\"mention\" href=\"tg://user?id={}\">",
                        entity.user_id.unwrap_or_default()
                    ),
                    "</a>",
                ),
                "mention" | "hashtag" | "cashtag" | "bot_command" => {
                    ("<span class=\"tag\">".to_string(), "</span>")
                }
                "custom_emoji" => ("<span class=\"custom-emoji\">".to_string(), "</span>"),
                _ => continue,
            };
            open.push_str(&start);
            close.push(end);
        }
        out.push_str(&open);
        out.push_str(&escape_html(&segment.text));
        for end in close.iter().rev() {
            out.push_str(end);
        }
    }
    out
}

/// I take the innermost entity type of each run, in the shape Telegram Desktop writes `text_entities`.
pub fn desktop_entities(text: &str, entities: &[TextEntity]) -> Vec<serde_json::Value> {
    segments(text, entities)
        .into_iter()
        .map(|segment| {
            let entity = segment.entities.last();
            let kind = entity.map_or("plain", |e| desktop_kind(&e.kind));
            let mut value = serde_json::json!({ "type": kind, "text": segment.text });
            if let Some(entity) = entity {
                if let Some(url) = &entity.url {
                    value["href"] = url.clone().into();
                }
                if let Some(user_id) = entity.user_id {
                    value["user_id"] = user_id.into();
                }
                if let Some(language) = &entity.language {
                    value["language"] = language.clone().into();
                }
                if let Some(document_id) = entity.document_id {
                    value["document_id"] = document_id.to_string().into();
                }
            }
            value
        })
        .collect()
}

fn desktop_kind(kind: &str) -> &'static str {
    match kind {
        "bold" => "bold",
        "italic" => "italic",
        "underline" => "underline",
        "strike" => "strikethrough",
        "spoiler" => "spoiler",
        "code" => "code",
        "pre" => "pre",
        "blockquote" => "blockquote",
        "url" => "link",
        "text_link" => "text_link",
        "email" => "email",
        "phone" => "phone",
        "mention" => "mention",
        "mention_name" => "mention_name",
        "hashtag" => "hashtag",
        "cashtag" => "cashtag",
        "bot_command" => "bot_command",
        "bank_card" => "bank_card",
        "custom_emoji" => "custom_emoji",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(kind: &str, offset: i32, length: i32) -> TextEntity {
        TextEntity {
            kind: kind.into(),
            offset,
            length,
            url: None,
            user_id: None,
            language: None,
            document_id: None,
        }
    }

    #[test]
    fn offsets_are_utf16() {
        let text = "😀 bold";
        let entities = [entity("bold", 3, 4)];
        assert_eq!(to_html(text, &entities), "😀 <b>bold</b>");
    }

    #[test]
    fn nested_entities_close_in_order() {
        let entities = [entity("bold", 0, 5), entity("italic", 2, 3)];
        assert_eq!(to_html("hello", &entities), "<b>he</b><b><i>llo</i></b>");
    }

    #[test]
    fn unsafe_links_are_dropped() {
        let mut link = entity("text_link", 0, 4);
        link.url = Some("javascript:alert(1)".into());
        assert_eq!(to_html("<x>!", &[link]), "&lt;x&gt;!");
    }

    #[test]
    fn desktop_shape() {
        let entities = [entity("bold", 0, 2)];
        let out = desktop_entities("hi there", &entities);
        assert_eq!(out[0]["type"], "bold");
        assert_eq!(out[1]["type"], "plain");
        assert_eq!(out[1]["text"], " there");
    }
}
