# ✈️ fly-telegram

![fly-telegram banner](banner.png)

A fast Telegram userbot written in Rust. Uses [grammers](https://codeberg.org/Lonami/grammers) for the MTProto user client, [teloxide](https://github.com/teloxide/teloxide) for the inline Bot API, and [Lua 5.4](https://www.lua.org/) as the module scripting language.

---

## Features

### Built-in commands

All commands use `.` as the prefix and only respond to **your own messages**.

| Command | Alias | Description |
|---|---|---|
| `.ping` | — | Replies with `pong` to check connectivity |
| `.eval <expression>` | `.e` | Evaluates a Lua expression and shows the result |
| `.term <command>` | — | Runs a shell command and shows its output |
| `.install <file-or-url> [name]` | — | Installs a Lua module into `modules/` |
| `.note set|get|clear` | — | Stores and reads a simple note |
| `.alias set|get|del` | — | Stores and reads text aliases |
| `.help` | — | Shows all available commands |
| `.restart` | — | Saves state and restarts the process |
| `.update` | — | Checks for updates (stub, extend via module) |

### Inline bot

A paired Telegram bot handles inline queries and button callbacks. Modules can register UUID-keyed inline results with keyboards that trigger Rust/Lua callbacks when pressed.

### Module system

Modules are plain Lua 5.4 files placed in the `modules/` directory. They are loaded at startup and **hot-reloaded automatically** whenever a file changes — no restart needed.

Each module receives a `ctx` object with the full Telegram and database API. See [moduleBuild.md](moduleBuild.md) for the full module authoring guide.

### Persistent database

A flat JSON file (`database.json`) acts as a key-value store accessible from both Rust and Lua. Writes are crash-safe: each flush writes to a `.tmp` sibling file and renames it over the target, so the database is never left in a truncated state.

Anti-delete history is stored separately in `data/antidelete.sqlite` so deleted
message archives do not bloat or expose the main JSON key-value store.

### Web authorization

On first launch (no saved session), an [axum](https://github.com/tokio-rs/axum) HTTP server starts at `http://127.0.0.1:8080` with a minimal login form. After successful sign-in the server shuts down automatically.

The login form accepts an optional SOCKS5 proxy URL:

```text
socks5://127.0.0.1:9050
socks5://user:password@127.0.0.1:9050
```

Use an IP address for the proxy host if you need to avoid local DNS lookup for
the proxy itself. Telegram datacenter targets are connected by IP through the
SOCKS5 tunnel.

### Multiple accounts

Use a different session name in `/login` to authorize another account. The
dashboard starts a separate update loop for the new session immediately and
remembers it in `session_files`, so all known accounts are connected again on
the next launch.

### Dashboard

After the userbot connects, the same local web port becomes a dashboard at
`http://127.0.0.1:8080`. It shows uptime, connection state, account name,
per-account status, update and command counters, loaded modules, proxy state,
and database encryption state.

The dashboard settings panel can update the stored SOCKS5 proxy URL and change
or clear the master password used for local encrypted storage. Proxy changes
apply to new MTProto connections after restart.

### Master password

Set `FLY_MASTER_PASSWORD` before launch to encrypt local state at rest:

```bash
# Linux / macOS
export FLY_MASTER_PASSWORD="change-me"

# Windows (PowerShell)
$env:FLY_MASTER_PASSWORD = "change-me"
```

When enabled, `database.json` is written as `database.json.enc`, and
`fly-telegram.session` is sealed as `fly-telegram.session.enc` after a normal
shutdown. The clear session file exists while the process is running because
`grammers` needs a SQLite session file.

Anti-delete storage follows the same master password. With encryption enabled,
`data/antidelete.sqlite` is sealed as `data/antidelete.sqlite.enc` at rest.

### Hot-reload

The `notify` crate watches the `modules/` directory. When a `.lua` file is saved, the old module is unloaded and the new version is loaded without restarting the process.

---

## Requirements

| Tool | Minimum version | Notes |
|---|---|---|
| Rust + Cargo | 1.75 | Install via [rustup.rs](https://rustup.rs) |
| C compiler | — | MSVC (Windows) or GCC/Clang (Linux/macOS) — required to compile Lua 5.4 |
| Git | any | Needed to clone the repo |

No system Lua installation is required — Lua 5.4 is compiled and bundled automatically by Cargo (`vendored` feature).

---

## Getting started

### 1. Obtain Telegram API credentials

Go to [https://my.telegram.org/auth](https://my.telegram.org/auth), sign in, open **API development tools**, and create an application. Save your **API ID** (integer) and **API hash** (string).

### 2. Clone and build

```bash
git clone <repo-url>
cd rust-fly-telegram

# Windows
compile.bat

# Linux / macOS
chmod +x compile.sh
./compile.sh
```

Or build manually:

```bash
cargo build --release
```

The binary is placed at `target/release/fly-telegram` (Linux/macOS) or `target\release\fly-telegram.exe` (Windows).

### 3. First run

```bash
# Linux / macOS
./target/release/fly-telegram

# Windows
.\target\release\fly-telegram.exe
```

On first launch you will be prompted for:

1. **API ID** — integer from my.telegram.org
2. **API hash** — string from my.telegram.org
3. **Phone number** — in international format, e.g. `+79001234567`
4. **Login code** — sent to your Telegram account
5. **2FA password** — only if your account has two-factor authentication enabled

Credentials are saved to `database.json`. The session key is saved to `fly-telegram.session`. Neither file should be committed to version control.

### 4. Inline bot (optional)

Create a bot via [@BotFather](https://t.me/BotFather) and set the `TELOXIDE_TOKEN` environment variable before launching:

```bash
# Linux / macOS
export TELOXIDE_TOKEN="123456789:AAxxxxxx"

# Windows (PowerShell)
$env:TELOXIDE_TOKEN = "123456789:AAxxxxxx"
```

Enable inline mode for the bot in BotFather (`/setinline`).

---

## Project structure

```
rust-fly-telegram/
├── src/
│   ├── main.rs            Entry point
│   ├── database.rs        JSON key-value store
│   ├── watcher.rs         Hot-reload file watcher
│   ├── client/
│   │   ├── mod.rs         grammers update loop
│   │   └── auth.rs        CLI + web authorization
│   ├── loader/
│   │   ├── mod.rs         Lua module loader & dispatcher
│   │   └── context.rs     ctx UserData exposed to Lua
│   ├── bot/
│   │   └── mod.rs         teloxide inline bot
│   └── web/
│       ├── mod.rs         axum login server
│       └── login.html     Login page
├── modules/               Lua modules (hot-reloaded)
│   ├── core.lua
│   ├── executor.lua
│   ├── help.lua
│   └── updater.lua
├── database.json          Created on first run
├── fly-telegram.session   Created on first run
├── compile.bat            Windows build script
├── compile.sh             Linux/macOS build script
├── README.md
└── moduleBuild.md         Module authoring guide
```

---

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `TELOXIDE_TOKEN` | Optional | Bot token for the inline bot subsystem |
| `FLY_MASTER_PASSWORD` | Optional | Encrypts `database.json` and the session file at rest |
| `RUST_LOG` | Optional | Log level, e.g. `fly_telegram=debug` |

---

## License

MIT
