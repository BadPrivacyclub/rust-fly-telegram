# Module authoring guide

fly-telegram modules are plain Lua 5.4 scripts. Place them in the `modules/` directory. They are loaded at startup and **hot-reloaded automatically** when the file is saved.

---

## Minimal module

```lua
local M = {}

M.meta = { name = "hello", version = "1.0" }

M.commands = {
    hello = "hello_cmd",
}

function M.hello_cmd(ctx, args)
    ctx:edit("👋 Hello, " .. (args ~= "" and args or "world") .. "!")
end

return M
```

Save the file to `modules/hello.lua`. The bot reloads it automatically. Send `.hello` in any chat (as yourself) to test it.

---

## Module structure

Every module file must return a table with the following fields:

```lua
local M = {}

M.meta = {
    name    = "my_module",
    version = "1.0",
}

M.commands = {
    cmd      = "cmd_handler",
    alias    = "cmd_handler",
    another  = "another_handler",
}

function M.cmd_handler(ctx, args)
end

function M.another_handler(ctx, args)
end

return M
```

---

## The ctx object

Every handler receives `ctx` as its first argument. The following methods are available:

### Telegram actions

| Method | Description |
|---|---|
| `ctx:edit(text)` | Edit the command message with `text` |
| `ctx:reply(text)` | Send a new message in the same chat |
| `ctx:delete()` | Delete the command message |
| `ctx:install_module(source, name)` | Install a `.lua` module from a file or URL |
| `ctx:download_replied_media(name)` | Save media from the replied message under `data/downloads/` |
| `ctx:download_url(url, name)` | Download an HTTP(S) URL under `data/downloads/` |
| `ctx:send_file(path, caption)` | Upload and send a relative local file path |

These methods are **async**. Lua awaits them automatically inside a handler.

### Database

| Method | Description |
|---|---|
| `ctx:db_get(key)` | Returns the stored value for `key`, or `nil` |
| `ctx:db_set(key, value)` | Stores `value` under `key` and flushes to disk |

Supported value types for `db_set`: `string`, `number` (integer), `boolean`.

### Network, secrets, and runtime

| Method | Description |
|---|---|
| `ctx:http_get(url)` | Fetches a small text response |
| `ctx:http_json_get(url)` | Fetches and decodes JSON |
| `ctx:http_request(method, url, body, headers)` | Sends a small HTTP request |
| `ctx:http_json_request(method, url, body, headers)` | Sends an HTTP request and decodes JSON |
| `ctx:http_json_multipart_file_request(method, url, file_field, path, fields, headers)` | Uploads a relative local file as multipart form data and decodes JSON |
| `ctx:env_get(name)` | Reads an environment variable |
| `ctx:message_text()` | Returns the current message text |
| `ctx:replied_text()` | Returns replied message text, if any |
| `ctx:sleep_ms(ms)` | Sleeps for milliseconds |

Network responses are size-limited. URL/file helpers reject absolute paths and
`..` path components.

### Module manifests and permissions

Public modules should ship a sibling manifest:

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

Supported permissions include `network`, `shell`, `telegram.media`,
`telegram.history`, `modules.install`, and `secrets`. Built-in modules are
trusted; installed public modules default to `trusted: false` with no granted
permissions until reviewed.

### Installing modules from Lua

The built-in installer module exposes:

```text
.install <file-or-url> [name]
```

Internally it calls `ctx:install_module(source, name)`. `source` may be a local
file path or an `http(s)` URL. `name` is optional and must be a plain file name;
`.lua` is appended automatically when missing. The file is written into
`modules/`, then the watcher hot-loads it.

### Example using the database

```lua
M.commands = { count = "count_cmd" }

function M.count_cmd(ctx, args)
    local n = ctx:db_get("counter") or 0
    n = n + 1
    ctx:db_set("counter", n)
    ctx:edit("Count: " .. tostring(n))
end
```

---

## Command arguments

The `args` parameter is a plain string containing everything the user typed after the command name, trimmed of leading and trailing spaces.

```
User sends:   .greet Alice Bob
args value:   "Alice Bob"
```

Parse it however you like:

```lua
function M.greet_cmd(ctx, args)
    if args == "" then
        ctx:edit("Usage: .greet <name>")
        return
    end
    local name = args:match("^(%S+)")
    ctx:edit("Hello, " .. name .. "!")
end
```

---

## Error handling

Wrap risky code in `pcall` to prevent the handler from crashing the module:

```lua
function M.safe_cmd(ctx, args)
    local ok, err = pcall(function()
        error("oops")
    end)
    if not ok then
        ctx:edit("❌ Error: " .. tostring(err))
    end
end
```

Unhandled Lua errors are caught by the loader, logged to the console, and do not affect other modules.

---

## Multiple commands in one module

```lua
local M = {}

M.meta = { name = "math", version = "1.0" }

M.commands = {
    add = "add_cmd",
    mul = "mul_cmd",
}

function M.add_cmd(ctx, args)
    local a, b = args:match("(-?%d+)%s+(-?%d+)")
    if not a then ctx:edit("Usage: .add <a> <b>") return end
    ctx:edit(tostring(tonumber(a) + tonumber(b)))
end

function M.mul_cmd(ctx, args)
    local a, b = args:match("(-?%d+)%s+(-?%d+)")
    if not a then ctx:edit("Usage: .mul <a> <b>") return end
    ctx:edit(tostring(tonumber(a) * tonumber(b)))
end

return M
```

---

## Module state

Variables declared as `local` at the top level of the module file persist for the lifetime of the module (until it is reloaded). Use them for caching or counters that do not need to survive a restart.

```lua
local M = {}
local call_count = 0   -- lives in memory, reset on reload

M.meta    = { name = "counter", version = "1.0" }
M.commands = { calls = "calls_cmd" }

function M.calls_cmd(ctx, args)
    call_count = call_count + 1
    ctx:edit("Called " .. call_count .. " times since last reload.")
end

return M
```

For persistent state that survives restarts, use `ctx:db_set` / `ctx:db_get` instead.

---

## Hot-reload

The file watcher monitors the `modules/` directory. When you save a `.lua` file:

1. The old version of the module is unloaded.
2. The new file is executed.
3. The module is registered with its updated command table.

There is no need to restart the process. If the new file has a syntax error, the old module stays unloaded and the error is printed to the log. Fix the file and save again.

---

## Activating a module

1. Place the `.lua` file in the `modules/` directory.
2. The bot loads it automatically (at startup or when the file is created/modified).
3. Send the command defined in `M.commands` from your Telegram account.

To **disable** a module without deleting it, rename the file so it no longer ends in `.lua` (e.g. `mymodule.lua.disabled`).

---

## Lua standard library

The full Lua 5.4 standard library is available inside modules, including:

- `string`, `table`, `math`, `io`, `os`, `utf8`
- `pcall`, `xpcall`, `error`, `load`, `loadfile`
- `require`: can load other Lua files relative to `modules/`

> **Note:** `io.popen` and `os.execute` run shell commands as the user that launched the bot. Use them carefully.

---

## Example: full module

```lua
local M = {}

M.meta     = { name = "note", version = "1.0" }
M.commands = { note = "note_cmd" }

function M.note_cmd(ctx, args)
    local sub, rest = args:match("^(%S+)%s*(.*)")

    if sub == "save" then
        if rest == "" then
            ctx:edit("Usage: .note save <text>")
            return
        end
        ctx:db_set("note_value", rest)
        ctx:edit("✅ Note saved.")

    elseif sub == "get" then
        local value = ctx:db_get("note_value")
        if value == nil then
            ctx:edit("No note saved yet. Use .note save <text>.")
        else
            ctx:edit("📝 " .. tostring(value))
        end

    else
        ctx:edit("Subcommands: save <text> | get")
    end
end

return M
```
