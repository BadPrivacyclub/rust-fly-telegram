# Changelog

## 0.2.1

### Fixes and additions after the first 0.2.1 build

- Panel: connected accounts show their Telegram avatar next to the name and ID.
- Anti-delete: chats are now named and get an avatar in the panel. Groups and channels
  whose update did not carry the chat (or whose name has unusual Unicode) are looked up
  instead of being listed as "Unknown chat"; a known title is never overwritten by an
  unknown one. A deletion in one channel can no longer match same-numbered messages of
  another chat.
- **Tor mode**: a `socks5://127.0.0.1:9050` (or `:9150`) proxy is detected as Tor and every
  account gets its own SOCKS identity, so each account uses a separate circuit and a
  different exit IP. Proxies with their own credentials are left alone.
- **Desktop device profiles**: generated or hand-made profiles (device/PC name, OS, app
  version, optional languages) are pinned to accounts. Enable them in the setup wizard or
  later under Settings; new accounts get a generated profile automatically, existing ones
  can be assigned one. Off by default, so existing installations behave as before.

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
