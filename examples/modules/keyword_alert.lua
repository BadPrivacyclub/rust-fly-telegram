-- Event handler: notifies you through the control bot when a watched word appears.
-- Needs the telegram.read permission when installed as a third-party module.
local M = {}

M.commands = { watch = "watch_cmd" }

M.help = {
    category = "automation",
    description = "Get a bot notification when someone writes a watched word",
    commands = { watch = { args = "add <word> | del <word> | list", desc = "Manage watched words" } },
}

local KEY = "keyword_alert.words"

local function words(ctx)
    local list = ctx:db_get(KEY)
    return type(list) == "table" and list or {}
end

function M.watch_cmd(ctx, args)
    local action, word = ui.split(args)
    local list = words(ctx)
    word = word:lower()
    if action == "add" and word ~= "" then
        table.insert(list, word)
        ctx:db_set(KEY, list)
        ctx:edit(ui.ok("Watching", ui.mono(word)))
    elseif action == "del" and word ~= "" then
        for i = #list, 1, -1 do
            if list[i] == word then table.remove(list, i) end
        end
        ctx:db_set(KEY, list)
        ctx:edit(ui.ok("Stopped watching", ui.mono(word)))
    else
        ctx:edit(ui.card("👀", "Watched words", #list > 0 and { table.concat(list, ", ") } or { ui.italic("None") }))
    end
end

function M.on_message(ctx, msg)
    if msg.from_self or msg.text == "" then
        return
    end
    local text = msg.text:lower()
    for _, word in ipairs(words(ctx)) do
        if text:find(word, 1, true) then
            ctx:notify(string.format("%s in %s: %s",
                msg.sender_name or "Someone", msg.chat_title or tostring(msg.chat_id), msg.text))
            return
        end
    end
end

return M
