use std::sync::Arc;

use anyhow::Result;
use grammers_client::tl;
use serde_json::{json, Value};

use super::fetch::{Fetcher, PeerIndex};
use super::model::ExportMethod;
use super::{start, ExportRequest};
use crate::account_tools::{now_secs, AccountHandle};
use crate::app::Services;

const RECENT_FOR_EDITS: i32 = 40;

fn state_key(rule_id: &str, account_id: i64) -> String {
    format!("export.watch.{rule_id}.{account_id}")
}

/// I run a scheduled check: when a watched chat changed since the last look, I export it.
/// I return how many exports I started.
pub async fn run_check(
    services: &Arc<Services>,
    handle: &AccountHandle,
    rule_id: &str,
    chats: &[i64],
    every_hours: u32,
    rules_patch: &Value,
) -> Result<usize> {
    let key = state_key(rule_id, handle.user_id);
    let mut state = services.db.get(&key).await;
    if !state.is_object() {
        state = json!({});
    }
    let now = now_secs();
    let last = state["last_check"].as_i64().unwrap_or(0);
    if now - last < i64::from(every_hours.max(1)) * 3600 {
        return Ok(0);
    }
    state["last_check"] = now.into();

    let job = services
        .jobs
        .start("watch", "Export watch check", &handle.name)
        .await;
    let fetcher = Fetcher::new(
        handle.client.clone(),
        None,
        job.clone(),
        std::env::temp_dir(),
        handle.user_id,
    );
    let mut index = PeerIndex::default();
    let dialogs = match fetcher.dialogs(&mut index).await {
        Ok(dialogs) => dialogs,
        Err(error) => {
            job.finish(crate::jobs::JobStatus::Failed, error.to_string())
                .await;
            services.db.set(&key, state).await?;
            return Err(error);
        }
    };
    let mut started = 0;
    let mut changed_titles = Vec::new();
    for chat_id in chats {
        let Some(dialog) = dialogs.iter().find(|d| d.peer.id == *chat_id) else {
            job.log(format!("Chat {chat_id} is not in the dialog list"))
                .await;
            continue;
        };
        let latest_edit = latest_edit(&fetcher, &dialog.peer.input).await;
        let deleted = crate::anti_delete::deleted_for_chat(&services.db, handle.user_id, *chat_id)
            .await
            .map(|list| list.len())
            .unwrap_or(0);
        let fingerprint = format!("{}:{latest_edit}:{deleted}", dialog.top_message);
        let field = chat_id.to_string();
        if state["chats"][&field].as_str() == Some(fingerprint.as_str()) {
            continue;
        }
        state["chats"][&field] = fingerprint.into();
        changed_titles.push(dialog.peer.title.clone());
        let request = ExportRequest {
            method: ExportMethod::Custom,
            accounts: vec![handle.user_id],
            chats: vec![*chat_id],
            rules: Some(rules_patch.clone()),
        };
        match start(services, request).await {
            Ok(jobs) => started += jobs.len(),
            Err(error) => job.log(format!("{}: {error:#}", dialog.peer.title)).await,
        }
    }
    services.db.set(&key, state).await?;
    let summary = if changed_titles.is_empty() {
        "No changes since the last check".to_string()
    } else {
        format!("Changed: {}", changed_titles.join(", "))
    };
    job.finish(crate::jobs::JobStatus::Done, summary).await;
    Ok(started)
}

async fn latest_edit(fetcher: &Fetcher, peer: &tl::enums::InputPeer) -> i32 {
    let Ok(response) = fetcher
        .call(tl::functions::messages::GetHistory {
            peer: peer.clone(),
            offset_id: 0,
            offset_date: 0,
            add_offset: 0,
            limit: RECENT_FOR_EDITS,
            max_id: 0,
            min_id: 0,
            hash: 0,
        })
        .await
    else {
        return 0;
    };
    let messages = match response {
        tl::enums::messages::Messages::Messages(m) => m.messages,
        tl::enums::messages::Messages::Slice(m) => m.messages,
        tl::enums::messages::Messages::ChannelMessages(m) => m.messages,
        tl::enums::messages::Messages::NotModified(_) => Vec::new(),
    };
    messages
        .iter()
        .filter_map(|message| match message {
            tl::enums::Message::Message(m) => m.edit_date,
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

pub async fn forget_rule(services: &Services, rule_id: &str) {
    for key in services
        .db
        .keys_with_prefix(&format!("export.watch.{rule_id}."))
        .await
    {
        let _ = services.db.remove(&key).await;
    }
}
