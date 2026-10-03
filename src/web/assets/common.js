// Shared helpers for every panel page: API calls, theme, i18n, toasts.
"use strict";

const Fly = (() => {
  const store = {
    get(key, fallback) {
      try { return localStorage.getItem(key) ?? fallback; } catch { return fallback; }
    },
    set(key, value) {
      try { localStorage.setItem(key, value); } catch { /* storage may be unavailable */ }
    },
  };


  async function api(path, options = {}) {
    const init = {
      method: options.method || (options.body !== undefined ? "POST" : "GET"),
      headers: { "X-Fly-Request": "1", ...(options.headers || {}) },
      credentials: "same-origin",
    };
    if (options.body instanceof FormData) {
      init.body = options.body;
    } else if (options.body !== undefined) {
      init.headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(options.body);
    }
    const res = await fetch(path, init);
    if (res.status === 401 && !path.startsWith("/auth")) {
      location.href = "/auth";
      throw new Error("unauthorized");
    }
    const type = res.headers.get("content-type") || "";
    const data = type.includes("application/json") ? await res.json() : await res.text();
    if (!res.ok || (data && data.ok === false && !data.need_password)) {
      const message = (data && data.error) || (typeof data === "string" && data) || res.statusText;
      const error = new Error(message);
      error.data = data;
      throw error;
    }
    return data;
  }


  // [id, label, preview colors: background, surface, accent]
  const THEMES = [
    ["auto", "Auto", ["#f4f6fa", "#161d27", "#229ed9"]],
    ["light", "Light", ["#f4f6fa", "#ffffff", "#229ed9"]],
    ["dark", "Dark", ["#0f141b", "#161d27", "#3cb1ec"]],
    ["amoled", "AMOLED", ["#000000", "#0a0a0a", "#3cb1ec"]],
    ["midnight", "Midnight", ["#0b1020", "#121a33", "#7c8cff"]],
    ["ocean", "Ocean", ["#061f28", "#0a2c37", "#21d4c4"]],
    ["forest", "Forest", ["#0e1a12", "#15261a", "#4fc97a"]],
    ["sunset", "Sunset", ["#fff6ef", "#ffffff", "#f2633a"]],
    ["rose", "Rose", ["#fff5f8", "#ffffff", "#e64980"]],
    ["nord", "Nord", ["#2e3440", "#3b4252", "#88c0d0"]],
    ["dracula", "Dracula", ["#1e1f29", "#282a36", "#bd93f9"]],
    ["contrast", "High contrast", ["#000000", "#000000", "#ffd400"]],
  ];
  const ACCENTS = ["", "#229ed9", "#6366f1", "#8b5cf6", "#ec4899", "#ef4444", "#f97316", "#eab308", "#22c55e", "#14b8a6", "#06b6d4"];

  function applyAppearance() {
    const root = document.documentElement;
    const theme = store.get("fly.theme", "auto");
    root.setAttribute("data-theme", THEMES.some(([id]) => id === theme) ? theme : "auto");
    const accent = store.get("fly.accent", "");
    if (/^#[0-9a-f]{6}$/i.test(accent)) {
      root.style.setProperty("--accent", accent);
      root.style.setProperty("--accent-strong", `color-mix(in srgb, ${accent} 80%, white)`);
    } else {
      root.style.removeProperty("--accent");
      root.style.removeProperty("--accent-strong");
    }
    const reduced = window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const effects = store.get("fly.effects", reduced ? "off" : "on");
    root.setAttribute("data-effects", effects === "on" ? "on" : "off");
  }

  function setTheme(id) { store.set("fly.theme", id); applyAppearance(); }
  function setAccent(hex) { store.set("fly.accent", hex || ""); applyAppearance(); }
  function setEffects(on) { store.set("fly.effects", on ? "on" : "off"); applyAppearance(); }

  // Kept for the login and setup pages: cycles through a few popular themes.
  function cycleTheme() {
    const order = ["auto", "light", "dark", "midnight", "ocean", "rose", "dracula"];
    const current = store.get("fly.theme", "auto");
    const next = order[(order.indexOf(current) + 1) % order.length];
    setTheme(next);
    return next;
  }

  // Appearance dialog content; `host` is any element (a modal body).
  function renderAppearance(host) {
    const theme = store.get("fly.theme", "auto");
    const accent = store.get("fly.accent", "");
    const effects = document.documentElement.getAttribute("data-effects") === "on";
    host.innerHTML = `
      <div class="stack">
        <h3>${esc(t("Theme"))}</h3>
        <div class="swatches">${THEMES.map(([id, label, colors]) => `
          <button class="swatch ${id === theme ? "active" : ""}" data-theme-id="${id}" type="button">
            <div class="preview"><span style="background:${colors[0]}"></span><span style="background:${colors[1]}"></span><span style="background:${colors[2]}"></span></div>
            ${esc(t(label))}
          </button>`).join("")}</div>
        <h3>${esc(t("Accent color"))}</h3>
        <div class="accent-dots">${ACCENTS.map((hex) => hex
          ? `<button class="accent-dot ${hex === accent ? "active" : ""}" data-accent="${hex}" style="background:${hex}" type="button" title="${hex}"></button>`
          : `<button class="btn btn-sm ${accent === "" ? "btn-primary" : ""}" data-accent="" type="button">${esc(t("Theme default"))}</button>`).join("")}
          <input type="color" id="accent-custom" value="${accent || "#229ed9"}" title="${esc(t("Custom"))}" style="width:36px;height:30px;border:0;background:none;padding:0;cursor:pointer">
        </div>
        <label class="row" style="justify-content:space-between">
          <span><b>${esc(t("Visual effects"))}</b><br><span class="small muted">${esc(t("Glass cards, animated background, motion"))}</span></span>
          <span class="switch"><input type="checkbox" id="effects-toggle" ${effects ? "checked" : ""}><span></span></span>
        </label>
      </div>`;
    host.querySelectorAll("[data-theme-id]").forEach((el) => el.addEventListener("click", () => { setTheme(el.dataset.themeId); renderAppearance(host); }));
    host.querySelectorAll("[data-accent]").forEach((el) => el.addEventListener("click", () => { setAccent(el.dataset.accent); renderAppearance(host); }));
    host.querySelector("#accent-custom").addEventListener("input", (e) => { setAccent(e.target.value); });
    host.querySelector("#effects-toggle").addEventListener("change", (e) => setEffects(e.target.checked));
  }

  applyAppearance();


  const dict = {
    en: {},
    ru: {
      "Sign in": "Войти",
      "Panel password": "Пароль панели",
      "Open the one-time login link printed in the console, or send": "Откройте одноразовую ссылку из консоли или отправьте",
      "in any Telegram chat to receive a fresh link.": "в любом чате Telegram, чтобы получить новую ссылку.",
      "The login link has expired or was already used.": "Ссылка для входа устарела или уже использована.",
      "Wrong password": "Неверный пароль",
      "Overview": "Обзор",
      "Modules": "Модули",
      "Logs": "Логи",
      "Anti-delete": "Анти-удаление",
      "Backups": "Бэкапы",
      "Settings": "Настройки",
      "Accounts": "Аккаунты",
      "Sign out": "Выйти",
      "Theme": "Тема",
      "Uptime": "Аптайм",
      "Commands": "Команды",
      "Updates": "Апдейты",
      "Memory": "Память",
      "CPU": "ЦП",
      "Connected": "Подключено",
      "Disconnected": "Нет связи",
      "Activity": "Активность",
      "Last 60 minutes": "За последние 60 минут",
      "Top commands": "Популярные команды",
      "No commands yet": "Команд пока не было",
      "Account": "Аккаунт",
      "Add account": "Добавить аккаунт",
      "Status": "Статус",
      "Enabled": "Включён",
      "Disabled": "Выключен",
      "Install": "Установить",
      "Install module": "Установить модуль",
      "Module URL": "URL модуля",
      "Reload": "Перезагрузить",
      "Delete": "Удалить",
      "Source": "Исходный код",
      "Configure": "Настроить",
      "Save": "Сохранить",
      "Saved": "Сохранено",
      "Cancel": "Отмена",
      "Close": "Закрыть",
      "Search": "Поиск",
      "Search modules": "Поиск модулей",
      "trusted": "доверенный",
      "sandboxed": "в песочнице",
      "Permissions": "Разрешения",
      "No settings for this module": "У модуля нет настроек",
      "Level": "Уровень",
      "Auto-refresh": "Автообновление",
      "Download backup": "Скачать бэкап",
      "Restore backup": "Восстановить из бэкапа",
      "Restart": "Перезапуск",
      "Restart now": "Перезапустить",
      "Command prefixes": "Префиксы команд",
      "Language": "Язык",
      "Bot token": "Токен бота",
      "Proxy": "Прокси",
      "Master password": "Мастер-пароль",
      "Notifications": "Уведомления",
      "Handlers": "Обработчики",
      "Security": "Безопасность",
      "General": "Основное",
      "Deleted messages": "Удалённые сообщения",
      "Chat": "Чат",
      "Sender": "Отправитель",
      "Text": "Текст",
      "Deleted": "Удалено",
      "Nothing here yet": "Здесь пока пусто",
      "Version": "Версия",
      "Next": "Далее",
      "Back": "Назад",
      "Finish": "Готово",
      "Skip": "Пропустить",
      "Send code": "Отправить код",
      "Confirm": "Подтвердить",
      "Accent color": "Акцентный цвет",
      "Theme default": "Как в теме",
      "Custom": "Свой",
      "Visual effects": "Визуальные эффекты",
      "Glass cards, animated background, motion": "Стеклянные карточки, анимированный фон, анимации",
      "Appearance": "Оформление",
      "Light": "Светлая", "Dark": "Тёмная", "Auto": "Авто", "High contrast": "Контрастная",
    },
  };

  let lang = store.get("fly.lang", (navigator.language || "en").toLowerCase().startsWith("ru") ? "ru" : "en");

  function t(text) {
    return (dict[lang] && dict[lang][text]) || text;
  }

  // Pages add their own strings: Fly.extend({ ru: { "Hello": "Привет" } }).
  function extend(more) {
    for (const [code, entries] of Object.entries(more)) {
      dict[code] = Object.assign(dict[code] || {}, entries);
    }
  }

  function setLang(next) {
    lang = next === "ru" ? "ru" : "en";
    store.set("fly.lang", lang);
    translate(document);
  }

  function translate(root) {
    root.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      el.textContent = t(key);
    });
    root.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
      el.setAttribute("placeholder", t(el.getAttribute("data-i18n-placeholder")));
    });
    document.documentElement.lang = lang;
  }


  function toast(message, kind = "info") {
    let host = document.querySelector(".toast-host");
    if (!host) {
      host = document.createElement("div");
      host.className = "toast-host";
      document.body.appendChild(host);
    }
    const el = document.createElement("div");
    el.className = `toast ${kind}`;
    el.textContent = message;
    host.appendChild(el);
    setTimeout(() => el.remove(), 4200);
  }

  function esc(value) {
    return String(value ?? "").replace(/[&<>"']/g, (ch) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
    })[ch]);
  }

  function duration(seconds) {
    seconds = Math.max(0, Math.floor(seconds || 0));
    const d = Math.floor(seconds / 86400);
    const h = Math.floor((seconds % 86400) / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    if (d) return `${d}d ${h}h ${m}m`;
    if (h) return `${h}h ${m}m`;
    if (m) return `${m}m ${s}s`;
    return `${s}s`;
  }

  function bytes(value) {
    if (value == null) return "—";
    const units = ["B", "KB", "MB", "GB"];
    let n = value;
    let i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return `${n.toFixed(n >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
  }

  document.addEventListener("DOMContentLoaded", () => translate(document));

  // Animates a number from its current value to `value`.
  function countTo(el, value, format = (n) => Math.round(n).toLocaleString()) {
    const from = Number(el.dataset.value || 0);
    el.dataset.value = value;
    if (document.documentElement.getAttribute("data-effects") !== "on" || from === value) {
      el.textContent = format(value);
      return;
    }
    const started = performance.now();
    const step = (now) => {
      const p = Math.min(1, (now - started) / 600);
      const eased = 1 - Math.pow(1 - p, 3);
      el.textContent = format(from + (value - from) * eased);
      if (p < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  }

  // Stable pleasant gradient for an ID, used for avatars.
  function avatarGradient(seed) {
    let h = 0;
    for (const ch of String(seed)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    const a = h % 360;
    return `linear-gradient(135deg, hsl(${a} 75% 58%), hsl(${(a + 50) % 360} 70% 48%))`;
  }

  return { api, t, extend, setLang, get lang() { return lang; }, translate, toast, esc, duration, bytes, cycleTheme, setTheme, renderAppearance, countTo, avatarGradient, store };
})();
