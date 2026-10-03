local M = {}

M.meta = { name = "handlers", version = "2.0" }

M.commands = {
    afk = "afk_cmd",
    autoread = "autoread_cmd",
    antidelete = "antidelete_cmd",
}

M.help = {
    category = "automation",
    description = "Away mode, automatic read receipts, deleted message archive",
    commands = {
        afk = { args = "on [reason] | off", desc = "Auto-reply when someone mentions or writes to you" },
        autoread = { args = "on|off", desc = "Mark incoming messages as read" },
        antidelete = { args = "on|off", desc = "Keep copies of deleted messages (see the web panel)" },
    },
}

local function toggle(ctx, key, title, args, command)
    local action = ui.split(args)
    if action == "on" or action == "off" then
        ctx:db_set(key, action == "on")
        ctx:edit(ui.toggle(title, action == "on"))
    elseif action == "" or action == "status" then
        ctx:edit(ui.toggle(title, ctx:db_get(key) == true))
    else
        ctx:edit(ui.usage(ctx:prefix(), command, { "on", "off" }))
    end
end

function M.afk_cmd(ctx, args)
    local action, reason = ui.split(args)
    if action == "on" then
        ctx:db_set("handlers.afk.enabled", true)
        if reason ~= "" then
            ctx:db_set("handlers.afk.reason", reason)
        end
        ctx:edit(ui.toggle("AFK", true, "Reply: " .. ui.mono(ctx:db_get("handlers.afk.reason") or "I'm busy right now and will reply later.")))
    elseif action == "off" then
        ctx:db_set("handlers.afk.enabled", false)
        ctx:edit(ui.toggle("AFK", false))
    else
        ctx:edit(ui.toggle("AFK", ctx:db_get("handlers.afk.enabled") == true,
            ui.italic(ctx:prefix() .. "afk on [reason] · " .. ctx:prefix() .. "afk off")))
    end
end

function M.autoread_cmd(ctx, args)
    toggle(ctx, "handlers.autoread.enabled", "Auto-read", args, "autoread")
end

function M.antidelete_cmd(ctx, args)
    toggle(ctx, "handlers.antidelete.enabled", "Anti-delete", args, "antidelete")
end

return M
