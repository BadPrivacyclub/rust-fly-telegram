//! Core message translations. English is the default; modules can read the active
//! language with `ctx:lang()` and pick their own strings.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Ru,
}

impl Lang {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "ru" | "rus" | "russian" | "русский" => Lang::Ru,
            _ => Lang::En,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ru => "ru",
        }
    }
}

/// Translation table: key, English, Russian.
const TABLE: &[(&str, &str, &str)] = &[
    ("help.title", "Commands", "Команды"),
    ("help.modules", "modules", "модулей"),
    ("help.commands", "commands", "команд"),
    ("help.prefix", "prefix", "префикс"),
    (
        "help.footer",
        "Details: {p}help <module or command>",
        "Подробнее: {p}help <модуль или команда>",
    ),
    (
        "help.not_found",
        "Nothing found for",
        "Ничего не найдено по запросу",
    ),
    ("help.module", "Module", "Модуль"),
    ("help.version", "Version", "Версия"),
    ("help.disabled", "disabled", "выключен"),
    ("help.trusted", "trusted", "доверенный"),
    ("help.sandboxed", "sandboxed", "песочница"),
    ("help.permissions", "Permissions", "Разрешения"),
    ("help.settings", "Settings", "Настройки"),
    ("help.no_description", "No description", "Нет описания"),
    ("cat.core", "Core", "Ядро"),
    ("cat.messaging", "Messaging", "Сообщения"),
    ("cat.modules", "Modules", "Модули"),
    ("cat.info", "Info & OSINT", "Инфо и OSINT"),
    ("cat.files", "Files & media", "Файлы и медиа"),
    ("cat.ai", "AI", "ИИ"),
    ("cat.automation", "Automation", "Автоматизация"),
    ("cat.groups", "Groups", "Группы"),
    ("cat.security", "Security", "Безопасность"),
    ("cat.music", "Music", "Музыка"),
    ("cat.fun", "Fun", "Развлечения"),
    ("cat.utils", "Utilities", "Утилиты"),
    ("cat.other", "Other", "Другое"),
    ("error.title", "Error", "Ошибка"),
    (
        "error.sudo_denied",
        "This command is not available to sudo users.",
        "Эта команда недоступна sudo-пользователям.",
    ),
    ("restart.done", "Restarted in", "Перезапущено за"),
    ("restart.running", "Restarting…", "Перезапуск…"),
    (
        "bot.started",
        "fly-telegram started",
        "fly-telegram запущен",
    ),
    ("bot.menu", "Control panel", "Панель управления"),
    ("bot.status", "Status", "Статус"),
    ("bot.modules", "Modules", "Модули"),
    ("bot.settings", "Settings", "Настройки"),
    ("bot.logs", "Recent errors", "Последние ошибки"),
    ("bot.restart", "Restart", "Перезапуск"),
    (
        "bot.restart_confirm",
        "Restart the userbot now?",
        "Перезапустить юзербота сейчас?",
    ),
    ("bot.yes", "Yes, restart", "Да, перезапустить"),
    ("bot.back", "Back", "Назад"),
    ("bot.close", "Close", "Закрыть"),
    ("bot.enabled", "enabled", "включён"),
    ("bot.disabled", "disabled", "выключен"),
    ("bot.uptime", "Uptime", "Аптайм"),
    ("bot.accounts", "Accounts", "Аккаунты"),
    ("bot.commands_run", "Commands", "Команд"),
    ("bot.updates", "Updates", "Апдейтов"),
    ("bot.memory", "Memory", "Память"),
    ("bot.errors", "Errors", "Ошибок"),
    (
        "bot.no_errors",
        "No warnings or errors.",
        "Ни предупреждений, ни ошибок.",
    ),
    (
        "bot.not_owner",
        "This bot belongs to another fly-telegram account.",
        "Этот бот принадлежит другому аккаунту fly-telegram.",
    ),
    (
        "bot.pm_new",
        "New private message",
        "Новое личное сообщение",
    ),
    ("bot.pm_allow", "Allow", "Разрешить"),
    ("bot.pm_deny", "Block", "Заблокировать"),
    ("bot.pm_allowed", "Allowed", "Разрешено"),
    ("bot.pm_denied", "Blocked", "Заблокировано"),
    ("bot.language", "Language", "Язык"),
    (
        "bot.panel_link",
        "Web panel login link (valid 15 min, works on this computer only)",
        "Ссылка для входа в веб-панель (действует 15 минут, только на этом компьютере)",
    ),
    ("bot.panel", "Web panel", "Веб-панель"),
    ("bot.module_error", "Module error", "Ошибка модуля"),
    ("bot.deleted", "Deleted message", "Удалённое сообщение"),
    ("set.afk", "AFK auto-reply", "Автоответ AFK"),
    ("set.autoread", "Auto-read", "Автопрочтение"),
    ("set.antidelete", "Anti-delete", "Анти-удаление"),
    ("set.pmguard", "PM guard", "Защита ЛС"),
    (
        "set.cleanjoins",
        "Clean join messages",
        "Чистка сообщений о входе",
    ),
    ("set.captcha", "Group CAPTCHA", "Капча в группах"),
    ("set.notify_errors", "Notify: errors", "Уведомления: ошибки"),
    (
        "set.notify_pmguard",
        "Notify: new PMs",
        "Уведомления: новые ЛС",
    ),
    (
        "set.notify_startup",
        "Notify: startup",
        "Уведомления: запуск",
    ),
    (
        "set.notify_antidelete",
        "Notify: deleted messages",
        "Уведомления: удалённые сообщения",
    ),
];

pub fn tr(lang: Lang, key: &str) -> &'static str {
    TABLE
        .iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, en, ru)| match lang {
            Lang::En => *en,
            Lang::Ru => *ru,
        })
        .unwrap_or("?")
}

/// Known category slugs map to translated titles; anything else is shown as written.
pub fn category_title(lang: Lang, category: &str) -> String {
    let slug = category.trim().to_ascii_lowercase();
    let key = format!("cat.{slug}");
    if TABLE.iter().any(|(k, _, _)| *k == key) {
        tr(lang, &key).to_string()
    } else if category.trim().is_empty() {
        tr(lang, "cat.other").to_string()
    } else {
        category.trim().to_string()
    }
}

/// Fixed display order for known categories; custom categories sort after them.
pub fn category_rank(category: &str) -> usize {
    const ORDER: &[&str] = &[
        "core",
        "modules",
        "messaging",
        "automation",
        "security",
        "groups",
        "info",
        "utils",
        "files",
        "ai",
        "music",
        "fun",
    ];
    let slug = category.trim().to_ascii_lowercase();
    ORDER
        .iter()
        .position(|value| *value == slug)
        .unwrap_or(ORDER.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_unique_and_translated() {
        for (index, (key, en, ru)) in TABLE.iter().enumerate() {
            assert!(!en.is_empty() && !ru.is_empty(), "{key}");
            assert!(
                TABLE[index + 1..].iter().all(|(other, _, _)| other != key),
                "duplicate {key}"
            );
        }
        assert_eq!(tr(Lang::Ru, "help.title"), "Команды");
        assert_eq!(category_title(Lang::En, "Custom"), "Custom");
    }
}
