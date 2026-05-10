-- Alias module.

local M = {}

M.meta = { name = "aliases", version = "1.0" }

M.commands = {
    alias = "alias_cmd",
}

function M.alias_cmd(ctx, args)
    local action, name, rest = args:match("^(%S+)%s+(%S+)%s*(.*)$")

    if action == "set" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.alias set <name> <text>`")
            return
        end

        ctx:db_set("alias." .. name, rest)
        ctx:edit("**Alias**  \nName: `" .. name .. "`  \nStatus: `saved`")
        return
    end

    if action == "get" then
        local value = ctx:db_get("alias." .. name)
        if value == nil then
            ctx:edit("**Alias**  \nName: `" .. tostring(name) .. "`  \nStatus: `not found`")
        else
            ctx:edit("**Alias** `" .. name .. "`\n\n```text\n" .. tostring(value) .. "\n```")
        end
        return
    end

    if action == "del" then
        ctx:db_set("alias." .. name, nil)
        ctx:edit("**Alias**  \nName: `" .. name .. "`  \nStatus: `deleted`")
        return
    end

    ctx:edit("**Subcommands**  \n`set <name> <text>`  \n`get <name>`  \n`del <name>`")
end

return M
