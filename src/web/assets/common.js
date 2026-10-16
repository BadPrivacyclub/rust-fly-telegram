// I hold the helpers every panel page shares: API calls, theme, i18n, toasts.
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

  // I list each theme as id, label, and preview colors (background, surface, accent).
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

  // I keep this for the login and setup pages: it cycles through a few popular themes.
  function cycleTheme() {
    const order = ["auto", "light", "dark", "midnight", "ocean", "rose", "dracula"];
    const current = store.get("fly.theme", "auto");
    const next = order[(order.indexOf(current) + 1) % order.length];
    setTheme(next);
    return next;
  }

  // I render the appearance dialog into `host`, any element such as a modal body.
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


  // I draw stroke icons on a 24px grid in the current text color.
  const ICONS = {
    plane: '<path d="M21 3 3 10.5l7 2.5 2.5 7z"/><path d="m10 13 4.5-4.5"/>',
    palette: '<path d="M12 3a9 9 0 1 0 0 18c1.2 0 1.6-.8 1.6-1.6 0-1.2-1-1.6-1-2.6 0-1 .8-1.8 1.8-1.8H17a4 4 0 0 0 4-4c0-4.4-4-8-9-8z"/><circle cx="7.5" cy="11" r="1"/><circle cx="10" cy="7" r="1"/><circle cx="14.5" cy="7" r="1"/>',
    save: '<path d="M5 3h11l3 3v15H5z"/><path d="M8 3v5h7M8 21v-7h8v7"/>',
    broom: '<path d="M14 4 20 10M17 7l-6.5 6.5"/><path d="M10.5 13.5 4 20h6l3.5-3.5"/>',
    bolt: '<path d="M13 2 4 14h7l-1 8 9-12h-7z"/>',
    users: '<circle cx="9" cy="8" r="3.5"/><path d="M2.5 20c.8-3.6 3.4-5.5 6.5-5.5s5.7 1.9 6.5 5.5"/><path d="M16 4.5a3.5 3.5 0 0 1 0 7M18 14.8c2 .7 3.2 2.4 3.5 5.2"/>',
    user: '<circle cx="12" cy="8" r="4"/><path d="M4 21c1-4 4-6 8-6s7 2 8 6"/>',
    check: '<path d="m5 12 4 4 10-10"/>',
    checkCircle: '<circle cx="12" cy="12" r="9"/><path d="m8 12 3 3 5-6"/>',
    xCircle: '<circle cx="12" cy="12" r="9"/><path d="m9 9 6 6M15 9l-6 6"/>',
    x: '<path d="M6 6l12 12M18 6 6 18"/>',
    bot: '<rect x="4" y="8" width="16" height="12" rx="3"/><path d="M12 8V4M9 13h.01M15 13h.01M9 17h6"/>',
    trash: '<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>',
    lock: '<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
    shield: '<path d="M12 3 4 6v6c0 5 3.5 8 8 9 4.5-1 8-4 8-9V6z"/>',
    key: '<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9M17 6l3 3M15 8l2 2"/>',
    gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.9 4.9 7 7M17 17l2.1 2.1M2 12h3M19 12h3M4.9 19.1 7 17M17 7l2.1-2.1"/>',
    pencil: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>',
    message: '<path d="M4 5h16v11H8l-4 4z"/>',
    megaphone: '<path d="M3 11v2l13 5V6zM16 9h2a3 3 0 0 1 0 6h-2M7 14l1 5h3l-1-4"/>',
    moon: '<path d="M20 15A8 8 0 1 1 9 4a6.5 6.5 0 0 0 11 11z"/>',
    puzzle: '<path d="M10 3h4v3a2 2 0 1 0 4 0V3h3v7h-3a2 2 0 1 0 0 4h3v7h-7v-3a2 2 0 1 0-4 0v3H3v-7h3a2 2 0 1 0 0-4H3V3z"/>',
    menu: '<path d="M4 6h16M4 12h16M4 18h16"/>',
    rocket: '<path d="M5 15c-1.5 1.5-2 4-2 6 2 0 4.5-.5 6-2"/><path d="M9 15 6 12l3-6c3-4 8-4 12-3 1 4 1 9-3 12l-6 3z"/><circle cx="15" cy="9" r="1.5"/>',
    timer: '<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2.5M9 2h6"/>',
    keyboard: '<rect x="2" y="6" width="20" height="12" rx="2"/><path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10"/>',
    inbox: '<path d="M3 13h5l2 3h4l2-3h5"/><path d="M5 5h14l2 8v6H3v-6z"/>',
    chip: '<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>',
    box: '<path d="m3 7 9-4 9 4v10l-9 4-9-4z"/><path d="m3 7 9 4 9-4M12 11v10"/>',
    bellOff: '<path d="M6 8a6 6 0 0 1 9.5-4.9M18 8c0 7 3 9 3 9H9M13.7 21a2 2 0 0 1-3.4 0M3 3l18 18M6 8c0 7-3 9-3 9h7"/>',
    crown: '<path d="m3 8 4.5 4L12 5l4.5 7L21 8l-2 11H5z"/>',
    hourglass: '<path d="M6 3h12M6 21h12M7 3c0 5 10 5 10 9s-10 4-10 9M17 3c0 5-10 5-10 9s10 4 10 9"/>',
    stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
    calendar: '<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M8 3v4M16 3v4"/>',
    flame: '<path d="M12 22a7 7 0 0 0 7-7c0-4-3-6-4-10-2 2-3 4-3 6-1-1-2-2-2-4-2 2-5 5-5 8a7 7 0 0 0 7 7z"/>',
    clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
    repeat: '<path d="M17 2l4 4-4 4"/><path d="M3 11V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4"/><path d="M21 13v2a3 3 0 0 1-3 3H3"/>',
    dot: '<circle cx="12" cy="12" r="5" fill="currentColor"/>',
    eye: '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
    chart: '<path d="M3 3v18h18"/><path d="M7 15l4-4 3 3 5-6"/>',
    clipboard: '<rect x="5" y="4" width="14" height="17" rx="2"/><path d="M9 4V3h6v1M9 11h6M9 15h4"/>',
    list: '<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>',
    arrowUp: '<path d="M12 19V5M5 12l7-7 7 7"/>',
    refresh: '<path d="M21 12a9 9 0 1 1-3-6.7L21 8"/><path d="M21 3v5h-5"/>',
    logout: '<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9"/>',
    party: '<path d="M4 20 9 7l8 8z"/><path d="M14 3v2M19 8h2M17.5 5.5 19 4M12 9c1-1 2-1 3 0"/>',
    forward: '<path d="M14 5l7 7-7 7"/><path d="M21 12H9a6 6 0 0 0-6 6"/>',
    download: '<path d="M12 3v12m0 0-4-4m4 4 4-4M4 17v3h16v-3"/>',
    upload: '<path d="M12 21V9m0 0-4 4m4-4 4 4M4 7V4h16v3"/>',
    archive: '<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v11h14V9M10 13h4"/>',
    file: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>',
    fileText: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M9 13h6M9 17h6"/>',
    external: '<path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
    plus: '<path d="M12 5v14M5 12h14"/>',
    info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 8h.01"/>',
    warn: '<path d="M12 3 2 20h20z"/><path d="M12 10v4M12 17h.01"/>',
    star: '<path d="m12 3 2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z"/>',
    search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-4-4"/>',
    photo: '<rect x="3" y="4" width="18" height="16" rx="3"/><circle cx="9" cy="10" r="2"/><path d="m21 16-5-5-9 9"/>',
    history: '<path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5M12 7v5l3 2"/>',
    sparkles: '<path d="M12 3l1.8 4.7L18.5 9.5l-4.7 1.8L12 16l-1.8-4.7L5.5 9.5l4.7-1.8z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>',
    pdf: '<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M8 16v-4h1.5a1.5 1.5 0 0 1 0 3H8M13 12v4h1a2 2 0 0 0 0-4z"/>',
  };

  function icon(name, extraClass = "") {
    const body = ICONS[name] || ICONS.dot;
    return `<svg class="ic ${extraClass}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;
  }

  // I fill `<i data-icon="name"></i>` placeholders in static markup.
  function hydrateIcons(root) {
    root.querySelectorAll("i[data-icon]").forEach((el) => {
      if (!el.firstChild) el.innerHTML = icon(el.dataset.icon);
    });
  }

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

  // Pages add their own strings through me: Fly.extend({ ru: { "Hello": "Привет" } }).
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
    hydrateIcons(root);
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
    if (value == null) return "-";
    const units = ["B", "KB", "MB", "GB"];
    let n = value;
    let i = 0;
    while (n >= 1024 && i < units.length - 1) { n /= 1024; i++; }
    return `${n.toFixed(n >= 10 || i === 0 ? 0 : 1)} ${units[i]}`;
  }

  document.addEventListener("DOMContentLoaded", () => translate(document));

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

  // I derive a stable, pleasant avatar gradient from an ID.
  function avatarGradient(seed) {
    let h = 0;
    for (const ch of String(seed)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
    const a = h % 360;
    return `linear-gradient(135deg, hsl(${a} 75% 58%), hsl(${(a + 50) % 360} 70% 48%))`;
  }

  return { api, t, extend, setLang, get lang() { return lang; }, translate, toast, esc, duration, bytes, cycleTheme, setTheme, renderAppearance, countTo, avatarGradient, store, icon, hydrateIcons };
})();
