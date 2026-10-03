-- Persistent to-do list: stores a Lua table in the database.
local M = {}

M.commands = { todo = "todo_cmd" }

M.help = {
    category = "messaging",
    description = "A tiny to-do list",
    commands = { todo = { args = "add <text> | done <n> | clear | list", desc = "Manage your to-do list" } },
}

M.config = {
    { key = "max_items", type = "number", default = 20, description = "List size limit" },
}

local KEY = "todo.items"

local function load(ctx)
    local list = ctx:db_get(KEY)
    return type(list) == "table" and list or {}
end

function M.todo_cmd(ctx, args)
    local action, rest = ui.split(args)
    local list = load(ctx)

    if action == "add" and rest ~= "" then
        if #list >= ctx:cfg("max_items") then
            return ctx:edit(ui.warn("The list is full", "Raise the limit with " .. ui.mono(ctx:prefix() .. "cfg todo max_items 50")))
        end
        table.insert(list, rest)
        ctx:db_set(KEY, list)
        ctx:edit(ui.ok("Added", rest))
    elseif action == "done" and tonumber(rest) then
        local removed = table.remove(list, tonumber(rest))
        ctx:db_set(KEY, #list > 0 and list or nil)
        ctx:edit(removed and ui.ok("Done", removed) or ui.err("No item #" .. rest))
    elseif action == "clear" then
        ctx:db_set(KEY, nil)
        ctx:edit(ui.ok("List cleared"))
    else
        local lines = {}
        for i, item in ipairs(list) do
            table.insert(lines, ui.mono(i) .. " " .. item)
        end
        ctx:edit(ui.card("✅", "To-do", #lines > 0 and lines or { ui.italic("Empty") },
            ctx:prefix() .. "todo add <text> · done <n>"))
    end
end

return M
