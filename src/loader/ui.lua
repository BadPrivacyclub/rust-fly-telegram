-- Shared message formatting for fly-telegram modules; I am available as the global `ui`.
-- Messages are Telegram markdown, and I join lines inside a block with hard breaks.

local ui = {}

-- I keep plain icons here. With a premium emoji pack configured, ui.icons returns custom emoji
-- instead, and accounts without Premium still see these.
ui.plain_icons = {
    ok = "✅", err = "❌", warn = "⚠️", info = "ℹ️", wait = "⏳",
    on = "🟢", off = "⚪", dot = "▸", gear = "⚙️", module = "🧩",
    lock = "🔒", clock = "⏰", chart = "📊", note = "📝", user = "👤",
    usage = "📖", search = "🔍", trash = "🗑", star = "⭐", fire = "🔥", link = "🔗",
}

-- I get this from the host: icon name to custom emoji document id (as a string).
ui.__premium = {}

-- I build a custom emoji by document id; `fallback` is what accounts without Premium see.
function ui.emoji(id, fallback)
    fallback = tostring(fallback or "⭐"):gsub("[%[%]]", "")
    local digits = tostring(id or ""):match("^%d+$")
    if not digits then
        return fallback
    end
    return "[" .. fallback .. "](tg://emoji?id=" .. digits .. ")"
end

ui.icons = setmetatable({}, {
    __index = function(_, name)
        local plain = ui.plain_icons[name]
        local id = ui.__premium[name]
        if plain and id then
            return ui.emoji(id, plain)
        end
        return plain
    end,
    __pairs = function()
        return function(_, key)
            local name = next(ui.plain_icons, key)
            if name == nil then
                return nil
            end
            return name, ui.icons[name]
        end, nil, nil
    end,
})

function ui.icon(name)
    return ui.icons[name] or ""
end

local BR = "  \n"

-- Inline code cannot contain backticks, so I swap them for quotes.
function ui.mono(value)
    return "`" .. tostring(value == nil and "-" or value):gsub("`", "'") .. "`"
end

function ui.bold(value)
    return "**" .. tostring(value):gsub("%*%*", "*") .. "**"
end

function ui.italic(value)
    return "__" .. tostring(value):gsub("__", "_") .. "__"
end

function ui.code(text, lang)
    text = tostring(text == nil and "" or text):gsub("```", "`\u{200b}``")
    return "```" .. (lang or "text") .. "\n" .. text .. "\n```"
end

function ui.join(lines)
    local out = {}
    for _, line in ipairs(lines or {}) do
        if line ~= nil and line ~= false then
            table.insert(out, tostring(line))
        end
    end
    return table.concat(out, BR)
end

function ui.title(icon, text)
    return (icon and (icon .. " ") or "") .. ui.bold(text)
end

-- I build a titled block: ui.card(ui.icons.note, "Notes", { "line", ui.kv("Count", 3) })
function ui.card(icon, title, lines, footer)
    local parts = { ui.title(icon, title) }
    if lines and #lines > 0 then
        table.insert(parts, ui.join(lines))
    end
    local text = table.concat(parts, "\n")
    if footer and footer ~= "" then
        text = text .. "\n\n" .. ui.italic(footer)
    end
    return text
end

function ui.kv(label, value)
    return ui.icons.dot .. " " .. tostring(label) .. ": " .. ui.mono(value)
end

function ui.status(enabled, on_text, off_text)
    if enabled then
        return ui.icons.on .. " " .. (on_text or "on")
    end
    return ui.icons.off .. " " .. (off_text or "off")
end

function ui.toggle(title, enabled, detail)
    return ui.card(enabled and ui.icons.on or ui.icons.off, title, {
        "Status: " .. ui.mono(enabled and "enabled" or "disabled"),
        detail,
    })
end

function ui.ok(title, detail)
    return ui.card(ui.icons.ok, title, detail and { detail } or nil)
end

function ui.err(title, detail)
    return ui.card(ui.icons.err, title, detail and { detail } or nil)
end

function ui.warn(title, detail)
    return ui.card(ui.icons.warn, title, detail and { detail } or nil)
end

function ui.info(title, detail)
    return ui.card(ui.icons.info, title, detail and { detail } or nil)
end

function ui.wait(title)
    return ui.icons.wait .. " " .. ui.italic(title or "Working…")
end

-- I format usage lines: ui.usage(ctx:prefix(), "note", { "set <text>", "get", "clear" }, "Saved notes")
function ui.usage(prefix, command, variants, description)
    local lines = {}
    for _, variant in ipairs(variants or { "" }) do
        local suffix = variant ~= "" and (" " .. variant) or ""
        table.insert(lines, ui.mono(prefix .. command .. suffix))
    end
    if description and description ~= "" then
        table.insert(lines, 1, ui.italic(description))
    end
    return ui.card(ui.icons.usage, "Usage", lines)
end

function ui.list(items, bullet)
    local lines = {}
    for _, item in ipairs(items or {}) do
        table.insert(lines, (bullet or "•") .. " " .. tostring(item))
    end
    return ui.join(lines)
end

function ui.section(title, lines)
    return ui.bold(title) .. "\n" .. ui.join(lines)
end

function ui.bar(fraction, width)
    width = width or 10
    fraction = math.max(0, math.min(1, tonumber(fraction) or 0))
    local filled = math.floor(fraction * width + 0.5)
    return string.rep("▰", filled) .. string.rep("▱", width - filled)
end

function ui.duration(seconds)
    seconds = math.max(0, math.floor(tonumber(seconds) or 0))
    local d = seconds // 86400
    local h = (seconds % 86400) // 3600
    local m = (seconds % 3600) // 60
    local s = seconds % 60
    if d > 0 then return string.format("%dd %dh %dm", d, h, m) end
    if h > 0 then return string.format("%dh %dm", h, m) end
    if m > 0 then return string.format("%dm %ds", m, s) end
    return string.format("%ds", s)
end

function ui.bytes(value)
    value = tonumber(value)
    if not value then return "-" end
    local units = { "B", "KB", "MB", "GB" }
    local index = 1
    while value >= 1024 and index < #units do
        value = value / 1024
        index = index + 1
    end
    if index == 1 then return string.format("%d %s", value, units[index]) end
    return string.format("%.1f %s", value, units[index])
end

-- I parse "10m", "2h30m", "1d", "45s", "90" (seconds) into seconds, or return nil when invalid.
function ui.parse_duration(text)
    text = tostring(text or ""):lower()
    if text:match("^%d+$") then return tonumber(text) end
    local total, matched = 0, false
    local units = { s = 1, m = 60, h = 3600, d = 86400, w = 604800 }
    local rest = text:gsub("(%d+)([smhdw])", function(n, unit)
        total = total + tonumber(n) * units[unit]
        matched = true
        return ""
    end)
    if not matched or rest ~= "" then return nil end
    return total
end

-- I build a mention link for a user ID that renders as a clickable name.
function ui.mention(user_id, name)
    local id = math.tointeger(tonumber(user_id))
    if not id then return tostring(name or user_id) end
    local label = tostring(name or id):gsub("[%[%]]", "")
    return "[" .. label .. "](tg://user?id=" .. id .. ")"
end

-- I split arguments into the first word and the rest.
function ui.split(args)
    local head, rest = tostring(args or ""):match("^(%S+)%s*(.*)$")
    return head or "", rest or ""
end

-- I make a readable message from a caught error: no stack traceback, no "runtime error:" or
-- chunk:line prefixes. Use it with pcall: ctx:edit(ui.err("Failed", ui.error_text(e)))
function ui.error_text(err)
    local text = tostring(err == nil and "unknown error" or err)
    text = text:gsub("\nstack traceback:.*$", "")
    local changed = true
    while changed do
        changed = false
        for _, pattern in ipairs({ "^%s*runtime error:%s*", "^%s*callback error%s*", "^%s*caused by:%s*", '^%[string "[^"]*"%]:%d+:%s*', "^[%w_%-%./\\]+:%d+:%d+:%s*", "^[%w_%-%./\\]+%.lua:%d+:%s*", "^[%w_%-]+:%d+:%s*" }) do
            local stripped, count = text:gsub(pattern, "")
            if count > 0 then
                text, changed = stripped, true
            end
        end
    end
    text = text:gsub("^%s+", ""):gsub("%s+$", "")
    if text == "" then text = "unknown error" end
    return text
end

-- I call fn(...) and return its results, or nil and a clean error message.
-- local data, err = ui.try(ctx.http_json_get, ctx, url)
function ui.try(fn, ...)
    local result = table.pack(pcall(fn, ...))
    if not result[1] then
        return nil, ui.error_text(result[2])
    end
    return table.unpack(result, 2, result.n)
end

-- I split an argument string into words; "double quotes" keep spaces: ui.args('a "b c"') -> { "a", "b c" }
function ui.args(args)
    local out = {}
    local text = tostring(args or "")
    local pos = 1
    while pos <= #text do
        local s, e = text:find("^%s+", pos)
        if s then pos = e + 1 end
        if pos > #text then break end
        if text:sub(pos, pos) == '"' then
            local close = text:find('"', pos + 1, true)
            if close then
                table.insert(out, text:sub(pos + 1, close - 1))
                pos = close + 1
            else
                table.insert(out, text:sub(pos + 1))
                break
            end
        else
            local word_end = (text:find("%s", pos) or (#text + 1)) - 1
            table.insert(out, text:sub(pos, word_end))
            pos = word_end + 1
        end
    end
    return out
end

function ui.trim(value)
    return (tostring(value or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

-- I read safely deep inside decoded JSON: ui.get(data, "current.temp", "weather.1.date").
-- Numeric path parts index arrays (1-based). I return nil instead of raising.
function ui.get(value, path, default)
    local current = value
    for part in tostring(path or ""):gmatch("[^%.]+") do
        if type(current) ~= "table" then
            return default
        end
        local key = math.tointeger(tonumber(part))
        local next_value = key and current[key]
        if next_value == nil then
            next_value = current[part]
        end
        current = next_value
    end
    if current == nil then
        return default
    end
    return current
end

-- I return the first argument that is not nil, false, or an empty string.
function ui.first(...)
    for index = 1, select("#", ...) do
        local value = select(index, ...)
        if value ~= nil and value ~= false and value ~= "" then
            return value
        end
    end
    return nil
end

-- I format a number with up to `decimals` digits, or "-" for anything that is not a number.
function ui.num(value, decimals)
    local number = tonumber(value)
    if not number then return "-" end
    local text = string.format("%." .. tostring(decimals or 1) .. "f", number)
    text = text:gsub("(%..-)0+$", "%1"):gsub("%.$", "")
    if text == "-0" then text = "0" end
    return text
end

function ui.urlencode(value)
    return (tostring(value == nil and "" or value):gsub("([^%w%-%._~])", function(ch)
        return string.format("%%%02X", string.byte(ch))
    end))
end

-- I build a sorted query string: ui.query({ q = "x y", n = 2 }) -> "n=2&q=x%20y"
function ui.query(params)
    local keys = {}
    for key in pairs(params or {}) do
        table.insert(keys, tostring(key))
    end
    table.sort(keys)
    local parts = {}
    for _, key in ipairs(keys) do
        local value = params[key]
        if value ~= nil and value ~= false then
            table.insert(parts, ui.urlencode(key) .. "=" .. ui.urlencode(value))
        end
    end
    return table.concat(parts, "&")
end

-- I clip text to `max` characters (UTF-8 aware) with an ellipsis.
function ui.clip(text, max)
    text = tostring(text or "")
    max = max or 200
    if utf8.len(text) == nil or utf8.len(text) <= max then
        return text
    end
    return text:sub(1, utf8.offset(text, max + 1) - 1) .. "…"
end

return ui
