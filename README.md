# ✈️ fly-telegram

![fly-telegram banner](banner.png)

A fast Telegram userbot written in Rust. Uses [grammers](https://codeberg.org/Lonami/grammers) for the MTProto user client, [teloxide](https://github.com/teloxide/teloxide) for the inline Bot API, and [Lua 5.4](https://www.lua.org/) as the module scripting language.

---

## Features

### Included commands

Commands use `.` as the prefix and only respond to **your own messages**. Their availability depends on the corresponding Lua module, permissions, configuration, and any external services noted below.

| Area | Commands | Description |
|---|---|---|
| Core | `.ping`, `.help` | Checks connectivity and displays command help |
| Execution | `.eval`, `.e`, `.term` | Evaluates Lua expressions or runs shell commands |
| Notes and aliases | `.note`, `.alias` | Stores notes and text aliases |
| Module management | `.install`, `.market` | Installs local or remote modules and uses the module marketplace |
| Files | `.dl`, `.sendfile`, `.urlupload`, `.rename` | Downloads, sends, uploads, and renames files |
| Message tools | `.del`, `.info`, `.sd`, `.ytdl` | Manages messages and chat information; `.ytdl` requires `yt-dlp` |
| Handlers | `.afk`, `.autoread`, `.antidelete`, `.pmguard` | Controls automatic message handling and private message protection |
| Group tools | `.cleanjoins`, `.captcha`, `.group` | Configures group protection |
| OSINT | `.ip`, `.domain`, `.rdap` | Performs network and domain lookups |
| AI | `.ai`, `.ask`, `.summarize`, `.transcribe`, `.translate` | Uses configured AI providers |
| Music | `.play`, `.vplay`, `.queue`, `.skip`, `.seek`, `.loop`, `.shuffle`, `.stop`, `.toptracks` | Controls the external `music-worker` service |
| Animations | `.type`, `.scroll`, `.magic`, `.heart` | Produces text animations |
| Automation | `.gifts`, `.taskbot` | Runs dry run automation hooks |
| Process control | `.restart`, `.update` | Terminates the process for an external supervisor, or pulls changes and rebuilds when required |

### Inline bot

A paired Telegram bot handles inline queries and button callbacks. Modules can register UUID-keyed inline results with keyboards that trigger Rust/Lua callbacks when pressed.

### Module system

Modules are plain Lua 5.4 files placed in the `modules/` directory. They are loaded at startup and **hot-reloaded automatically** whenever a file changes. No restart is needed.

Each module receives a `ctx` object with Telegram and database APIs. Sensitive operations are controlled by manifest permissions and the Lua sandbox. See [moduleBuild.md](moduleBuild.md) for the full module authoring guide.

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

Set `FLY_MASTER_PASSWORD` before launch to encrypt the main database, the default session, and anti-delete storage at rest:

Linux and macOS:

```bash
export FLY_MASTER_PASSWORD="change-me"
```

Windows PowerShell:

```powershell
$env:FLY_MASTER_PASSWORD = "change-me"
```

When enabled, `database.json` is written as `database.json.enc`, and
`fly-telegram.session` is sealed as `fly-telegram.session.enc` after a normal
shutdown. The clear session file exists while the process is running because
`grammers` needs a SQLite session file.

Additional account sessions under `sessions/` are not currently encrypted. Use
Ctrl+C for a graceful shutdown that seals the default session. The `.restart`
command terminates the process and expects an external supervisor to start it
again.

Anti-delete storage follows the same master password. With encryption enabled,
`data/antidelete.sqlite` is sealed as `data/antidelete.sqlite.enc` at rest.

### Hot-reload

The `notify` crate watches the `modules/` directory. When a `.lua` file is saved, the old module is unloaded and the new version is loaded without restarting the process.

### Module signing (Ed25519)

Modules that declare `"trusted": true` in their manifest gain access to the full Lua runtime (no sandbox). Without signing, any file dropped into `modules/` with that flag would immediately obtain elevated privileges.

The signing system binds the `trusted` flag to an **operator-held Ed25519 key pair**. On load, the runtime verifies the signature against the module source and its security-relevant manifest fields (`name`, `version`, `permissions`, `trusted`). A missing or invalid signature forces `trusted` to `false` with a warning, even after a hot-reload of a tampered file.

#### Setup (one-time)

```bash
./target/release/fly-telegram --keygen
```

```powershell
.\target\release\fly-telegram.exe --keygen
```

The command prompts for a signing password and writes the encrypted private key to `keys/signing.key.enc` and the raw public key to `keys/signing.pub`.

#### Signing a module

```bash
./target/release/fly-telegram --sign modules/core.lua
```

```powershell
.\target\release\fly-telegram.exe --sign modules\core.lua
```

The command prompts for the signing password, writes the Base64 signature into `core.lua.manifest.json`, and refreshes the source checksum.

Repeat for every module that needs `"trusted": true`. Modules without a valid signature are sandboxed automatically.

#### What is signed

```json
{"name":"core","permissions":[...],"source_sha256":"<hex>","trusted":true,"version":"1.0.0"}
```

Keys are sorted lexicographically, making the payload deterministic. Changing the source file, permissions, name, or version invalidates the signature.

If `keys/signing.pub` is absent, all modules run sandboxed regardless of their manifest.

---

## Requirements

| Tool | Requirement | Notes |
|---|---|---|
| Rust + Cargo | 1.75 or newer recommended | Install via [rustup.rs](https://rustup.rs) |
| C compiler | Required | Use MSVC on Windows or GCC/Clang on Linux and macOS to compile Lua 5.4 |
| Git | Required | Used to clone the repository and by the `.update` command |

No system Lua installation is required. Lua 5.4 is compiled and bundled automatically by Cargo (`vendored` feature).

---

## Getting started

### 1. Obtain Telegram API credentials

Go to [https://my.telegram.org/auth](https://my.telegram.org/auth), sign in, open **API development tools**, and create an application. Save your **API ID** (integer) and **API hash** (string).

### 2. Clone and build

Windows:

```powershell
git clone https://github.com/BadPrivacyclub/rust-fly-telegram.git
cd rust-fly-telegram
compile.bat
```

On Linux or macOS:

```bash
git clone https://github.com/BadPrivacyclub/rust-fly-telegram.git
cd rust-fly-telegram
chmod +x compile.sh
./compile.sh
```

Or build manually:

```bash
cargo build --release
```

The binary is placed at `target/release/fly-telegram` (Linux/macOS) or `target\release\fly-telegram.exe` (Windows).

### 3. First run

Linux and macOS:

```bash
./target/release/fly-telegram
```

Windows:

```powershell
.\target\release\fly-telegram.exe
```

By default, the application starts its authorization server. Open
`http://127.0.0.1:8080` and enter your API ID, API hash, phone number, optional
session name, and optional SOCKS5 proxy. Telegram sends a login code and may ask
for your 2FA password. The authorization server closes after a successful login
and the dashboard starts on the same address.

Pass `--no-web` to skip the web authorization form. In this mode the terminal
prompts for the API ID, API hash, phone number, login code, and optional 2FA
password. The dashboard still starts after the account connects.

Credentials are saved to `database.json`, or to `database.json.enc` when a
master password is configured. The default Telegram session is stored in
`fly-telegram.session`. These files must not be committed to version control.

### 4. Inline bot (optional)

Create a bot via [@BotFather](https://t.me/BotFather) and set the `TELOXIDE_TOKEN` environment variable before launching. As an alternative, store the token under `bot_token` in `database.json`.

Linux and macOS:

```bash
export TELOXIDE_TOKEN="123456789:AAxxxxxx"
```

Windows PowerShell:

```powershell
$env:TELOXIDE_TOKEN = "123456789:AAxxxxxx"
```

Enable inline mode for the bot in BotFather (`/setinline`).

---

## Project structure

- `src/`: Rust application source.
  - `main.rs`: userbot entry point and signing CLI.
  - `music_worker.rs`: external voice chat music worker.
  - `anti_delete.rs`: deleted message archive and search.
  - `crypto.rs`: local encryption and module signing.
  - `database.rs`: persistent JSON storage.
  - `runtime.rs`: shared runtime status and counters.
  - `client/`: Telegram update loop and authorization.
  - `loader/`: Lua module loading, manifests, and runtime context.
  - `bot/`: teloxide inline bot.
  - `web/`: authorization server and web pages.
- `modules/`: hot-reloaded Lua modules.
- `keys/`: generated signing keys.
- `sessions/`: additional Telegram account sessions.
- `data/`: anti-delete storage, downloaded media, and temporary module files.
- `database.json`: runtime configuration and state.
- `fly-telegram.session`: Telegram session data.
- `compile.bat` and `compile.sh`: platform build scripts.
- `moduleBuild.md`: module authoring guide.

---

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `TELOXIDE_TOKEN` | Optional | Bot token for the inline bot subsystem |
| `FLY_MASTER_PASSWORD` | Optional | Encrypts the main database, default session, and anti-delete storage at rest |
| `RUST_LOG` | Optional | Log level, e.g. `fly_telegram=debug` |

---

## CLI flags

| Flag | Description |
|---|---|
| `--no-web` | Skip the web authorization form; the dashboard still starts after connection |
| `--keygen` | Generate an Ed25519 key pair in `keys/` and exit |
| `--sign <path>` | Sign a module's manifest with the operator key and exit |

---

## License

[MIT](LICENSE)
