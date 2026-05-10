-- Handlers module: controls event-based userbot features.

local M = {}

M.meta = { name = "handlers", version = "1.0" }

M.commands = {
    afk = "afk_cmd",
    autoread = "autoread_cmd",
    antidelete = "antidelete_cmd",
}

local function parse_toggle(args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    return action, rest or ""
end

local function set_toggle(ctx, key, label, args)
    local action = parse_toggle(args)
    if action == "on" then
        ctx:db_set(key, true)
        ctx:edit("**" .. label .. "**  \nStatus: `enabled`")
        return true
    end

    if action == "off" then
        ctx:db_set(key, false)
        ctx:edit("**" .. label .. "**  \nStatus: `disabled`")
        return true
    end

    return false
end

function M.afk_cmd(ctx, args)
    local action, reason = parse_toggle(args)
    if action == "on" then
        ctx:db_set("handlers.afk.enabled", true)
        if reason ~= "" then
            ctx:db_set("handlers.afk.reason", reason)
        end
        ctx:edit("**AFK**  \nStatus: `enabled`")
        return
    end

    if action == "off" then
        ctx:db_set("handlers.afk.enabled", false)
        ctx:edit("**AFK**  \nStatus: `disabled`")
        return
    end

    ctx:edit("**Usage**  \n`.afk on [text]`  \n`.afk off`")
end

function M.autoread_cmd(ctx, args)
    if not set_toggle(ctx, "handlers.autoread.enabled", "Autoread", args) then
        ctx:edit("**Usage**  \n`.autoread on`  \n`.autoread off`")
    end
end

function M.antidelete_cmd(ctx, args)
    if not set_toggle(ctx, "handlers.antidelete.enabled", "Anti-delete", args) then
        ctx:edit("**Usage**  \n`.antidelete on`  \n`.antidelete off`")
    end
end

return M
