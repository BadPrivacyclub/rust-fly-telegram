local M = {}

M.meta = { name = "group", version = "2.0" }

M.commands = {
    cleanjoins = "cleanjoins_cmd",
    captcha = "captcha_cmd",
    purge = "purge_cmd",
    pin = "pin_cmd",
    unpin = "unpin_cmd",
    tagall = "tagall_cmd",
}

M.help = {
    category = "groups",
    description = "Group moderation: join cleanup, CAPTCHA, purge, pins, mentions",
    commands = {
        cleanjoins = { args = "on|off|status", desc = "Delete join/leave service messages" },
        captcha = { args = "on|off|status|text <text>", desc = "Ask new members for a code; {code} in text" },
        purge = { args = "(reply)", desc = "Delete every message from the replied one to this one" },
        pin = { args = "(reply)", desc = "Pin the replied message" },
        unpin = { args = "(reply)", desc = "Unpin the replied message" },
        tagall = { args = "[text]", desc = "Mention up to 100 members, five per message" },
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
        ctx:edit(ui.usage(ctx:prefix(), command, { "on", "off", "status" }))
    end
end

function M.cleanjoins_cmd(ctx, args)
    toggle(ctx, "group.clean_joins.enabled", "Clean join messages", args, "cleanjoins")
end

function M.captcha_cmd(ctx, args)
    local action, rest = ui.split(args)
    if action == "text" then
        if rest == "" then
            ctx:edit(ui.usage(ctx:prefix(), "captcha", { "text Welcome! Send {code} to stay." }))
            return
        end
        ctx:db_set("group.captcha.text", rest)
        ctx:edit(ui.ok("CAPTCHA text saved"))
        return
    end
    toggle(ctx, "group.captcha.enabled", "Group CAPTCHA", args, "captcha")
end

function M.purge_cmd(ctx, args)
    local msg = ctx:message()
    if not msg.reply_to_id then
        ctx:edit(ui.usage(ctx:prefix(), "purge", { "" }, "Reply to the first message to delete"))
        return
    end
    local count = ctx:purge()
    local note = ui.ok("Purged " .. tostring(count) .. " message(s)")
    ctx:reply(note)
end

function M.pin_cmd(ctx, args)
    ctx:pin()
    ctx:edit("📌 " .. ui.italic("Pinned"))
end

function M.unpin_cmd(ctx, args)
    ctx:unpin()
    ctx:edit("📍 " .. ui.italic("Unpinned"))
end

function M.tagall_cmd(ctx, args)
    local msg = ctx:message()
    if not msg.is_group then
        ctx:edit(ui.warn("Groups only"))
        return
    end
    ctx:delete()
    ctx:tag_all(args, 100)
end

return M
