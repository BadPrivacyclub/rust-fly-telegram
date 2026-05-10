-- Module installer.

local M = {}

M.meta = { name = "installer", version = "1.1" }

M.commands = {
    install = "install_cmd",
}

function M.install_cmd(ctx, args)
    local source, name = args:match("^(%S+)%s*(.*)$")
    if not source then
        local ok, result = pcall(function()
            return ctx:install_replied_module(nil)
        end)
        if ok then
            ctx:edit(tostring(result))
        else
            ctx:edit("**Usage**  \nReply `.install` to a `.lua` file.  \nOr use `.install <file-or-url> [name]`.")
        end
        return
    end

    if name == "" then
        name = nil
    end

    local ok, result = pcall(function()
        return ctx:install_module(source, name)
    end)

    if ok then
        ctx:edit(tostring(result))
    else
        ctx:edit("**Install failed**\n\n```text\n" .. tostring(result) .. "\n```")
    end
end

return M
