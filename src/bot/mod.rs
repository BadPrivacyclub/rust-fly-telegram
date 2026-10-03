//! Control bot: an owner-only panel inside Telegram, notifications, and inline help.
//!
//! The bot answers only the Telegram accounts logged into this userbot. Everyone else gets
//! a short refusal, so a leaked bot username exposes nothing.

use std::sync::Arc;

use teloxide::prelude::*;
use teloxide::types::{
    BotCommand, InlineKeyboardButton, InlineKeyboardMarkup, InlineQueryResult,
    InlineQueryResultArticle, InputMessageContent, InputMessageContentText,
    MaybeInaccessibleMessage, ParseMode,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::core_settings::{self, TOGGLES};
use crate::i18n::{self, tr, Lang};
use crate::loader::{Loader, ModuleSummary};
use crate::notify::{clip, escape_html};

const MODULES_PER_PAGE: usize = 12;

pub struct BotManager {
    loader: Arc<Loader>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl BotManager {
    pub fn new(loader: Arc<Loader>) -> Arc<Self> {
        Arc::new(Self {
            loader,
            task: Mutex::new(None),
        })
    }

    /// (Re)starts the bot with the current token. Without a token the bot stays off.
    pub async fn restart(self: &Arc<Self>) {
        let mut task = self.task.lock().await;
        if let Some(handle) = task.take() {
            handle.abort();
        }
        let notifier = Arc::clone(&self.loader.services.notifier);
        notifier.set_bot(None, None).await;

        let Some(token) = core_settings::bot_token(&self.loader.services.db).await else {
            info!("control bot disabled: no bot token configured");
            return;
        };
        let bot = Bot::new(token);
        let me = match bot.get_me().await {
            Ok(me) => me,
            Err(error) => {
                warn!("control bot token rejected by Telegram: {error}");
                return;
            }
        };
        let username = me.username.clone();
        info!(
            "control bot @{} started",
            username.as_deref().unwrap_or("unknown")
        );
        notifier.set_bot(Some(bot.clone()), username).await;
        let _ = bot
            .set_my_commands(vec![
                BotCommand::new("start", "Control panel"),
                BotCommand::new("status", "Runtime status"),
                BotCommand::new("modules", "Modules"),
                BotCommand::new("settings", "Settings"),
                BotCommand::new("logs", "Recent errors"),
                BotCommand::new("panel", "Web panel login link"),
            ])
            .await;

        let loader = Arc::clone(&self.loader);
        *task = Some(tokio::spawn(async move {
            run_dispatcher(bot, loader).await;
        }));
    }

    pub async fn stop(&self) {
        if let Some(handle) = self.task.lock().await.take() {
            handle.abort();
        }
        self.loader.services.notifier.set_bot(None, None).await;
    }
}

/// Checks a bot token without starting the bot; returns the bot username.
pub async fn validate_token(token: &str) -> anyhow::Result<String> {
    let me = Bot::new(token.trim()).get_me().await?;
    Ok(me.username.clone().unwrap_or_default())
}

async fn run_dispatcher(bot: Bot, loader: Arc<Loader>) {
    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint({
            let loader = Arc::clone(&loader);
            move |bot: Bot, msg: teloxide::types::Message| {
                let loader = Arc::clone(&loader);
                async move { handle_message(bot, msg, loader).await }
            }
        }))
        .branch(Update::filter_inline_query().endpoint({
            let loader = Arc::clone(&loader);
            move |bot: Bot, q: InlineQuery| {
                let loader = Arc::clone(&loader);
                async move { handle_inline_query(bot, q, loader).await }
            }
        }))
        .branch(Update::filter_callback_query().endpoint({
            let loader = Arc::clone(&loader);
            move |bot: Bot, q: CallbackQuery| {
                let loader = Arc::clone(&loader);
                async move { handle_callback(bot, q, loader).await }
            }
        }));

    Dispatcher::builder(bot, handler)
        .default_handler(|_| async {})
        .build()
        .dispatch()
        .await;
}

async fn is_owner(loader: &Loader, user_id: u64) -> bool {
    loader
        .services
        .runtime
        .owner_ids()
        .await
        .contains(&(user_id as i64))
}

async fn lang(loader: &Loader) -> Lang {
    core_settings::language(&loader.services.db).await
}

fn button(label: impl Into<String>, data: impl Into<String>) -> InlineKeyboardButton {
    InlineKeyboardButton::callback(label.into(), data.into())
}

struct Screen {
    text: String,
    keyboard: InlineKeyboardMarkup,
}

fn home_screen(lang: Lang) -> Screen {
    Screen {
        text: format!(
            "✈️ <b>fly-telegram</b> <code>v{}</code>\n{}",
            crate::VERSION,
            tr(lang, "bot.menu")
        ),
        keyboard: InlineKeyboardMarkup::new(vec![
            vec![
                button(format!("📊 {}", tr(lang, "bot.status")), "m:status"),
                button(format!("🧩 {}", tr(lang, "bot.modules")), "m:mods:0"),
            ],
            vec![
                button(format!("⚙️ {}", tr(lang, "bot.settings")), "m:set"),
                button(format!("🧾 {}", tr(lang, "bot.logs")), "m:logs"),
            ],
            vec![
                button(format!("🖥 {}", tr(lang, "bot.panel")), "m:panel"),
                button(
                    format!(
                        "🌐 {}: {}",
                        tr(lang, "bot.language"),
                        lang.code().to_uppercase()
                    ),
                    "m:lang",
                ),
            ],
            vec![button(
                format!("🔄 {}", tr(lang, "bot.restart")),
                "m:restart",
            )],
        ]),
    }
}

fn back_row(lang: Lang, target: &str) -> Vec<InlineKeyboardButton> {
    vec![button(format!("‹ {}", tr(lang, "bot.back")), target)]
}

async fn status_screen(loader: &Loader, lang: Lang) -> Screen {
    let runtime = &loader.services.runtime;
    let accounts = runtime.accounts().await;
    let mut text = format!(
        "📊 <b>{}</b>\n\n⏱ {}: <code>{}</code>\n⌨️ {}: <code>{}</code>\n📨 {}: <code>{}</code>\n❗ {}: <code>{}</code>",
        tr(lang, "bot.status"),
        tr(lang, "bot.uptime"),
        format_duration(runtime.uptime_seconds()),
        tr(lang, "bot.commands_run"),
        runtime.commands_seen(),
        tr(lang, "bot.updates"),
        runtime.updates_seen(),
        tr(lang, "bot.errors"),
        runtime.errors_seen(),
    );
    if let Some(memory) = crate::runtime::process_memory_bytes() {
        text.push_str(&format!(
            "\n💾 {}: <code>{:.1} MB</code>",
            tr(lang, "bot.memory"),
            memory as f64 / 1_048_576.0
        ));
    }
    if let Some(cpu) = runtime.process_cpu_percent().await {
        text.push_str(&format!("\n🧮 CPU: <code>{cpu:.1}%</code>"));
    }
    text.push_str(&format!("\n\n👤 <b>{}</b>", tr(lang, "bot.accounts")));
    for account in accounts {
        text.push_str(&format!(
            "\n{} {} · <code>{}</code>",
            if account.connected { "🟢" } else { "🔴" },
            escape_html(&account.name),
            account.commands_seen
        ));
    }
    let top = runtime.top_commands(5);
    if !top.is_empty() {
        let prefix = first_prefix(loader).await;
        text.push_str("\n\n🏆 ");
        text.push_str(
            &top.iter()
                .map(|c| {
                    format!(
                        "<code>{prefix}{}</code>×{}",
                        escape_html(&c.command),
                        c.count
                    )
                })
                .collect::<Vec<_>>()
                .join("  "),
        );
    }
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![vec![
            button("↻", "m:status"),
            button(format!("‹ {}", tr(lang, "bot.back")), "m:home"),
        ]]),
    }
}

async fn modules_screen(loader: &Loader, lang: Lang, page: usize) -> Screen {
    let modules = loader.summaries().await;
    let pages = modules.len().div_ceil(MODULES_PER_PAGE).max(1);
    let page = page.min(pages - 1);
    let mut rows = Vec::new();
    let slice = modules
        .iter()
        .skip(page * MODULES_PER_PAGE)
        .take(MODULES_PER_PAGE)
        .collect::<Vec<_>>();
    for chunk in slice.chunks(2) {
        rows.push(
            chunk
                .iter()
                .map(|module| {
                    button(
                        format!(
                            "{} {}",
                            if module.enabled { "🟢" } else { "⚪" },
                            module.info.name
                        ),
                        callback_data(&format!("m:mod:{}", module.info.name)),
                    )
                })
                .collect(),
        );
    }
    let mut nav = Vec::new();
    if page > 0 {
        nav.push(button("‹", format!("m:mods:{}", page - 1)));
    }
    nav.push(button(format!("{}/{}", page + 1, pages), "m:noop"));
    if page + 1 < pages {
        nav.push(button("›", format!("m:mods:{}", page + 1)));
    }
    rows.push(nav);
    rows.push(back_row(lang, "m:home"));
    let enabled = modules.iter().filter(|m| m.enabled).count();
    Screen {
        text: format!(
            "🧩 <b>{}</b>\n<code>{enabled}/{}</code> {}",
            tr(lang, "bot.modules"),
            modules.len(),
            tr(lang, "bot.enabled")
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

async fn module_screen(loader: &Loader, lang: Lang, name: &str) -> Option<Screen> {
    let module = loader
        .summaries()
        .await
        .into_iter()
        .find(|module| module.info.name == name)?;
    let prefix = first_prefix(loader).await;
    let text = module_card_html(&module, lang, &prefix);
    let toggle = if module.enabled {
        format!("⚪ {}", tr(lang, "bot.disabled"))
    } else {
        format!("🟢 {}", tr(lang, "bot.enabled"))
    };
    Some(Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![
            vec![
                button(toggle, callback_data(&format!("m:mt:{name}"))),
                button("↻ reload", callback_data(&format!("m:mr:{name}"))),
            ],
            back_row(lang, "m:mods:0"),
        ]),
    })
}

fn module_card_html(module: &ModuleSummary, lang: Lang, prefix: &str) -> String {
    let description = if !module.help.description.is_empty() {
        module.help.description.clone()
    } else if !module.info.description.is_empty() {
        module.info.description.clone()
    } else {
        tr(lang, "help.no_description").to_string()
    };
    let mut text = format!(
        "🧩 <b>{}</b> <code>v{}</code> · {} · {}\n<i>{}</i>\n",
        escape_html(&module.info.name),
        escape_html(&module.info.version),
        if module.enabled {
            format!("🟢 {}", tr(lang, "bot.enabled"))
        } else {
            format!("⚪ {}", tr(lang, "bot.disabled"))
        },
        if module.info.trusted {
            tr(lang, "help.trusted")
        } else {
            tr(lang, "help.sandboxed")
        },
        escape_html(&description)
    );
    for command in &module.help.commands {
        let usage = if command.args.is_empty() {
            format!("{prefix}{}", command.name)
        } else {
            format!("{prefix}{} {}", command.name, command.args)
        };
        text.push_str(&format!("\n▸ <code>{}</code>", escape_html(&usage)));
        if !command.description.is_empty() {
            text.push_str(&format!(" — {}", escape_html(&command.description)));
        }
    }
    if !module.config.is_empty() {
        text.push_str(&format!("\n\n⚙️ <b>{}</b>", tr(lang, "help.settings")));
        for entry in &module.config {
            let shown = if entry.field.secret {
                if entry.is_set {
                    "••••••".to_string()
                } else {
                    "—".to_string()
                }
            } else {
                crate::loader::meta::display_value(&entry.field, &entry.value)
            };
            text.push_str(&format!(
                "\n<code>{}</code> = <code>{}</code>",
                escape_html(&entry.field.key),
                escape_html(&clip(&shown, 60))
            ));
        }
    }
    text
}

async fn settings_screen(loader: &Loader, lang: Lang) -> Screen {
    let db = &loader.services.db;
    let mut rows = Vec::new();
    for (key, label, default) in TOGGLES {
        let enabled = core_settings::flag(db, key, *default).await;
        rows.push(vec![button(
            format!("{} {}", if enabled { "🟢" } else { "⚪" }, tr(lang, label)),
            format!("m:st:{key}"),
        )]);
    }
    rows.push(back_row(lang, "m:home"));
    let prefixes = core_settings::prefixes(db).await;
    Screen {
        text: format!(
            "⚙️ <b>{}</b>\n{}: <code>{}</code>",
            tr(lang, "bot.settings"),
            tr(lang, "help.prefix"),
            escape_html(&prefixes.join(" "))
        ),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

fn logs_screen(loader: &Loader, lang: Lang) -> Screen {
    let records = loader.services.logs.recent_errors(10);
    let text = if records.is_empty() {
        format!(
            "🧾 <b>{}</b>\n\n{}",
            tr(lang, "bot.logs"),
            tr(lang, "bot.no_errors")
        )
    } else {
        let mut text = format!("🧾 <b>{}</b>\n", tr(lang, "bot.logs"));
        for record in records {
            let icon = if record.level == "ERROR" {
                "🔴"
            } else {
                "🟡"
            };
            text.push_str(&format!(
                "\n{icon} <code>{}</code> {}",
                format_clock(record.ts_ms),
                escape_html(&clip(&record.message, 220))
            ));
        }
        text
    };
    Screen {
        text,
        keyboard: InlineKeyboardMarkup::new(vec![vec![
            button("↻", "m:logs"),
            button(format!("‹ {}", tr(lang, "bot.back")), "m:home"),
        ]]),
    }
}

pub fn pm_keyboard(lang: Lang, user_id: i64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![
        button(
            format!("✅ {}", tr(lang, "bot.pm_allow")),
            format!("pm:a:{user_id}"),
        ),
        button(
            format!("⛔ {}", tr(lang, "bot.pm_deny")),
            format!("pm:d:{user_id}"),
        ),
    ]])
}

async fn help_overview_html(loader: &Loader, lang: Lang) -> Screen {
    let modules = loader.summaries().await;
    let prefix = first_prefix(loader).await;
    let commands: usize = modules
        .iter()
        .filter(|m| m.enabled)
        .map(|m| m.info.commands.len())
        .sum();
    let mut text = format!(
        "✈️ <b>fly-telegram</b> <code>v{}</code>\n<code>{}</code> {} · <code>{commands}</code> {} · {} <code>{}</code>\n",
        crate::VERSION,
        modules.len(),
        tr(lang, "help.modules"),
        tr(lang, "help.commands"),
        tr(lang, "help.prefix"),
        escape_html(&prefix)
    );
    let mut category: Option<String> = None;
    for module in &modules {
        let title = i18n::category_title(lang, &module.category);
        if category.as_deref() != Some(title.as_str()) {
            text.push_str(&format!("\n<b>▸ {}</b>\n", escape_html(&title)));
            category = Some(title);
        }
        let name = if module.enabled {
            format!("<b>{}</b>", escape_html(&module.info.name))
        } else {
            format!("<s>{}</s>", escape_html(&module.info.name))
        };
        let commands = module
            .info
            .commands
            .iter()
            .map(|c| format!("<code>{}{}</code>", escape_html(&prefix), escape_html(c)))
            .collect::<Vec<_>>()
            .join(" ");
        text.push_str(&format!("{name} {commands}\n"));
    }
    let rows = modules
        .chunks(3)
        .map(|chunk| {
            chunk
                .iter()
                .map(|m| {
                    button(
                        m.info.name.clone(),
                        callback_data(&format!("h:{}", m.info.name)),
                    )
                })
                .collect()
        })
        .collect::<Vec<Vec<_>>>();
    Screen {
        text: clip(&text, 4000),
        keyboard: InlineKeyboardMarkup::new(rows),
    }
}

async fn help_module_html(loader: &Loader, lang: Lang, name: &str) -> Option<Screen> {
    let module = loader.summaries().await.into_iter().find(|module| {
        module.info.name == name || module.info.commands.iter().any(|c| c == name)
    })?;
    let prefix = first_prefix(loader).await;
    Some(Screen {
        text: module_card_html(&module, lang, &prefix),
        keyboard: InlineKeyboardMarkup::new(vec![vec![button(
            format!("‹ {}", tr(lang, "bot.back")),
            "h:",
        )]]),
    })
}

async fn handle_inline_query(
    bot: Bot,
    query: InlineQuery,
    loader: Arc<Loader>,
) -> ResponseResult<()> {
    if !is_owner(&loader, query.from.id.0).await {
        bot.answer_inline_query(query.id, Vec::<InlineQueryResult>::new())
            .cache_time(0)
            .is_personal(true)
            .await?;
        return Ok(());
    }
    let lang = lang(&loader).await;
    let text = query.query.trim();
    let topic = text
        .strip_prefix("help")
        .map(str::trim)
        .unwrap_or(text)
        .to_lowercase();
    let screen = if topic.is_empty() {
        Some(help_overview_html(&loader, lang).await)
    } else {
        help_module_html(&loader, lang, &topic).await
    };
    let results = match screen {
        Some(screen) => vec![InlineQueryResult::Article(
            InlineQueryResultArticle::new(
                "help",
                format!("fly-telegram · {}", tr(lang, "help.title")),
                InputMessageContent::Text(
                    InputMessageContentText::new(screen.text).parse_mode(ParseMode::Html),
                ),
            )
            .description(if topic.is_empty() {
                "help".to_string()
            } else {
                topic
            })
            .reply_markup(screen.keyboard),
        )],
        None => Vec::new(),
    };
    bot.answer_inline_query(query.id, results)
        .cache_time(0)
        .is_personal(true)
        .await?;
    Ok(())
}

async fn handle_message(
    bot: Bot,
    msg: teloxide::types::Message,
    loader: Arc<Loader>,
) -> ResponseResult<()> {
    if !msg.chat.is_private() {
        return Ok(());
    }
    let Some(from) = msg.from.as_ref() else {
        return Ok(());
    };
    let lang = lang(&loader).await;
    if !is_owner(&loader, from.id.0).await {
        bot.send_message(msg.chat.id, tr(lang, "bot.not_owner"))
            .await?;
        return Ok(());
    }

    let command = msg
        .text()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .split('@')
        .next()
        .unwrap_or_default()
        .to_string();
    let screen = match command.as_str() {
        "/status" => status_screen(&loader, lang).await,
        "/modules" => modules_screen(&loader, lang, 0).await,
        "/settings" => settings_screen(&loader, lang).await,
        "/logs" => logs_screen(&loader, lang),
        "/panel" => {
            send_panel_link(&bot, msg.chat.id, &loader, lang).await?;
            return Ok(());
        }
        _ => home_screen(lang),
    };
    bot.send_message(msg.chat.id, screen.text)
        .parse_mode(ParseMode::Html)
        .reply_markup(screen.keyboard)
        .await?;
    Ok(())
}

async fn send_panel_link(
    bot: &Bot,
    chat: ChatId,
    loader: &Loader,
    lang: Lang,
) -> ResponseResult<()> {
    let link = loader.services.panel_link().await;
    bot.send_message(
        chat,
        format!(
            "🖥 <b>{}</b>\n{}\n\n<code>{}</code>",
            tr(lang, "bot.panel"),
            tr(lang, "bot.panel_link"),
            escape_html(&link)
        ),
    )
    .parse_mode(ParseMode::Html)
    .await?;
    Ok(())
}

async fn handle_callback(
    bot: Bot,
    query: CallbackQuery,
    loader: Arc<Loader>,
) -> ResponseResult<()> {
    let lang = lang(&loader).await;
    if !is_owner(&loader, query.from.id.0).await {
        bot.answer_callback_query(query.id)
            .text(tr(lang, "bot.not_owner"))
            .show_alert(true)
            .await?;
        return Ok(());
    }
    let data = query.data.clone().unwrap_or_default();
    let mut toast: Option<String> = None;

    // Inline help messages live in other chats and are edited through their inline id.
    if let Some(topic) = data.strip_prefix("h:") {
        let screen = if topic.is_empty() {
            Some(help_overview_html(&loader, lang).await)
        } else {
            help_module_html(&loader, lang, topic).await
        };
        bot.answer_callback_query(query.id.clone()).await?;
        if let Some(screen) = screen {
            edit_screen(&bot, &query, screen).await;
        }
        return Ok(());
    }

    if let Some(rest) = data.strip_prefix("pm:") {
        let (action, id) = rest.split_once(':').unwrap_or(("", ""));
        let allow = action == "a";
        let db = &loader.services.db;
        let (add_to, remove_from) = if allow {
            ("pmguard.allow", "pmguard.deny")
        } else {
            ("pmguard.deny", "pmguard.allow")
        };
        let result = async {
            core_settings::csv_update(db, add_to, id, true).await?;
            core_settings::csv_update(db, remove_from, id, false).await?;
            db.remove(&format!("pmguard.challenge_seen.{id}")).await
        }
        .await;
        let label = if allow {
            format!("✅ {}", tr(lang, "bot.pm_allowed"))
        } else {
            format!("⛔ {}", tr(lang, "bot.pm_denied"))
        };
        let pending = match id.parse::<i64>() {
            Ok(user_id) => loader
                .services
                .notifier
                .pm_pending
                .lock()
                .await
                .remove(&user_id),
            Err(_) => None,
        };
        if let (true, Some(pending), Ok(())) = (allow, pending, result.as_ref().map(|_| ())) {
            let text = db
                .get("pmguard.approve_text")
                .await
                .as_str()
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "✅ Approved. I'll reply soon.".to_string());
            if let Some(peer) = pending.peer.to_ref() {
                if let Err(error) = crate::telegram::send_markdown(
                    &pending.client,
                    &loader.services.runtime,
                    peer,
                    &text,
                )
                .await
                {
                    warn!("could not notify approved user {}: {error}", pending.name);
                }
            }
        }
        bot.answer_callback_query(query.id.clone())
            .text(match result {
                Ok(()) => label.clone(),
                Err(error) => error.to_string(),
            })
            .await?;
        if let Some(MaybeInaccessibleMessage::Regular(message)) = &query.message {
            let text = format!(
                "{}\n\n<b>{}</b>",
                escape_html(message.text().unwrap_or_default()),
                label
            );
            let _ = bot
                .edit_message_text(message.chat.id, message.id, text)
                .parse_mode(ParseMode::Html)
                .await;
        }
        return Ok(());
    }

    let screen = match data.as_str() {
        "m:home" => Some(home_screen(lang)),
        "m:status" => Some(status_screen(&loader, lang).await),
        "m:set" => Some(settings_screen(&loader, lang).await),
        "m:logs" => Some(logs_screen(&loader, lang)),
        "m:noop" => None,
        "m:lang" => {
            let next = if lang == Lang::En { Lang::Ru } else { Lang::En };
            let _ = core_settings::set_language(&loader.services.db, next).await;
            Some(home_screen(next))
        }
        "m:panel" => {
            if let Some(message) = &query.message {
                send_panel_link(&bot, message.chat().id, &loader, lang).await?;
            }
            None
        }
        "m:restart" => Some(Screen {
            text: format!("🔄 <b>{}</b>", tr(lang, "bot.restart_confirm")),
            keyboard: InlineKeyboardMarkup::new(vec![vec![
                button(format!("✅ {}", tr(lang, "bot.yes")), "m:restart!"),
                button(format!("‹ {}", tr(lang, "bot.back")), "m:home"),
            ]]),
        }),
        "m:restart!" => {
            bot.answer_callback_query(query.id.clone())
                .text(tr(lang, "restart.running"))
                .await?;
            edit_screen(
                &bot,
                &query,
                Screen {
                    text: format!("🔄 <i>{}</i>", tr(lang, "restart.running")),
                    keyboard: InlineKeyboardMarkup::default(),
                },
            )
            .await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            crate::restart::restart_now();
        }
        other => {
            if let Some(page) = other.strip_prefix("m:mods:") {
                Some(modules_screen(&loader, lang, page.parse().unwrap_or(0)).await)
            } else if let Some(name) = other.strip_prefix("m:mod:") {
                module_screen(&loader, lang, name).await
            } else if let Some(name) = other.strip_prefix("m:mt:") {
                let enabled = loader
                    .summaries()
                    .await
                    .iter()
                    .find(|m| m.info.name == name)
                    .map(|m| m.enabled);
                if let Some(enabled) = enabled {
                    if let Err(error) = loader.set_enabled(name, !enabled).await {
                        toast = Some(error.to_string());
                    }
                }
                module_screen(&loader, lang, name).await
            } else if let Some(name) = other.strip_prefix("m:mr:") {
                toast = Some(match loader.reload(name).await {
                    Ok(()) => "✅".to_string(),
                    Err(error) => clip(&error.to_string(), 180),
                });
                module_screen(&loader, lang, name).await
            } else if let Some(key) = other.strip_prefix("m:st:") {
                if let Some(default) = core_settings::toggle_default(key) {
                    let db = &loader.services.db;
                    let current = core_settings::flag(db, key, default).await;
                    let _ = db.set(key, serde_json::Value::Bool(!current)).await;
                }
                Some(settings_screen(&loader, lang).await)
            } else {
                None
            }
        }
    };

    let mut answer = bot.answer_callback_query(query.id.clone());
    if let Some(toast) = toast {
        answer = answer.text(toast);
    }
    answer.await?;
    if let Some(screen) = screen {
        edit_screen(&bot, &query, screen).await;
    }
    Ok(())
}

async fn edit_screen(bot: &Bot, query: &CallbackQuery, screen: Screen) {
    let result = if let Some(inline_id) = &query.inline_message_id {
        bot.edit_message_text_inline(inline_id.clone(), screen.text)
            .parse_mode(ParseMode::Html)
            .reply_markup(screen.keyboard)
            .await
            .map(drop)
    } else if let Some(message) = &query.message {
        bot.edit_message_text(message.chat().id, message.id(), screen.text)
            .parse_mode(ParseMode::Html)
            .reply_markup(screen.keyboard)
            .await
            .map(drop)
    } else {
        Ok(())
    };
    if let Err(error) = result {
        // "message is not modified" is expected when refreshing an unchanged screen.
        if !error.to_string().contains("not modified") {
            warn!("bot could not edit message: {error}");
        }
    }
}

/// Telegram limits callback data to 64 bytes.
fn callback_data(data: &str) -> String {
    let mut out = String::new();
    for ch in data.chars() {
        if out.len() + ch.len_utf8() > 64 {
            break;
        }
        out.push(ch);
    }
    out
}

async fn first_prefix(loader: &Loader) -> String {
    core_settings::prefixes(&loader.services.db)
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| ".".to_string())
}

fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m {}s", seconds % 60)
    }
}

fn format_clock(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_data_is_capped() {
        assert_eq!(callback_data(&"x".repeat(100)).len(), 64);
        assert_eq!(callback_data("m:mod:core"), "m:mod:core");
    }

    #[test]
    fn formats_time() {
        assert_eq!(format_duration(59), "0m 59s");
        assert_eq!(format_duration(3_700), "1h 1m");
        assert_eq!(format_duration(90_000), "1d 1h 0m");
        assert_eq!(format_clock(3_661_000), "01:01:01");
    }

    #[test]
    fn home_screen_has_restart() {
        let screen = home_screen(Lang::En);
        assert!(screen.text.contains("fly-telegram"));
        let flat = format!("{:?}", screen.keyboard);
        assert!(flat.contains("m:restart"));
    }
}
