# Changelog

## Unreleased

### New

- **Chat export** (panel, Export): Telegram's official takeout export (Telegram Desktop
  `result.json` and HTML) and an extended export with rich HTML, statistics, a media gallery,
  search, JSON, single-chat PDF with request and completion stamps, and deleted messages from
  the anti-delete archive marked in place. Per-account export rules, placeholders for left-out
  media, size limits, and an `export_watch` automation that exports chats when they change.
- **Session recovery** (panel, Session recovery): passkey access keys kept in a password
  protected vault, automatic keys for new accounts once Telegram allows it, restore with
  two-step verification, and "Delete forever" for lost accounts.
- **Personas** (`.persona`, `.unpersona`, panel Accounts → Persona): six generated personas
  (anime fan, TikToker, OSINT analyst, anonymous, musician, artist) with a themed name, bio,
  username and public photo from the internet. Profile photos are hidden, not deleted; the photo
  privacy rule with all exceptions, the name, bio, username and previous public photo are
  restored on `.unpersona`. The old username is parked on a private channel meanwhile, with a
  warning first when the public channel limit is reached.
- **Anti-delete in a private channel** (`.antidelete channel on`, `.antidelete text on`):
  messages are forwarded with their author, deleted ones get a card with details and hashtags,
  and the panel shows them with their media.
- **Profile photos in the web panel** for every connected account.
- **Richer PDF export**: chat photo on the cover, a detailed overview, participants with photos
  and message shares, activity charts (by hour, weekday and month), media and reaction
  breakdowns, sender photos and names in the message flow, message IDs, edit, read, view and
  forward details, and a PDF outline. Pictures are embedded as JPEG, so files are much smaller.
- **Premium emoji** in replies (`.emoji`, `ui.emoji(id, fallback)`), and vector icons across
  the web panel.
- **Device profiles** (panel, Settings → Device profiles, Accounts → Device, and the add-account
  wizard): an optional generator gives every new account a different realistic desktop
  profile (system, device, app and version), with your own profiles in a list and a profile
  change for any account. Without the generator, accounts report `Linux · VPS · Fly Telegram
  v.<version>`.
- Access keys record their provider as `Fly-Telegram v.<version>`, shown in Session recovery.

### Changes

- Module HTTP requests (persona pictures included) go through the SOCKS5 proxy from the
  settings.
- Accounts without a chosen profile now report `Linux · VPS · Fly Telegram v.<version>`
  instead of the host system and the library version.
- Anti-delete no longer caches messages or downloads media while it is turned off.
- `persona` and `unpersona` are not available to sudo users.
- A session that Telegram signed out is reported as lost instead of waiting for a login code
  in the terminal.
- Code comments are rewritten, and long dashes are gone from messages and the panel.

## 0.2.2

Module runtime update: fixes for `.weather` and `.rdap`, fewer errors in every module,
and simpler settings.

### Fixes

- **`.weather`** now uses Open-Meteo (free, no key, small and fast answers), with
  wttr.in as a fallback. It accepts coordinates (`.weather 52.52, 13.40`), shows a
  forecast of 1–7 days with rain chances, and answers in Russian when the interface
  language is Russian. The first city you ask for becomes the default.
- **`.rdap`** handles every kind of RDAP answer: domains, IP addresses and ranges, and AS
  numbers (`AS13335`). It shows the registrar, abuse contact, dates, status, and name
  servers. A domain that is not registered gets a clear "Not found" card. When rdap.org
  fails, the registry is asked directly through the IANA bootstrap. URLs like
  `https://example.com/page` are cleaned up automatically.
- **`.ip`** resolves host names, shows your own public IP when called without an
  argument, and falls back to a second provider when the first one rate-limits you.
- Empty values from `ctx:db_get`, `ctx:cfg`, and JSON `null` are now real `nil` in Lua.
  Before, they were a special value that counted as "true", which broke checks like
  `value or default`. The visible effects that are now fixed:
  - `.pmguard` showed "enabled" when it was off;
  - the AI module used an empty API key instead of reporting it missing;
  - `.notes`, webhooks, and settings sometimes showed `userdata: 0x…`.
- Editing a message to the same text no longer posts a duplicate answer.
- `.time` works for any city (looked up through geocoding), with a second time service
  as a fallback.
- `.calc` accepts `2,5`, `×`, `÷`, and unary `+`, and reports division by zero.
- `.cur` accepts `100 usd to eur` and `100usd eur`.
- `.short`, `.paste`, `.market`, `.music`, and the automation webhooks report service
  errors clearly.

### Runtime

- **HTTP for modules** was rewritten:
  - a shared client with a `fly-telegram/<version>` User-Agent (several services
    rejected requests without one);
  - a 25 s timeout and a 4 MiB response limit (before: 12 s and 256 KiB, which wttr.in
    exceeded);
  - errors written for people, for example `rdap.org answered HTTP 404 Not Found` or
    `could not connect to wttr.in: check the internet connection or the proxy`.
- **New `ctx:fetch(url, options)`** never raises for network problems. It returns
  `{ ok, status, text, json, error }`, builds query strings, and sends JSON bodies.
- **Readable error cards**: a failing command now shows the reason without the Lua stack
  trace, a hint (network, rate limit, API key, permission, or a module bug), the
  command's usage, and `module.lua:line`.
- **Required settings**: `required = true` in `M.config` makes commands answer with a
  "Setup needed" card that shows the exact `.cfg` command, instead of failing. The panel
  marks such fields.

### Simpler settings

- `.cfg weather city=Berlin units=imperial` sets several values at once.
- `.cfg` accepts a command name instead of the module name (`.cfg weather city Berlin`)
  and the `module.key` form (`.cfg utils.city Berlin`).
- `.cfg <module>` marks required settings that are not set yet.

### For module authors

- New `ui` helpers:
  - `ui.get` for safe deep access into JSON;
  - `ui.args`, a word splitter that understands quotes;
  - `ui.first`, `ui.num`, `ui.trim`, `ui.clip`;
  - `ui.urlencode` and `ui.query`;
  - `ui.error_text` and `ui.try` for clean error messages.
- The module test suite runs every command of every bundled and example module with odd
  input, with the network down, with HTTP 404 answers, and without a replied message. It
  also checks weather, RDAP, IP, DNS, time, and currency against real API responses.
- The guide documents `ctx:fetch`, the new helpers, `required` settings, and more common
  errors.

## 0.2.1

### Download and run

- Docker support is gone. fly-telegram is a single program: download, unpack, and
  double-click.
- Without `--data-dir`, data is kept next to the executable (portable), or in
  `~/fly-telegram` when the executable sits in a system folder.
- The setup wizard opens in your browser on first run (`--no-browser` turns this off).
- A startup banner shows the data folder and panel address. On Windows, a console window
  started with a double-click stays open when something goes wrong.
- Release archives include `START-HERE.txt` in English and Russian.

### Account tools

- **Smart cleanup** for one or several accounts at once. It can delete contacts, leave
  groups and channels, and delete private and bot chats.
  - Filters: inactivity, a never-touch list (IDs or @usernames), admin chats, pinned
    chats, archive only.
  - Chats you own are never touched.
  - There is always a preview first, with a single-use confirmation code (Telegram) or a
    typed confirmation (panel). A JSON export is saved before anything is deleted.
  - Requests are paced to stay under Telegram's limits; FLOOD_WAIT is handled
    automatically, and you can cancel at any time.
- **Bulk actions**: read all (including mentions), archive inactive chats, mute all,
  export contacts and chats.
- **Profile**: name, bio, username.
- **Background tasks** show progress, a log, and a cancel button. The control bot
  notifies you when a task finishes.
- Commands: `.acc`, `.cleanup`, `.readall`, `.archive`, `.muteall`, `.export`, `.jobs`,
  `.profile`.

### Automations

- Rules for one or several accounts, managed in the panel or with `.auto` and `.away`:
  - scheduled messages (every N minutes, or daily at a set time on chosen weekdays);
  - away replies in a time window;
  - auto-forward with keyword filters;
  - auto-delete of your own messages;
  - a clock in your name or bio (`{time} {date} {weekday} {clock}`);
  - bio rotation;
  - always online or always offline;
  - auto-read for chosen chats.

### Handy extras

- `.coin`, `.dice`, `.random`, `.countdown`, `.time`, `.pass`, `.b64`, `.hash`,
  `.uuid`, `.short`, `.id`.

### Web panel

- **12 themes**: Auto, Light, Dark, AMOLED, Midnight, Ocean, Forest, Sunset, Rose, Nord,
  Dracula, High contrast. Pick any accent color.
- **Effects**: glass cards, an animated gradient background, entrance animations, and
  animated counters. One switch turns them off, and they respect your system's
  reduced-motion setting.
- **New pages**:
  - Accounts: select accounts, see live chat counts, run bulk actions, open the
    cleanup wizard, edit profiles.
  - Automations: a visual rule editor.
  - Tasks: progress and logs.
- **Ctrl+K command palette** for pages, actions, themes, and module settings.
- **Getting started** checklist on the overview.

### For module authors

- The guide now starts with a 5-minute tutorial. It adds a cookbook, debugging tips, a
  table of common errors, and the marketplace catalog format.
- Example modules in `examples/modules/` are exercised by the test suite.
- New `ctx` methods: `sha256`, `uuid`, account tools, automations.

## 0.2.0

A large update focused on features, a redesigned interface in the panel and in Telegram,
and much easier installation.

### Installation and setup

- **One-line installers** for Linux/macOS (`install.sh`) and Windows (`install.ps1`). They
  download a verified release and fall back to building from source.
- **Prebuilt releases** for Linux x86_64/arm64, Windows x86_64, and macOS Intel/Apple
  Silicon, published automatically for every `v*` tag with SHA-256 checksums.
- **Single executable**: built-in modules are embedded and installed on first start.
  Later releases upgrade the ones you have not edited.
- **Browser setup wizard**: panel password, optional control bot, API keys with
  instructions, then phone, code, and 2FA. The panel opens without a second login.
- **`doctor`** checks the environment, and **`service install`** sets up systemd,
  launchd, or Task Scheduler.
- **`config.toml`**, `FLY_*` environment variables, and `--data-dir`, `--host`, `--port`, and
  `--no-panel` flags. Secrets for background services go in `fly-telegram.env`.
- **Self-update**: `.update now` or the panel installs the latest release and restarts
  in place.
- The Bot API client now uses rustls, so the build no longer depends on OpenSSL.

### Features

- New commands:
  - Settings: `.prefix`, `.lang`, `.modules`, `.module on|off|reload|remove|restore`,
    `.cfg`, `.sudo`, `.panel`.
  - Reminders: `.remind` and `.later`, using Telegram scheduled messages, so they work
    even while the userbot is offline.
  - Auto-replies: `.filter`, `.unfilter`, `.filters` (keyword replies per chat).
  - Groups: `.welcome` greetings, `.purge`, `.pin`, `.unpin`, `.tagall`.
  - Utilities: `.weather`, `.cur`, `.qr`, `.calc` (a safe parser), `.paste`.
  - Backups: `.backup` and `.restore`, plus download and restore in the panel.
  - Also new: `.stats`, `.approve`, `.block`, and named notes with `.notes`.
- **Command prefixes** are configurable, and you can have several.
- **Sudo users** can run safe commands for you.
- **Generated help**: `.help` lists every module by category, and `.help <module|command>`
  shows usage. With the control bot, help appears as an interactive message with buttons.
- **Module settings** (`M.config`) are validated and editable from Telegram, the bot,
  and the panel. The AI module's API keys and models moved there.
- **Modules can be enabled and disabled** without deleting them.
- **Event handlers** (`on_message`) let modules react to messages and group joins.
- **Restarts** happen in place. The command message is then edited to show "Restarted in N s".
- **English and Russian** for core messages, the bot, and the panel.

### Control bot

- Owner-only menu: status, modules (toggle and reload), settings toggles, recent errors,
  panel login link, language, and restart.
- Notifications for module errors, startup, new private messages (with **Allow** and
  **Block** buttons), and deleted messages. Each type can be switched off.
- Changing the bot token in the panel restarts the bot without restarting the userbot.

### Web panel

- Redesigned with a sidebar, light and dark themes, and a mobile layout. It loads no
  external assets.
- Overview with a 60-minute activity chart, top commands, accounts, and setup hints.
- Modules: search and filters, settings forms, permission grants for sandboxed modules,
  source viewer, reload, delete, install from a URL.
- Live logs, anti-delete search with media previews, backups, full settings, and update
  checks.

### Telegram interface

- Every built-in module uses one message style through the new `ui` library: titled
  cards, status lines, usage hints, durations, and progress bars.
- Usage hints show your actual prefix.

### Security

- **The web panel now requires a login** (password or one-time link). Earlier versions
  served it without authentication and with permissive CORS. Any website open in your
  browser could change the proxy or the master password.
- Only loopback host names are accepted, which blocks DNS rebinding. State-changing
  requests need a CSRF header and a same-origin `Origin`.
- Sandboxed modules can no longer read credentials, the bot token, panel settings, or
  other modules' settings through `ctx:db_get`.
- Built-in modules now actually run trusted. Before, their `trusted` flag was cleared
  because they had no operator signature. Trust now comes from matching the copy
  embedded in the binary.
- `.ytdl` passes arguments directly to yt-dlp instead of going through a shell.

### Fixes

- A long-running command, such as `.sd 60 …`, no longer stalls every other command when a
  module is hot-reloaded.
- A malformed `tg://user?id=` link in a module's output no longer crashes the handler.
- `.restart` works without an external supervisor.

### For module authors

- New `ctx` methods:
  - Messages: `message`, `replied`, `answer`, `schedule`, `send_saved`, `pin`, `unpin`,
    `purge`, `tag_all`, `react`.
  - Settings and help: `cfg`, `cfg_set`, `prefix`, `lang`, `help`.
  - Other: `notify`, `db_keys`, `run_process`.
- `ctx:db_set` stores tables.
- New `ui` and `json` globals.
- New permissions: `telegram.read` (needed for `on_message` in sandboxed modules) and
  `core.admin`.

See [moduleBuild.md](moduleBuild.md).

## 0.1.0

Initial Rust release: grammers userbot with Lua modules, inline bot, web authorization,
dashboard, anti-delete, module signing, and encryption at rest.
