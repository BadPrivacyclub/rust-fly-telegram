local M = {}

M.commands = {
    auto = "auto_cmd",
    away = "away_cmd",
}

M.help = {
    category = "automation",
    description = "Automation rules: schedules, away replies, profile clock, presence (full editor in the panel)",
    commands = {
        auto = { args = "[on|off|del <id>] | every <min> <chat> <text> | daily <HH:MM> <chat> <text> | clock <name|bio> <template> | online | offline | presence off", desc = "List and manage rules" },
        away = { args = "<HH:MM-HH:MM> <text> | off", desc = "Auto-reply to private messages during these hours" },
    },
}

M.config = {
    { key = "utc_offset", type = "number", default = 0, description = "Your UTC offset in hours for rule times" },
}

local TYPE_ICONS = {
    schedule = "🗓", away_reply = "🌙", forward = "↪️", auto_delete = "🧨",
    profile_clock = "🕐", bio_rotation = "🔁", presence = "🟢", auto_read = "👀",
}

local function offset(ctx)
    return math.floor((tonumber(ctx:cfg("utc_offset")) or 0) * 60)
end

local function describe(rule)
    local t = rule.type
    if t == "schedule" then
        local when = rule.every_minutes and ("every " .. rule.every_minutes .. " min") or ("daily " .. tostring(rule.at))
        return when .. " → " .. rule.chat
    elseif t == "away_reply" then
        return rule.from .. "–" .. rule.to
    elseif t == "profile_clock" then
        return rule.field .. ": " .. rule.template
    elseif t == "presence" then
        return rule.online and "always online" or "always offline"
    elseif t == "forward" then
        return tostring(rule.from_chat) .. " → " .. rule.to_chat
    elseif t == "auto_delete" then
        return "after " .. rule.after_minutes .. " min"
    end
    return t:gsub("_", " ")
end

local function save(ctx, rule, title)
    rule.utc_offset_minutes = rule.utc_offset_minutes or offset(ctx)
    local ok, saved = pcall(function() return ctx:auto_save(rule) end)
    if ok then
        ctx:edit(ui.ok(title, ui.mono(saved.id) .. " · " .. describe(saved)))
    else
        ctx:edit(ui.err("Rule not saved", tostring(saved)))
    end
end

local function list(ctx)
    local rules = ctx:auto_rules()
    if #rules == 0 then
        ctx:edit(ui.info("No automation rules yet", "Try " .. ui.mono(ctx:prefix() .. "away 23:00-08:00 I'm asleep") .. " or open the Automations page in the panel."))
        return
    end
    local lines = {}
    for _, rule in ipairs(rules) do
        table.insert(lines, string.format("%s %s %s %s · %s",
            rule.enabled and ui.icons.on or ui.icons.off,
            TYPE_ICONS[rule.type] or "•",
            ui.bold(rule.name),
            ui.mono(rule.id),
            describe(rule)))
    end
    ctx:edit(ui.card("⚙️", "Automations", lines, ctx:prefix() .. "auto on|off|del <id>"))
end

function M.auto_cmd(ctx, args)
    local action, rest = ui.split(args)
    if action == "" then
        return list(ctx)
    end
    if action == "on" or action == "off" then
        local ok, err = pcall(function() ctx:auto_toggle(rest, action == "on") end)
        ctx:edit(ok and ui.toggle("Rule " .. rest, action == "on") or ui.err("Not changed", tostring(err)))
    elseif action == "del" then
        local ok, err = pcall(function() ctx:auto_delete(rest) end)
        ctx:edit(ok and ui.ok("Rule deleted", ui.mono(rest)) or ui.err("Not deleted", tostring(err)))
    elseif action == "every" then
        local minutes, chat, text = rest:match("^(%d+)%s+(%S+)%s+(.+)$")
        if not minutes then
            return ctx:edit(ui.usage(ctx:prefix(), "auto", { "every 60 me Drink water 💧", "every 1440 @channel Good morning" }))
        end
        save(ctx, { type = "schedule", name = "Every " .. minutes .. " min", chat = chat, text = text, every_minutes = tonumber(minutes) }, "Schedule saved")
    elseif action == "daily" then
        local at, chat, text = rest:match("^(%d%d?:%d%d)%s+(%S+)%s+(.+)$")
        if not at then
            return ctx:edit(ui.usage(ctx:prefix(), "auto", { "daily 09:00 me Plan the day", "daily 21:30 -100123 Good night" }))
        end
        save(ctx, { type = "schedule", name = "Daily " .. at, chat = chat, text = text, at = at }, "Schedule saved")
    elseif action == "clock" then
        local field, template = ui.split(rest)
        if (field ~= "name" and field ~= "bio") or template == "" then
            return ctx:edit(ui.usage(ctx:prefix(), "auto", { "clock name Alex {clock} {time}", "clock bio Local time: {time}, {weekday}" },
                "Placeholders: {time} {date} {weekday} {clock}"))
        end
        save(ctx, { id = "clock-" .. field, type = "profile_clock", name = "Clock in " .. field, field = field, template = template }, "Profile clock on")
    elseif action == "online" or action == "offline" then
        save(ctx, { id = "presence", type = "presence", name = "Presence", online = action == "online" }, "Presence rule saved")
    elseif action == "presence" and rest == "off" then
        pcall(function() ctx:auto_delete("presence") end)
        ctx:edit(ui.ok("Presence rule removed"))
    else
        list(ctx)
    end
end

function M.away_cmd(ctx, args)
    if args == "off" then
        local ok = pcall(function() ctx:auto_delete("away") end)
        ctx:edit(ok and ui.toggle("Away replies", false) or ui.info("Away replies were not set"))
        return
    end
    local from, to, text = args:match("^(%d%d?:%d%d)%-(%d%d?:%d%d)%s+(.+)$")
    if not from then
        return ctx:edit(ui.usage(ctx:prefix(), "away", { "23:00-08:00 I'm asleep, I'll answer in the morning 🌙", "off" }))
    end
    save(ctx, { id = "away", type = "away_reply", name = "Away " .. from .. "–" .. to, from = from, to = to, text = text, cooldown_minutes = 60 }, "Away replies on")
end

return M
