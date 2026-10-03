local M = {}

M.meta = { name = "aliases", version = "2.0" }

M.commands = {
    alias = "alias_cmd",
}

M.help = {
    category = "messaging",
    description = "Text snippets: save once, paste anywhere with .alias <name>",
    commands = {
        alias = { args = "<name> | set <name> <text> | del <name> | list", desc = "Paste, save, delete, or list snippets" },
    },
}

local PREFIX = "alias."

function M.alias_cmd(ctx, args)
    local action, rest = ui.split(args)
    if action == "set" then
        local name, text = ui.split(rest)
        if name == "" or text == "" then
            ctx:edit(ui.usage(ctx:prefix(), "alias", { "set <name> <text>" }))
            return
        end
        ctx:db_set(PREFIX .. name, text)
        ctx:edit(ui.ok("Snippet saved", ui.mono(name)))
    elseif action == "del" then
        ctx:db_set(PREFIX .. rest, nil)
        ctx:edit(ui.ok("Snippet deleted", ui.mono(rest)))
    elseif action == "list" or action == "" then
        local names = {}
        for _, key in ipairs(ctx:db_keys(PREFIX)) do
            table.insert(names, ui.mono(key:sub(#PREFIX + 1)))
        end
        if #names == 0 then
            ctx:edit(ui.info("No snippets yet", ui.mono(ctx:prefix() .. "alias set <name> <text>")))
        else
            ctx:edit(ui.card("✂️", "Snippets", { table.concat(names, "  ") }, ctx:prefix() .. "alias <name> to paste"))
        end
    else
        local name = action == "get" and rest or action
        local value = ctx:db_get(PREFIX .. name)
        if value == nil then
            ctx:edit(ui.warn("No such snippet", ui.mono(name)))
        else
            -- Replace the command with the snippet text itself.
            ctx:edit(tostring(value))
        end
    end
end

return M
