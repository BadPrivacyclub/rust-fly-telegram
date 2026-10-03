-- Shared message formatting for fly-telegram modules, available as the global `ui`.
-- Messages are Telegram markdown; lines inside a block are joined with hard breaks.

local ui = {}

ui.icons = {
    ok = "✅", err = "❌", warn = "⚠️", info = "ℹ️", wait = "⏳",
    on = "🟢", off = "⚪", dot = "▸", gear = "⚙️", module = "🧩",
    lock = "🔒", clock = "⏰", chart = "📊", note = "📝", user = "👤",
}

local BR = "  \n"

-- Inline code cannot contain backticks.
function ui.mono(value)
    return "`" .. tostring(value == nil and "—" or value):gsub("`", "'") .. "`"
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

-- A titled block: ui.card("📝", "Notes", { "line", ui.kv("Count", 3) })
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

-- ui.usage(ctx:prefix(), "note", { "set <text>", "get", "clear" }, "Saved notes")
function ui.usage(prefix, command, variants, description)
    local lines = {}
    for _, variant in ipairs(variants or { "" }) do
        local suffix = variant ~= "" and (" " .. variant) or ""
        table.insert(lines, ui.mono(prefix .. command .. suffix))
    end
    if description and description ~= "" then
        table.insert(lines, 1, ui.italic(description))
    end
    return ui.card("📖", "Usage", lines)
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
    if not value then return "—" end
    local units = { "B", "KB", "MB", "GB" }
    local index = 1
    while value >= 1024 and index < #units do
        value = value / 1024
        index = index + 1
    end
    if index == 1 then return string.format("%d %s", value, units[index]) end
    return string.format("%.1f %s", value, units[index])
end

-- Parses "10m", "2h30m", "1d", "45s", "90" (seconds) into seconds; nil when invalid.
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

-- Mention link for a user ID: renders as a clickable name.
function ui.mention(user_id, name)
    local id = math.tointeger(tonumber(user_id))
    if not id then return tostring(name or user_id) end
    local label = tostring(name or id):gsub("[%[%]]", "")
    return "[" .. label .. "](tg://user?id=" .. id .. ")"
end

-- Splits arguments: first word and the rest.
function ui.split(args)
    local head, rest = tostring(args or ""):match("^(%S+)%s*(.*)$")
    return head or "", rest or ""
end

return ui
