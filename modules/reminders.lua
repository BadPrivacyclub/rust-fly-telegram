local M = {}

M.meta = { name = "reminders", version = "1.0" }

M.commands = {
    remind = "remind_cmd",
    later = "later_cmd",
}

M.help = {
    category = "automation",
    description = "Reminders and delayed messages, delivered by Telegram even if the userbot is offline",
    commands = {
        remind = { args = "<time> <text>", desc = "Reminder in Saved Messages, e.g. 2h30m or 18:45" },
        later = { args = "<time> <text>", desc = "Send a message to this chat later" },
    },
}

M.config = {
    { key = "utc_offset", type = "number", default = 0, description = "Your UTC offset in hours, used for HH:MM times" },
}

-- Accepts durations (10m, 1h30m, 2d, 90) and wall-clock times (HH:MM, next occurrence).
local function parse_when(ctx, value)
    local hours, minutes = value:match("^(%d%d?):(%d%d)$")
    if hours then
        hours, minutes = tonumber(hours), tonumber(minutes)
        if hours > 23 or minutes > 59 then
            return nil
        end
        local offset = math.floor((tonumber(ctx:cfg("utc_offset")) or 0) * 3600)
        local now = ctx:now_ms() // 1000 + offset
        local today = now - now % 86400
        local target = today + hours * 3600 + minutes * 60
        if target <= now + 10 then
            target = target + 86400
        end
        return target - now
    end
    return ui.parse_duration(value)
end

local function schedule(ctx, args, target, command)
    local when, text = ui.split(args)
    local seconds = when ~= "" and parse_when(ctx, when) or nil
    if not seconds or text == "" then
        ctx:edit(ui.usage(ctx:prefix(), command, { "10m <text>", "1h30m <text>", "18:45 <text>" }))
        return
    end
    if seconds < 10 then
        seconds = 10
    end
    local body = text
    if target == "me" then
        body = ui.card(ui.icons.clock, "Reminder", { text })
    end
    ctx:schedule(seconds, body, target)
    ctx:edit(ui.ok(target == "me" and "Reminder set" or "Message scheduled",
        "In " .. ui.mono(ui.duration(seconds)) .. (target == "me" and " · Saved Messages" or "")))
end

function M.remind_cmd(ctx, args)
    schedule(ctx, args, "me", "remind")
end

function M.later_cmd(ctx, args)
    schedule(ctx, args, "here", "later")
    -- Keep the chat clean: the scheduled message is the visible result.
    ctx:sleep(3)
    pcall(function() ctx:delete() end)
end

return M
