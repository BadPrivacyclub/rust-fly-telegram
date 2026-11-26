---@meta
-- Type definitions of the fly-telegram module API for the Lua language server (LuaLS).
-- Place this file and .luarc.json next to the modules; `fly-telegram types` writes both.

---@class FlyMessage
---@field id integer
---@field chat_id integer Chat id in the Bot API format
---@field text string
---@field date integer Unix time
---@field outgoing boolean
---@field from_self boolean
---@field mentioned boolean
---@field is_private boolean
---@field is_saved boolean
---@field is_group boolean
---@field is_command boolean Starts with a configured prefix
---@field has_media boolean
---@field reply_to_id? integer
---@field sender_id? integer
---@field sender_name? string
---@field sender_username? string
---@field sender_is_bot? boolean
---@field chat_title? string
---@field action? "join"|"leave"|"other"
---@field action_users? integer[]

---@class FlyKeptMessage
---@field id integer
---@field chat_id string
---@field sender_id? string
---@field text string

---@class FlyDeleteEvent
---@field chat_id? integer Known for channels and supergroups only
---@field ids integer[]
---@field messages? FlyKeptMessage[] What anti-delete kept; needs `telegram.deleted`

---@class FlyMembershipEvent
---@field chat_id integer
---@field user_ids integer[]
---@field by_link boolean

---@class FlyReactionCount
---@field emoji string Emoji, `custom:<id>`, or `paid`
---@field count integer

---@class FlyReactionEvent
---@field chat_id integer
---@field message_id integer
---@field reactions FlyReactionCount[]

---@class FlyFetchOptions
---@field method? string
---@field headers? table<string, string>
---@field query? table<string, string|number>
---@field body? string
---@field json? table
---@field timeout? integer Seconds, at most 120

---@class FlyFetchResult
---@field ok boolean
---@field status? integer
---@field reason? string
---@field url? string
---@field content_type? string
---@field text? string
---@field json? any
---@field error? string

---@class FlyHelp
---@field category? string
---@field description? string
---@field commands? table<string, string|{args?: string, desc?: string}>

---@class FlyConfigField
---@field key string
---@field type "string"|"number"|"bool"|"choice"
---@field default? any
---@field description? string
---@field secret? boolean
---@field required? boolean|string[]
---@field choices? string[]
---@field min? number
---@field max? number

---A module returns this table.
---@class FlyModule
---@field commands? table<string, string> Command name to handler name
---@field help? FlyHelp
---@field config? FlyConfigField[]
---@field every? table<string, string> Interval such as `10m` or `2h` to handler name
---@field at? table<string, string> Local time such as `09:30` to handler name
---@field on_message? fun(ctx: Ctx, msg: FlyMessage) Needs `telegram.read`
---@field on_edit? fun(ctx: Ctx, msg: FlyMessage) Needs `telegram.read`
---@field on_delete? fun(ctx: Ctx, ev: FlyDeleteEvent) Needs `telegram.read`
---@field on_join? fun(ctx: Ctx, ev: FlyMembershipEvent) Needs `telegram.read`
---@field on_leave? fun(ctx: Ctx, ev: FlyMembershipEvent) Needs `telegram.read`
---@field on_reaction? fun(ctx: Ctx, ev: FlyReactionEvent) Needs `telegram.read`
---@field on_load? fun()
---@field on_unload? fun()

---The context of a command, an event, or a scheduled call.
---@class Ctx
local Ctx = {}

---Replies to the current message.
---@param text string
function Ctx:reply(text) end

---Sends a message to the current chat without replying.
---@param text string
function Ctx:answer(text) end

---Edits the command message, or replies when it cannot be edited.
---@param text string
function Ctx:edit(text) end

---Deletes the current message.
function Ctx:delete() end

---The current message.
---@return FlyMessage?
function Ctx:message() end

---The message the current one replies to.
---@return FlyMessage?
function Ctx:replied() end

---Text of the current message.
---@return string
function Ctx:message_text() end

---Text of the replied message.
---@return string
function Ctx:replied_text() end

---Schedules a message in Telegram, 10 seconds to 365 days ahead.
---@param seconds integer
---@param text string
---@param target? "here"|"me"
function Ctx:schedule(seconds, text, target) end

---Sends to `"me"`, `"@username"`, or a chat id. Chats other than `me` need `telegram.send`.
---@param chat integer|string
---@param text string
function Ctx:send(chat, text) end

---Sends to Saved Messages.
---@param text string
function Ctx:send_saved(text) end

---Pins the replied message. Needs `telegram.history`.
function Ctx:pin() end

---Unpins the replied message. Needs `telegram.history`.
function Ctx:unpin() end

---Deletes messages from the replied one up to the command. Needs `telegram.history`.
---@return integer deleted
function Ctx:purge() end

---Mentions group members. Needs `telegram.history`.
---@param text? string
---@param limit? integer
---@return integer mentioned
function Ctx:tag_all(text, limit) end

---Reacts to the current message.
---@param emoji string
function Ctx:react(emoji) end

---Deletes the last own messages in the chat. Needs `telegram.history`.
---@param count integer
function Ctx:delete_last_own(count) end

---Details of the current or replied message. Needs `telegram.history`.
function Ctx:message_info() end

---A stored value. A sandboxed module sees only its own keys.
---@param key string
---@return any
function Ctx:db_get(key) end

---Stores a value; `nil` deletes the key. Sandboxed modules: at most 1000 keys, 256 KiB each.
---@param key string
---@param value any
function Ctx:db_set(key, value) end

---Sorted keys that start with `prefix`.
---@param prefix string
---@return string[]
function Ctx:db_keys(prefix) end

---A setting of this module from `M.config`.
---@param key string
---@return any
function Ctx:cfg(key) end

---Changes a setting of this module; `nil` resets it.
---@param key string
---@param value any
---@return any stored
function Ctx:cfg_set(key, value) end

---@return "en"|"ru"
function Ctx:lang() end

---The first command prefix.
---@return string
function Ctx:prefix() end

---Help text for everything, a module, or a command.
---@param query? string
---@return string
function Ctx:help(query) end

---Adds a notification to the panel inbox.
---@param text string
function Ctx:notify(text) end

---Name, version, permissions, and trust of this module.
---@return {name: string, version: string, permissions: string[], trusted: boolean}
function Ctx:module_info() end

---@return number
function Ctx:uptime_seconds() end

---@param text string
---@return string hex
function Ctx:sha256(text) end

---@return string
function Ctx:uuid() end

---@return integer
function Ctx:now_ms() end

---Version, uptime, CPU, memory, counters, modules, and accounts.
---@return table
function Ctx:runtime_stats() end

---Masks secrets such as tokens and API hashes in a text.
---@param text string
---@return string
function Ctx:sanitize(text) end

---Waits without holding the Lua state.
---@param seconds number
function Ctx:sleep(seconds) end

---@param millis integer
function Ctx:sleep_ms(millis) end

---Text of a URL. Needs `network`.
---@param url string
---@return string
function Ctx:http_get(url) end

---Decoded JSON of a URL. Needs `network`.
---@param url string
---@return any
function Ctx:http_json_get(url) end

---Needs `network`.
---@param method string
---@param url string
---@param body? string
---@param headers? table<string, string>
---@return string
function Ctx:http_request(method, url, body, headers) end

---Needs `network`.
---@param method string
---@param url string
---@param body? string
---@param headers? table<string, string>
---@return any
function Ctx:http_json_request(method, url, body, headers) end

---Uploads a file from `data/downloads/` as multipart. Needs `network` and `telegram.media`.
---@param method string
---@param url string
---@param file_field string
---@param path string
---@param fields? table<string, string>
---@param headers? table<string, string>
---@return any
function Ctx:http_json_multipart_file_request(method, url, file_field, path, fields, headers) end

---An HTTP request that never raises for network problems or statuses. Needs `network`.
---@param url string
---@param opts? FlyFetchOptions
---@return FlyFetchResult
function Ctx:fetch(url, opts) end

---Downloads the media of the replied message to `data/downloads/`. Needs `telegram.media`.
---@param name? string
---@return string path
function Ctx:download_replied_media(name) end

---Downloads a URL to `data/downloads/`. Needs `network` and `telegram.media`.
---@param url string
---@param name? string
---@return string path
function Ctx:download_url(url, name) end

---Sends a file from `data/downloads/`. Needs `telegram.media`.
---@param path string
---@param caption? string
function Ctx:send_file(path, caption) end

---An environment variable. Needs `secrets`.
---@param key string
---@return string?
function Ctx:env_get(key) end

---Runs a shell command and streams its output. Needs `shell`.
---@param command string
function Ctx:run_term(command) end

---Runs a program with arguments. Needs `shell`.
---@param program string
---@param args string[]
function Ctx:run_process(program, args) end

---Installs a module from a URL or path. Needs `modules.install`.
---@param source string
---@param name? string
function Ctx:install_module(source, name) end

---Installs the replied `.lua` file. Needs `modules.install`.
---@param name? string
function Ctx:install_replied_module(name) end

---Same as `install_module`. Needs `modules.install`.
---@param source string
---@param name? string
function Ctx:install_plugin(source, name) end

---Same as `install_replied_module`. Needs `modules.install`.
---@param name? string
function Ctx:install_replied_plugin(name) end

---Answers a permission request with `once`, `scope`, `always`, or `deny`. Needs `core.admin`.
---@param id integer
---@param choice "once"|"scope"|"always"|"deny"
---@return string outcome
function Ctx:perm_decide(id, choice) end

---Needs `core.admin`.
---@return table
function Ctx:premium_status() end

---Needs `core.admin`.
---@param what string
---@param value? string
---@param extra? string
function Ctx:premium_set(what, value, extra) end

---Needs `core.admin`.
---@return table[]
function Ctx:modules_list() end

---Needs `core.admin`.
---@param name string
---@param enabled boolean
function Ctx:module_set_enabled(name, enabled) end

---Needs `core.admin`.
---@param name string
function Ctx:module_reload(name) end

---Needs `core.admin`.
---@param name string
function Ctx:module_remove(name) end

---Needs `core.admin`.
---@param name? string
function Ctx:module_restore(name) end

---Needs `core.admin`.
---@param name string
---@return table
function Ctx:module_config(name) end

---Needs `core.admin`.
---@param name string
---@param key string
---@param value? string
function Ctx:module_config_set(name, key, value) end

---Needs `core.admin`.
---@return string[]
function Ctx:prefixes() end

---Needs `core.admin`.
---@param values string[]
function Ctx:set_prefixes(values) end

---Needs `core.admin`.
---@param code "en"|"ru"
function Ctx:set_lang(code) end

---Needs `core.admin`.
---@return integer[]
function Ctx:sudo_list() end

---Needs `core.admin`.
---@param users integer[]
function Ctx:sudo_set(users) end

---A one-time login link for the panel. Needs `core.admin`.
---@return string
function Ctx:panel_link() end

---Needs `core.admin`.
function Ctx:restart() end

---Needs `core.admin`.
---@return table
function Ctx:check_update() end

---Needs `core.admin`.
function Ctx:self_update() end

---Needs `core.admin`.
function Ctx:update_project() end

---Sends a backup to Saved Messages. Needs `core.admin`.
function Ctx:backup() end

---Restores the replied backup. Needs `core.admin`.
function Ctx:restore_backup() end

---The account of this call. Needs `core.admin`.
---@return table
function Ctx:account() end

---Needs `core.admin`.
---@return table
function Ctx:account_counts() end

---Needs `core.admin`.
---@param options table
---@return table
function Ctx:cleanup_preview(options) end

---Needs `core.admin`.
---@param code string
function Ctx:cleanup_run(code) end

---Needs `core.admin`.
---@param action string
---@param days? integer
function Ctx:account_bulk(action, days) end

---Needs `core.admin`.
---@return string[]
function Ctx:persona_kinds() end

---Needs `core.admin`.
---@return table
function Ctx:persona_check() end

---Needs `core.admin`.
---@param kind string
---@return table
function Ctx:persona_apply(kind) end

---Needs `core.admin`.
---@return table
function Ctx:persona_restore() end

---Needs `core.admin`.
---@return table
function Ctx:persona_status() end

---Needs `core.admin`.
---@return table[]
function Ctx:jobs() end

---Needs `core.admin`.
---@param id string
function Ctx:job_cancel(id) end

---Needs `core.admin`.
---@param update table
---@return table
function Ctx:profile_update(update) end

---Needs `core.admin`.
---@return table[]
function Ctx:auto_rules() end

---Needs `core.admin`.
---@param rule table
---@return table
function Ctx:auto_save(rule) end

---Needs `core.admin`.
---@param id string
---@param enabled boolean
function Ctx:auto_toggle(id, enabled) end

---Needs `core.admin`.
---@param id string
function Ctx:auto_delete(id) end

---Message formatting helpers.
---@class FlyUi
---@field icons table<string, string> Icons, premium emoji where available
---@field plain_icons table<string, string>
ui = {}

---@param id string|integer Premium emoji id
---@param fallback string
---@return string
function ui.emoji(id, fallback) end

---@param name string
---@return string
function ui.icon(name) end

---@param value any
---@return string
function ui.mono(value) end

---@param value any
---@return string
function ui.bold(value) end

---@param value any
---@return string
function ui.italic(value) end

---@param text string
---@param lang? string
---@return string
function ui.code(text, lang) end

---@param lines string[]
---@return string
function ui.join(lines) end

---@param icon string
---@param text string
---@return string
function ui.title(icon, text) end

---@param icon string
---@param title string
---@param lines string[]
---@param footer? string
---@return string
function ui.card(icon, title, lines, footer) end

---@param label string
---@param value any
---@return string
function ui.kv(label, value) end

---@param enabled boolean
---@param on_text? string
---@param off_text? string
---@return string
function ui.status(enabled, on_text, off_text) end

---@param title string
---@param enabled boolean
---@param detail? string
---@return string
function ui.toggle(title, enabled, detail) end

---@param title string
---@param detail? string
---@return string
function ui.ok(title, detail) end

---@param title string
---@param detail? string
---@return string
function ui.err(title, detail) end

---@param title string
---@param detail? string
---@return string
function ui.warn(title, detail) end

---@param title string
---@param detail? string
---@return string
function ui.info(title, detail) end

---@param title string
---@return string
function ui.wait(title) end

---@param prefix string
---@param command string
---@param variants string[]
---@param description? string
---@return string
function ui.usage(prefix, command, variants, description) end

---@param items string[]
---@param bullet? string
---@return string
function ui.list(items, bullet) end

---@param title string
---@param lines string[]
---@return string
function ui.section(title, lines) end

---@param fraction number
---@param width? integer
---@return string
function ui.bar(fraction, width) end

---@param seconds number
---@return string
function ui.duration(seconds) end

---@param value number
---@return string
function ui.bytes(value) end

---Seconds in a text such as `2h30m`.
---@param text string
---@return integer?
function ui.parse_duration(text) end

---@param user_id integer
---@param name string
---@return string
function ui.mention(user_id, name) end

---The first word and the rest.
---@param args string
---@return string first, string rest
function ui.split(args) end

---@param err any
---@return string
function ui.error_text(err) end

---Calls `fn` and returns its results, or `nil` and an error text.
---@param fn function
---@return any ...
function ui.try(fn, ...) end

---Words of an argument string.
---@param args string
---@return string[]
function ui.args(args) end

---@param value string
---@return string
function ui.trim(value) end

---A nested value by a dotted path.
---@param value table
---@param path string
---@param default? any
---@return any
function ui.get(value, path, default) end

---The first argument that is not `nil` or empty.
---@return any
function ui.first(...) end

---@param value number
---@param decimals? integer
---@return string
function ui.num(value, decimals) end

---@param value string
---@return string
function ui.urlencode(value) end

---@param params table<string, string|number>
---@return string
function ui.query(params) end

---@param text string
---@param max integer
---@return string
function ui.clip(text, max) end

---JSON helpers; `null` decodes to `nil`.
json = {}

---@param value any
---@return string
function json.encode(value) end

---@param text string
---@return any
function json.decode(text) end
