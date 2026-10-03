# Module authoring guide

fly-telegram modules are Lua 5.4 scripts in the `modules/` directory of your data
directory. They load at startup and **reload automatically** when you save the file.

- [Tutorial: your first module in 5 minutes](#tutorial-your-first-module-in-5-minutes)
- [Minimal module](#minimal-module)
- [Module structure](#module-structure)
- [Help: `M.help`](#help-mhelp)
- [Settings: `M.config`](#settings-mconfig)
- [Events: `M.on_message`](#events-mon_message)
- [The `ctx` API](#the-ctx-api)
- [The `ui` library](#the-ui-library)
- [The `json` library](#the-json-library)
- [Permissions and the sandbox](#permissions-and-the-sandbox)
- [Manifests and signing](#manifests-and-signing)
- [Hot reload, enabling, disabling](#hot-reload-enabling-disabling)
- [Full example](#full-example)
- [Cookbook](#cookbook)
- [Debugging and testing](#debugging-and-testing)
- [Common errors](#common-errors)
- [Publishing to the marketplace](#publishing-to-the-marketplace)
- [Example modules](#example-modules)

---

## Tutorial: your first module in 5 minutes

You only need a text editor and a running fly-telegram. A Lua module is a single file.

**1. Create the file.** Find the `modules/` folder in your data directory: the folder
next to the executable, or the one shown as `Data:` when fly-telegram starts. Create
`modules/greet.lua`:

```lua
local M = {}

M.commands = { greet = "greet_cmd" }

function M.greet_cmd(ctx, args)
    ctx:edit("👋 Hi, " .. (args ~= "" and args or "stranger") .. "!")
end

return M
```

**2. Try it.** In any chat, send `.greet Alex`. Your message changes to
`👋 Hi, Alex!`. You don't need to restart: modules reload as soon as you save the file.

**3. Make it pretty.** Swap the raw string for the shared `ui` helpers, so the output
looks like the built-in modules:

```lua
ctx:edit(ui.ok("Hi, " .. (args ~= "" and args or "stranger") .. "!"))
```

**4. Document it.** Add `M.help`, and the command appears in `.help` with a description,
both in Telegram and in the panel:

```lua
M.help = {
    category = "fun",
    description = "Friendly greetings",
    commands = { greet = { args = "[name]", desc = "Say hi" } },
}
```

**5. Make it configurable.** Declare a setting. It shows up in `.cfg greet` and as a form
in the panel (Modules → Configure):

```lua
M.config = {
    { key = "emoji", type = "string", default = "👋", description = "Emoji before the greeting" },
}
-- in the handler:
ctx:edit(ctx:cfg("emoji") .. " Hi, " .. name .. "!")
```

**6. Remember things.** `ctx:db_set` and `ctx:db_get` persist values, including
tables, across restarts:

```lua
local count = (ctx:db_get("greet.count") or 0) + 1
ctx:db_set("greet.count", count)
```

That covers the essentials. The rest of this guide is reference material and recipes.
Copy a module from [`examples/modules/`](examples/modules) to start from working code.

---

## Minimal module

```lua
local M = {}

M.commands = { hello = "hello_cmd" }

function M.hello_cmd(ctx, args)
    ctx:edit("👋 Hello, " .. (args ~= "" and args or "world") .. "!")
end

return M
```

Save it as `modules/hello.lua` and send `.hello` from your account.

---

## Module structure

A module returns a table:

| Field | Required | Purpose |
|---|---|---|
| `M.commands` | yes | Command name → handler function name |
| `M.help` | recommended | Category, description, and per-command usage for `.help`, the bot, and the panel |
| `M.config` | optional | Settings schema: validated, and editable with `.cfg` and in the panel |
| `M.on_message` | optional | Called for every new message (see [events](#events-mon_message)) |
| `M.meta` | optional | Informational (`name`, `version`); the manifest is authoritative |

Handlers receive `ctx` and `args`. `args` is everything after the command, trimmed:

```text
.greet  Alice Bob    →  args == "Alice Bob"
```

Several commands can point to the same handler (aliases):

```lua
M.commands = { eval = "eval_cmd", e = "eval_cmd" }
```

Command names are matched case-insensitively against every configured prefix
(`.` by default; see `.prefix`).

---

## Help: `M.help`

```lua
M.help = {
    category = "utils",            -- see the list below, or any custom title
    description = "Everyday helpers",
    commands = {
        weather = { args = "[city]", desc = "Current weather" },
        calc = "Calculator",       -- shorthand: description only
    },
}
```

Known categories, shown in this order and translated: `core`, `modules`, `messaging`,
`automation`, `security`, `groups`, `info`, `utils`, `files`, `ai`, `music`, `fun`.
Commands you don't document are still listed.

`.help weather` shows the module card with the `weather` line highlighted.

---

## Settings: `M.config`

Declare settings once. Users edit them with `.cfg <module> <key> <value>`, in the bot,
or in a panel form, and you read them with `ctx:cfg(key)`.

```lua
M.config = {
    { key = "provider", type = "select", options = { "openai", "anthropic" }, default = "openai",
      description = "Which provider to use" },
    { key = "api_key", type = "string", secret = true, description = "API key" },
    { key = "limit", type = "number", default = 10 },
    { key = "verbose", type = "bool", default = false },
    { key = "prompt", type = "text", default = "Be brief." },
}
```

| Type | Accepted input |
|---|---|
| `string`, `text` | any text (`text` gets a multi-line editor in the panel) |
| `number` | integers or decimals |
| `bool` | `on/off`, `true/false`, `yes/no`, `1/0` |
| `select` | one of `options` |

- `ctx:cfg(key)` returns the stored value, or `default` when nothing is stored.
- `ctx:cfg_set(key, value)` validates and stores the value. `nil` resets it.
- `secret = true` hides the value in `.cfg`, the bot, and the panel. It can be replaced
  but never read back.
- Values are stored under `cfg.<module>.<key>`, and sandboxed modules can only access
  their own.

---

## Events: `M.on_message`

```lua
function M.on_message(ctx, msg)
    if msg.from_self or msg.is_command then return end
    if msg.text:lower():find("ping") then
        ctx:answer("pong")
    end
end
```

The function runs for every **new** message in every chat, including your own. Keep it
fast and return early. `ctx` is bound to that message, so `ctx:reply`, `ctx:answer`, and
`ctx:react` act on it.

Fields of `msg` (the same table `ctx:message()` returns):

| Field | |
|---|---|
| `id`, `chat_id`, `text`, `date` | Basic data (`chat_id` uses the Bot API format) |
| `sender_id`, `sender_name`, `sender_username`, `sender_is_bot` | Sender details, when known |
| `chat_title` | Group or channel title, or the user's name in private chats |
| `outgoing`, `from_self` | Sent by this account |
| `mentioned` | You were mentioned |
| `is_private`, `is_saved`, `is_group` | Chat type |
| `is_command` | Starts with a configured prefix |
| `reply_to_id`, `has_media` | |
| `action`, `action_users` | `"join"`, `"leave"`, or `"other"` for service messages, plus the affected user IDs |

Sandboxed modules receive events only with the `telegram.read` permission.

---

## The `ctx` API

Methods are called with `:` and run asynchronously; Lua awaits them for you. Methods
marked with a permission fail with a clear error if the module doesn't have it.

### Messages

| Method | Description |
|---|---|
| `ctx:edit(text)` | Edit the command message (falls back to a new message) |
| `ctx:reply(text)` | Send a message to the same chat |
| `ctx:answer(text)` | Reply quoting the current message |
| `ctx:delete()` | Delete the current message |
| `ctx:react(emoji)` | React to the current message |
| `ctx:message()` | Current message as a table (see [events](#events-mon_message)) |
| `ctx:replied()` | The replied message: `{ id, text, sender_id, sender_name, sender_username, outgoing, has_media }`, or `nil` |
| `ctx:message_text()`, `ctx:replied_text()` | Text only |
| `ctx:schedule(seconds, text, target)` | Telegram-scheduled message. `target` is `"here"` (default) or `"me"` (Saved Messages), with a delay from 10 s to 365 days |
| `ctx:send_saved(text)` | Send to Saved Messages |
| `ctx:pin()`, `ctx:unpin()` | Pin or unpin the replied message (`telegram.history`) |
| `ctx:purge()` | Delete from the replied message to the current one; returns the count (`telegram.history`) |
| `ctx:tag_all(text, limit)` | Mention group members, five per message (`telegram.history`) |
| `ctx:delete_last_own(n)` | Delete your last `n` messages here (`telegram.history`) |
| `ctx:message_info()` | Chat, user, and DC info as markdown (`telegram.history`) |

Text is Telegram markdown: `**bold**`, `__italic__`, `` `code` ``, fenced code blocks,
and `[text](url)`. Long text is split automatically.

### Storage and settings

| Method | Description |
|---|---|
| `ctx:db_get(key)` | Stored value, or `nil` |
| `ctx:db_set(key, value)` | Store a string, number, boolean, or table. `nil` deletes the key |
| `ctx:db_keys(prefix)` | Sorted keys starting with `prefix` |
| `ctx:cfg(key)`, `ctx:cfg_set(key, value)` | Module settings (see above) |
| `ctx:prefix()` | The first command prefix, for usage hints |
| `ctx:lang()` | `"en"` or `"ru"` |
| `ctx:help(query)` | Help text for everything, a module, or a command |

Use a prefix like `mymodule.` for your keys. Keys under `web.`, `core.`, `notify.`,
`pmguard.`, other modules' `cfg.`, and credentials are off-limits to sandboxed modules.

### Network (`network`)

| Method | Description |
|---|---|
| `ctx:http_get(url)` | Text response (up to 256 KiB, 12 s timeout) |
| `ctx:http_json_get(url)` | Decoded JSON |
| `ctx:http_request(method, url, body, headers)` | Text response |
| `ctx:http_json_request(method, url, body, headers)` | Decoded JSON |
| `ctx:http_json_multipart_file_request(method, url, field, path, fields, headers)` | Upload a local file (also needs `telegram.media`) |

### Files (`telegram.media`)

| Method | Description |
|---|---|
| `ctx:download_replied_media(name)` | Save replied media under `data/downloads/`; returns the path |
| `ctx:download_url(url, name)` | Download a URL under `data/downloads/` (also needs `network`) |
| `ctx:send_file(path, caption)` | Upload a relative path to the chat |

Paths must be relative and must not contain `..`.

### Runtime and processes

| Method | Description |
|---|---|
| `ctx:runtime_stats()` | `{ version, uptime_seconds, cpu_percent, memory_bytes, updates_seen, commands_seen, errors_seen, modules, accounts, top_commands, os, arch }` |
| `ctx:now_ms()`, `ctx:sleep(s)`, `ctx:sleep_ms(ms)` | Time helpers (sleeps are capped at one day) |
| `ctx:notify(text)` | Send a notification through the control bot |
| `ctx:sanitize(text)` | Mask secrets (API hash, tokens) in text before you show it |
| `ctx:env_get(name)` | Read an environment variable (`secrets`) |
| `ctx:run_process(program, args)` | Run a program without a shell and show its output (`shell`) |
| `ctx:run_term(command)` | Run a shell command with live output (`shell`) |

### Utilities

| Method | Description |
|---|---|
| `ctx:sha256(text)` | Hex SHA-256 of a string |
| `ctx:uuid()` | Random UUID v4 |

### Administration (`core.admin`)

Used by the built-in `settings`, `updater`, and `backup` modules: `modules_list`,
`module_set_enabled`, `module_reload`, `module_remove`, `module_restore`,
`module_config`, `module_config_set`, `set_prefixes`, `set_lang`, `sudo_list`,
`sudo_set`, `panel_link`, `restart`, `check_update`, `self_update`, `backup`,
`restore_backup`, `install_module` (`modules.install`).

Account tools (used by the built-in `account` module):

| Method | Description |
|---|---|
| `ctx:account()` | `{ id, name, session }` of the account that received the command |
| `ctx:account_counts()` | `{ users, bots, groups, channels, contacts, archived, unread }` |
| `ctx:cleanup_preview(options)` | Builds a cleanup plan and returns counts, a sample, and a confirmation `code` |
| `ctx:cleanup_run(code)` | Starts the previewed cleanup as a background task; returns the task ID |
| `ctx:account_bulk(action, days)` | `read_all`, `archive_inactive`, `mute_all`, or `export` as a task |
| `ctx:jobs()`, `ctx:job_cancel(id)` | Background tasks and cancellation |
| `ctx:profile_update({ first_name, last_name, about, username })` | Edit the profile |

Cleanup options: `contacts`, `groups`, `channels`, `private_chats`, `bots` (what to
clean), and filters `inactive_days`, `keep_ids`, `keep_usernames`, `keep_admin`
(default `true`), `keep_pinned` (default `true`), `archived_only`, `revoke`.

Automations (used by the built-in `auto` module):

| Method | Description |
|---|---|
| `ctx:auto_rules()` | All rules |
| `ctx:auto_save(rule)` | Create or update a rule (validated); returns the saved rule with its `id` |
| `ctx:auto_toggle(id, enabled)`, `ctx:auto_delete(id)` | Manage rules |

A rule is a table with `type` (`schedule`, `away_reply`, `forward`, `auto_delete`,
`profile_clock`, `bio_rotation`, `presence`, `auto_read`), optional `name`,
`accounts` (user IDs, empty for all), `utc_offset_minutes`, and the type's own fields,
the same ones the panel editor shows.

---

## The `ui` library

A global `ui` table gives every module the same look. Each function returns a string;
pass it to `ctx:edit`.

```lua
ctx:edit(ui.card("📝", "Notes", { ui.kv("Saved", 3), ui.status(true) }, "Footer hint"))
ctx:edit(ui.ok("Saved", ui.mono(name)))
ctx:edit(ui.err("Request failed", tostring(err)))
ctx:edit(ui.usage(ctx:prefix(), "note", { "save <name> <text>", "del <name>" }))
```

| Function | Result |
|---|---|
| `ui.card(icon, title, lines, footer)` | Bold title, lines joined with line breaks, italic footer |
| `ui.ok/err/warn/info(title, detail)` | Card with ✅ ❌ ⚠️ ℹ️ |
| `ui.toggle(title, enabled, detail)` | 🟢/⚪ status card |
| `ui.usage(prefix, command, variants, description)` | Usage block |
| `ui.wait(text)` | ⏳ progress line |
| `ui.kv(label, value)` | `▸ label: value` |
| `ui.status(on, on_text, off_text)` | 🟢 on / ⚪ off |
| `ui.mono(v)`, `ui.bold(v)`, `ui.italic(v)`, `ui.code(text, lang)` | Formatting, safely escaped |
| `ui.list(items, bullet)`, `ui.section(title, lines)`, `ui.join(lines)` | Layout |
| `ui.bar(fraction, width)` | `▰▰▰▱▱` |
| `ui.duration(seconds)`, `ui.bytes(n)` | `1h 5m`, `3.2 MB` |
| `ui.parse_duration("1h30m")` | `5400` (accepts `s m h d w` or plain seconds); `nil` if invalid |
| `ui.mention(user_id, name)` | Clickable mention |
| `ui.split(args)` | First word and the rest |
| `ui.icons` | Shared emoji set |

---

## The `json` library

```lua
local body = json.encode({ model = "x", messages = { { role = "user", content = prompt } } })
local data = json.decode('{"ok": true}')
```

Use it instead of building JSON strings by hand.

---

## Permissions and the sandbox

**Trusted** modules run with the full Lua standard library and every permission. A
module is trusted when it is:

- a built-in module that is byte-identical to the copy embedded in the binary, or
- signed with your operator key (see below).

Every other module runs **sandboxed**:

- Only `assert error ipairs next pairs pcall select tonumber tostring type xpcall`,
  `coroutine math string table utf8`, `ui`, and `json` are available. There is no `os`,
  `io`, `load`, or `require`.
- `ctx` methods require the permissions listed in its manifest:

| Permission | Grants |
|---|---|
| `network` | HTTP requests |
| `telegram.read` | `on_message` events |
| `telegram.history` | Reading and deleting history, pins, mentions |
| `telegram.media` | Downloading and uploading files |
| `secrets` | Environment variables |
| `shell` | Running programs ⚠️ |
| `modules.install` | Installing other modules ⚠️ |
| `core.admin` | Userbot settings, restart, update, backups ⚠️ |

⚠️ These permissions effectively hand over control of the computer or the account.

Grant permissions in the panel (Modules → Permissions), or edit the manifest. The panel
also lists the capabilities the source code appears to use.

---

## Manifests and signing

A module can ship a manifest next to it, `modules/<name>.lua.manifest.json`:

```json
{
  "name": "example",
  "version": "1.0.0",
  "description": "Example module",
  "commands": ["example"],
  "permissions": ["network"],
  "trusted": false
}
```

`.install <url>` and the panel's installer generate one with no permissions.

To trust your own module completely:

```bash
fly-telegram keygen                      # once; writes keys/signing.key.enc and keys/signing.pub
fly-telegram sign modules/example.lua    # sets "trusted": true and adds a signature
```

The signature covers the source hash, `name`, `version`, and `permissions`, so any change
to the file or these fields drops the module back into the sandbox until you sign it again.

---

## Hot reload, enabling, disabling

- Saving a `.lua` file or its manifest reloads the module. If the new version fails to
  load, the error goes to the log; fix the file and save again.
- `.module off <name>` (or the switch in the panel) disables a module and keeps its file
  and data. `.module on <name>` enables it again.
- `.module reload <name>` reloads explicitly. `.module restore <name>` restores the
  embedded copy of a built-in module.
- Top-level `local` variables live until the next reload. Use `ctx:db_set` for anything
  that must survive a restart.

Handler errors appear in the chat with the module and handler name, go to the log and
the panel's error counter, and, when enabled, are sent to you through the bot.

---

## Full example

```lua
local M = {}

M.commands = { todo = "todo_cmd" }

M.help = {
    category = "messaging",
    description = "A tiny to-do list",
    commands = { todo = { args = "add <text> | done <n> | list", desc = "Manage your to-do list" } },
}

M.config = {
    { key = "max_items", type = "number", default = 20, description = "List size limit" },
}

local KEY = "todo.items"

local function items(ctx)
    local list = ctx:db_get(KEY)
    return type(list) == "table" and list or {}
end

function M.todo_cmd(ctx, args)
    local action, rest = ui.split(args)
    local list = items(ctx)

    if action == "add" and rest ~= "" then
        if #list >= ctx:cfg("max_items") then
            ctx:edit(ui.warn("List is full"))
            return
        end
        table.insert(list, rest)
        ctx:db_set(KEY, list)
        ctx:edit(ui.ok("Added", ui.mono(rest)))
    elseif action == "done" and tonumber(rest) then
        local removed = table.remove(list, tonumber(rest))
        ctx:db_set(KEY, list)
        ctx:edit(removed and ui.ok("Done", removed) or ui.err("No such item"))
    elseif action == "list" or action == "" then
        local lines = {}
        for i, item in ipairs(list) do
            table.insert(lines, ui.mono(i) .. " " .. item)
        end
        ctx:edit(ui.card("✅", "To-do", #lines > 0 and lines or { ui.italic("Empty") }))
    else
        ctx:edit(ui.usage(ctx:prefix(), "todo", { "add <text>", "done <n>", "list" }))
    end
end

return M
```

---

## Cookbook

Short, copyable answers to common tasks.

### Parse arguments

```lua
local action, rest = ui.split(args)            -- "add milk and eggs" → "add", "milk and eggs"
local n, text = args:match("^(%d+)%s+(.+)$")   -- "5 hello" → "5", "hello"
local words = {}
for word in args:gmatch("%S+") do table.insert(words, word) end
```

### Reply to a message or use its text

```lua
local text = args ~= "" and args or ctx:replied_text()
local replied = ctx:replied()            -- nil when the command is not a reply
if replied then ctx:edit("Replying to " .. (replied.sender_name or "someone")) end
```

### Store lists and objects

```lua
local list = ctx:db_get("mymod.list") or {}
table.insert(list, { text = "milk", at = ctx:now_ms() })
ctx:db_set("mymod.list", list)
ctx:db_set("mymod.list", nil)            -- delete
for _, key in ipairs(ctx:db_keys("mymod.")) do ... end
```

### Call an HTTP API

```lua
-- needs "network"
local data = ctx:http_json_get("https://api.example.com/v1/thing?id=" .. id)
local body = json.encode({ query = text })
local result = ctx:http_json_request("POST", "https://api.example.com/v1/search", body,
    { ["content-type"] = "application/json", ["authorization"] = "Bearer " .. ctx:cfg("api_key") })
```

Wrap calls in `pcall` when a failure should become a friendly message instead of an error:

```lua
local ok, data = pcall(function() return ctx:http_json_get(url) end)
if not ok then return ctx:edit(ui.err("Service unavailable", tostring(data))) end
```

### Keep a secret in settings

```lua
M.config = { { key = "api_key", type = "string", secret = true, description = "Your API key" } }
local key = ctx:cfg("api_key")
if not key or key == "" then
    return ctx:edit(ui.warn("No API key", "Set it with " .. ui.mono(ctx:prefix() .. "cfg mymod api_key <key>")))
end
```

### React to messages

```lua
function M.on_message(ctx, msg)          -- needs "telegram.read" when sandboxed
    if msg.from_self or msg.is_command then return end
    if msg.is_private and msg.text:lower():find("price") then
        ctx:answer("See the pinned message 📌")
    end
end
```

### Send something later

```lua
ctx:schedule(ui.parse_duration("2h"), "⏰ Stretch!", "me")   -- Saved Messages, delivered by Telegram
ctx:schedule(600, "Meeting in 10 minutes", "here")
```

### Show progress during long work

```lua
for i = 1, total do
    if i % 10 == 0 then ctx:edit(ui.wait("Working… " .. ui.bar(i / total) .. " " .. i .. "/" .. total)) end
    -- ...
end
```

### Notify yourself

```lua
ctx:notify("Backup finished")   -- arrives from the control bot
```

### Show text in the user's language

```lua
local ru = ctx:lang() == "ru"
ctx:edit(ui.ok(ru and "Готово" or "Done"))
```

---

## Debugging and testing

- **Logs**: errors from your handlers appear in the chat with the module and handler name,
  in the console, on the panel's **Logs** page, and as a bot notification if errors are
  enabled. For quick debug output, send yourself a message:
  `ctx:send_saved("debug: " .. tostring(value))`.
- **Try code live**: `.eval` runs Lua with `ctx` available, which is handy for exploring
  the API. For example, `.eval ctx:runtime_stats()` or `.eval json.encode(ctx:message())`.
- **Inspect a message**: `.id` shows chat and message IDs, and `.eval ctx:message()` shows
  the full message table your `on_message` would receive.
- **Reload**: saving the file reloads it. `.module reload <name>` does the same
  explicitly. If loading fails, the previous version is gone, so check the log and fix
  the file.
- **Test in Saved Messages** first, so nothing you are debugging reaches other people.
- **Sandbox check**: modules you install from others run sandboxed. To see how your own
  module behaves there, give it a manifest with `"trusted": false` and only the
  permissions it needs.

---

## Common errors

| Error | Cause and fix |
|---|---|
| `module 'x' needs permission 'network'` | The module is sandboxed. Grant the permission in the panel (Modules → Permissions) or in its manifest |
| `module 'x' may not access protected key '...'` | Sandboxed modules cannot read credentials, panel or core settings, or other modules' `cfg.*`. Use your own key prefix |
| `attempt to call a nil value (method 'xyz')` | The method name is misspelled, or it does not exist. Check the API tables above |
| `attempt to concatenate a nil value` | A value you joined with `..` is `nil`, often a missing setting or reply. Use `tostring(x)` or a default: `(x or "")` |
| `attempt to call a nil value (global 'os')` | `os`, `io`, and `load` are unavailable in the sandbox |
| Command does nothing | Check that `M.commands` maps the command to the right function name, that the module is enabled (`.modules`), and that no other module uses the same command |
| `on_message` never runs | Sandboxed modules need `telegram.read`, and the module must be enabled |
| `HTTP response is larger than 256 KiB` | Use an API endpoint that returns less data, or `ctx:download_url` for files |

---

## Publishing to the marketplace

`.market` reads a JSON catalog from any URL (set it with `.market source <url>`):

```json
{
  "modules": [
    {
      "name": "todo",
      "description": "A tiny to-do list",
      "url": "https://raw.githubusercontent.com/you/fly-modules/main/todo.lua"
    }
  ]
}
```

To share a module:

1. Put the `.lua` file somewhere with a stable raw URL, such as a GitHub repository.
2. Add an entry to a catalog file, and share the catalog URL.
3. Users run `.market install todo`, or paste the module URL into the panel's
   **Install module** box.

Installed modules start sandboxed with no permissions. In your module's description, list
the permissions it needs and why, so users know what to grant.

---

## Example modules

Ready-to-copy modules live in [`examples/modules/`](examples/modules). Copy one into your
`modules/` folder to try it:

| File | Shows |
|---|---|
| `hello.lua` | Commands, help, settings, `ui` |
| `todo.lua` | Storing a table, subcommands, a configurable limit |
| `keyword_alert.lua` | `on_message` events and bot notifications |
| `crypto_price.lua` | HTTP and JSON, `pcall` error handling, select-type settings |

These examples are checked by the test suite, so they keep working as the API evolves.
