use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InlineQueryResult, InlineQueryResultArticle,
    InputMessageContent, InputMessageContentText,
};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{env_key, DATABASE_FILE};
use crate::database::Database;

#[derive(Clone)]
pub struct InlineResult {
    pub title: String,
    pub text: String,
    pub buttons: Vec<Vec<InlineButton>>,
}

#[derive(Clone)]
pub struct InlineButton {
    pub label: String,
    pub callback_id: String,
}

pub struct InlineRegistry {
    pub results: RwLock<HashMap<String, InlineResult>>,
    pub callbacks: RwLock<HashMap<String, Arc<dyn Fn() + Send + Sync>>>,
}

impl InlineRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            results: RwLock::new(HashMap::new()),
            callbacks: RwLock::new(HashMap::new()),
        })
    }

    #[allow(dead_code)]
    pub async fn register(&self, result: InlineResult) -> String {
        let id = Uuid::new_v4().to_string();
        self.results.write().await.insert(id.clone(), result);
        id
    }
}

pub async fn run(db: Arc<Database>) -> Result<()> {
    let token = resolve_token(&db).await;

    let Some(token) = token else {
        warn!(
            "no bot token found, inline bot disabled. \
             Set TELOXIDE_TOKEN env var or add bot_token to {DATABASE_FILE}"
        );
        return Ok(());
    };

    let bot = Bot::new(token);
    info!("inline bot starting");

    let registry = InlineRegistry::new();

    let handler = dptree::entry()
        .branch(Update::filter_inline_query().endpoint({
            let registry = Arc::clone(&registry);
            move |bot: Bot, q: InlineQuery| {
                let registry = Arc::clone(&registry);
                async move { handle_inline_query(bot, q, registry).await }
            }
        }))
        .branch(Update::filter_callback_query().endpoint({
            let registry = Arc::clone(&registry);
            move |bot: Bot, q: CallbackQuery| {
                let registry = Arc::clone(&registry);
                async move { handle_callback_query(bot, q, registry).await }
            }
        }));

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn resolve_token(db: &Database) -> Option<String> {
    let db_value = db.get("bot_token").await;
    if let Some(s) = db_value.as_str() {
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }

    std::env::var(env_key::TELOXIDE_TOKEN)
        .ok()
        .filter(|s| !s.is_empty())
}

async fn handle_inline_query(
    bot: Bot,
    query: InlineQuery,
    registry: Arc<InlineRegistry>,
) -> ResponseResult<()> {
    let id = query.query.trim().to_string();
    let results_guard = registry.results.read().await;

    let answers: Vec<InlineQueryResult> = if let Some(result) = results_guard.get(&id) {
        let keyboard = build_keyboard(&result.buttons);
        let article = InlineQueryResultArticle::new(
            &id,
            result.title.clone(),
            InputMessageContent::Text(InputMessageContentText::new(result.text.clone())),
        )
        .reply_markup(keyboard);

        vec![InlineQueryResult::Article(article)]
    } else {
        vec![]
    };

    bot.answer_inline_query(query.id, answers).send().await?;
    Ok(())
}

async fn handle_callback_query(
    bot: Bot,
    query: CallbackQuery,
    registry: Arc<InlineRegistry>,
) -> ResponseResult<()> {
    // Telegram keeps the client spinner active until every callback is acknowledged.
    bot.answer_callback_query(query.id.clone()).await?;

    if let Some(data) = &query.data {
        let callbacks = registry.callbacks.read().await;
        if let Some(handler) = callbacks.get(data) {
            handler();
        }
    }

    Ok(())
}

fn build_keyboard(buttons: &[Vec<InlineButton>]) -> InlineKeyboardMarkup {
    let rows: Vec<Vec<InlineKeyboardButton>> = buttons
        .iter()
        .map(|row| {
            row.iter()
                .map(|b| InlineKeyboardButton::callback(b.label.clone(), b.callback_id.clone()))
                .collect()
        })
        .collect();

    InlineKeyboardMarkup::new(rows)
}
