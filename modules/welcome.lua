local M = {}

M.meta = { name = "welcome", version = "1.0" }

M.commands = {
    welcome = "welcome_cmd",
}

M.help = {
    category = "groups",
    description = "Greets new members in chosen groups",
    commands = {
        welcome = { args = "on [text] | off | status", desc = "Placeholders: {mention}, {name}, {id}, {chat}" },
    },
}

local DEFAULT_TEXT = "👋 Welcome, {mention}!"

local function key_for(chat_id)
    return "welcome." .. tostring(chat_id)
end

function M.welcome_cmd(ctx, args)
    local msg = ctx:message()
    if not msg.is_group then
        ctx:edit(ui.warn("Groups only", "Run this command inside the group to greet."))
        return
    end
    local action, text = ui.split(args)
    local key = key_for(msg.chat_id)
    if action == "on" then
        ctx:db_set(key, text ~= "" and text or DEFAULT_TEXT)
        ctx:edit(ui.toggle("Welcome messages", true, "Text: " .. ui.mono(ctx:db_get(key))))
    elseif action == "off" then
        ctx:db_set(key, nil)
        ctx:edit(ui.toggle("Welcome messages", false))
    elseif action == "status" or action == "" then
        local current = ctx:db_get(key)
        ctx:edit(ui.toggle("Welcome messages", current ~= nil, current and ("Text: " .. ui.mono(current)) or nil))
    else
        ctx:edit(ui.usage(ctx:prefix(), "welcome", { "on [text]", "off", "status" }, "Placeholders: {mention} {name} {id} {chat}"))
    end
end

function M.on_message(ctx, msg)
    if msg.action ~= "join" or not msg.is_group then
        return
    end
    local template = ctx:db_get(key_for(msg.chat_id))
    if not template then
        return
    end
    local greetings = {}
    for _, user_id in ipairs(msg.action_users or {}) do
        -- The sender's name is only known for users who joined by themselves.
        local name = (user_id == msg.sender_id and msg.sender_name) or "friend"
        local text = template
            :gsub("{mention}", (ui.mention(user_id, name):gsub("%%", "%%%%")))
            :gsub("{name}", (name:gsub("%%", "%%%%")))
            :gsub("{id}", tostring(user_id))
            :gsub("{chat}", ((msg.chat_title or ""):gsub("%%", "%%%%")))
        table.insert(greetings, text)
    end
    if #greetings > 0 then
        ctx:reply(table.concat(greetings, "\n"))
    end
end

return M
