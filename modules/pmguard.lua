local M = {}

M.meta = { name = "pmguard", version = "2.0" }

M.commands = {
    pmguard = "pmguard_cmd",
    approve = "approve_cmd",
    block = "block_cmd",
}

M.help = {
    category = "security",
    description = "Private message guard: strangers get a notice until you approve them",
    commands = {
        pmguard = { args = "on [text] | off | status | text <t> | denytext <t> | approvetext <t>", desc = "Configure the guard" },
        approve = { args = "[user_id]", desc = "Allow a user (in their chat or by ID)" },
        block = { args = "[user_id]", desc = "Deny a user (in their chat or by ID)" },
    },
}

local function csv_items(csv)
    local out = {}
    for item in tostring(csv or ""):gmatch("[^,]+") do
        item = item:match("^%s*(.-)%s*$")
        if item ~= "" then
            table.insert(out, item)
        end
    end
    return out
end

local function csv_set(ctx, key, value, present)
    local items = {}
    for _, item in ipairs(csv_items(ctx:db_get(key))) do
        if item ~= value then
            table.insert(items, item)
        end
    end
    if present then
        table.insert(items, value)
    end
    ctx:db_set(key, table.concat(items, ","))
end

-- Target user: explicit ID, the replied message's sender, or the private chat partner.
local function target_user(ctx, rest)
    if rest ~= "" then
        return rest:match("^%-?%d+$")
    end
    local replied = ctx:replied()
    if replied and replied.sender_id then
        return tostring(replied.sender_id)
    end
    local msg = ctx:message()
    if msg and msg.is_private and not msg.is_saved then
        return tostring(msg.chat_id)
    end
    return nil
end

local function decide(ctx, rest, allow)
    local user = target_user(ctx, rest)
    if not user then
        ctx:edit(ui.usage(ctx:prefix(), allow and "approve" or "block", { "<user_id>" }, "Or run it in the private chat"))
        return
    end
    csv_set(ctx, "pmguard.allow", user, allow)
    csv_set(ctx, "pmguard.deny", user, not allow)
    ctx:db_set("pmguard.challenge_seen." .. user, nil)
    if allow then
        ctx:edit(ui.ok("Approved", ui.mention(user, user)))
    else
        ctx:edit("⛔ " .. ui.bold("Blocked") .. "  \n" .. ui.mention(user, user))
    end
end

function M.approve_cmd(ctx, args)
    decide(ctx, args, true)
end

function M.block_cmd(ctx, args)
    decide(ctx, args, false)
end

function M.pmguard_cmd(ctx, args)
    local action, rest = ui.split(args)
    if action == "on" then
        ctx:db_set("pmguard.enabled", true)
        if rest ~= "" then
            ctx:db_set("pmguard.challenge_text", rest)
        end
        ctx:edit(ui.toggle("PM guard", true, ui.italic("New chats get a notice; approve them from the bot or with " .. ctx:prefix() .. "approve")))
    elseif action == "off" then
        ctx:db_set("pmguard.enabled", false)
        ctx:edit(ui.toggle("PM guard", false))
    elseif action == "allow" or action == "deny" then
        decide(ctx, rest, action == "allow")
    elseif action == "unallow" or action == "undeny" then
        csv_set(ctx, action == "unallow" and "pmguard.allow" or "pmguard.deny", rest, false)
        ctx:edit(ui.ok("Removed", ui.mono(rest)))
    elseif action == "text" or action == "denytext" or action == "approvetext" then
        if rest == "" then
            ctx:edit(ui.usage(ctx:prefix(), "pmguard", { action .. " <text>" }))
            return
        end
        local key = ({ text = "pmguard.challenge_text", denytext = "pmguard.deny_text", approvetext = "pmguard.approve_text" })[action]
        ctx:db_set(key, rest)
        ctx:edit(ui.ok("Text saved"))
    else
        local allow = csv_items(ctx:db_get("pmguard.allow"))
        local deny = csv_items(ctx:db_get("pmguard.deny"))
        ctx:edit(ui.card(ctx:db_get("pmguard.enabled") and ui.icons.on or ui.icons.off, "PM guard", {
            "Status: " .. ui.mono(ctx:db_get("pmguard.enabled") and "enabled" or "disabled"),
            ui.kv("Approved", #allow),
            ui.kv("Blocked", #deny),
            "",
            ui.mono(ctx:prefix() .. "pmguard on [text]") .. " · " .. ui.mono(ctx:prefix() .. "pmguard off"),
            ui.mono(ctx:prefix() .. "approve") .. " · " .. ui.mono(ctx:prefix() .. "block"),
        }))
    end
end

return M
