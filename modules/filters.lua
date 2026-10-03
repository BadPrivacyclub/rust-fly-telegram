local M = {}

M.meta = { name = "filters", version = "1.0" }

M.commands = {
    filter = "filter_cmd",
    unfilter = "unfilter_cmd",
    filters = "filters_cmd",
}

M.help = {
    category = "automation",
    description = "Automatic replies to keywords, per chat",
    commands = {
        filter = { args = "<trigger> | <reply>", desc = "Add a filter; reply to a message to use its text. Prefix the trigger with = for an exact match" },
        unfilter = { args = "<trigger>", desc = "Remove a filter" },
        filters = "List filters in this chat",
    },
}

M.config = {
    { key = "cooldown", type = "number", default = 15, description = "Seconds before the same filter can fire again in a chat" },
    { key = "reply_to_self", type = "bool", default = false, description = "Also react to your own messages" },
}

local last_fired = {}

local function key_for(chat_id)
    return "filters." .. tostring(chat_id)
end

local function load(ctx, chat_id)
    local value = ctx:db_get(key_for(chat_id))
    if type(value) ~= "table" then
        return {}
    end
    return value
end

local function count(tbl)
    local n = 0
    for _ in pairs(tbl) do n = n + 1 end
    return n
end

function M.filter_cmd(ctx, args)
    local msg = ctx:message()
    local trigger, reply = args:match("^(.-)%s*|%s*(.+)$")
    if not trigger then
        trigger = args
        local replied = ctx:replied()
        reply = replied and replied.text or ""
    end
    trigger = (trigger or ""):lower():gsub("^%s+", ""):gsub("%s+$", "")
    if trigger == "" or reply == "" then
        ctx:edit(ui.usage(ctx:prefix(), "filter", { "hello | Hi there!", "=price | See pinned message" },
            "Or reply to a message with " .. ctx:prefix() .. "filter <trigger>"))
        return
    end
    local filters = load(ctx, msg.chat_id)
    filters[trigger] = reply
    ctx:db_set(key_for(msg.chat_id), filters)
    ctx:edit(ui.ok("Filter saved", ui.mono(trigger) .. " · " .. count(filters) .. " in this chat"))
end

function M.unfilter_cmd(ctx, args)
    local msg = ctx:message()
    local trigger = args:lower()
    local filters = load(ctx, msg.chat_id)
    if filters[trigger] == nil then
        ctx:edit(ui.warn("No such filter", ui.mono(trigger)))
        return
    end
    filters[trigger] = nil
    if count(filters) == 0 then
        ctx:db_set(key_for(msg.chat_id), nil)
    else
        ctx:db_set(key_for(msg.chat_id), filters)
    end
    ctx:edit(ui.ok("Filter removed", ui.mono(trigger)))
end

function M.filters_cmd(ctx, args)
    local msg = ctx:message()
    local filters = load(ctx, msg.chat_id)
    local triggers = {}
    for trigger in pairs(filters) do
        table.insert(triggers, trigger)
    end
    table.sort(triggers)
    local lines = {}
    for _, trigger in ipairs(triggers) do
        local reply = tostring(filters[trigger]):gsub("\n", " ")
        if #reply > 60 then reply = reply:sub(1, 60) .. "…" end
        table.insert(lines, ui.mono(trigger) .. " → " .. reply)
    end
    if #lines == 0 then
        lines = { ui.italic("No filters in this chat") }
    end
    ctx:edit(ui.card("🔁", "Filters", lines))
end

local function matches(trigger, text)
    if trigger:sub(1, 1) == "=" then
        return text == trigger:sub(2)
    end
    -- Whole-word containment so "hi" does not fire on "this".
    local escaped = trigger:gsub("([%^%$%(%)%%%.%[%]%*%+%-%?])", "%%%1")
    return (" " .. text .. " "):find("[^%w]" .. escaped .. "[^%w]") ~= nil
end

function M.on_message(ctx, msg)
    if msg.is_command or msg.text == "" then
        return
    end
    if msg.from_self and not ctx:cfg("reply_to_self") then
        return
    end
    local filters = load(ctx, msg.chat_id)
    if next(filters) == nil then
        return
    end
    local text = msg.text:lower()
    local now = ctx:now_ms() // 1000
    local cooldown = tonumber(ctx:cfg("cooldown")) or 15
    for trigger, reply in pairs(filters) do
        if matches(trigger, text) then
            local fired_key = tostring(msg.chat_id) .. ":" .. trigger
            if (last_fired[fired_key] or 0) + cooldown <= now then
                last_fired[fired_key] = now
                ctx:answer(tostring(reply))
            end
            return
        end
    end
end

return M
