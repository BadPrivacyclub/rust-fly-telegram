local M = {}

M.meta = { name = "installer", version = "2.0" }

M.help = {
    category = "modules",
    description = "Install Lua modules from a file, URL, or replied document",
    commands = {
        install = { args = "<file-or-url> [name] | (reply to .lua)", desc = "Install a module; it runs sandboxed until you grant permissions" },
    },
}

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
            ctx:edit(ui.usage(ctx:prefix(), "install", { "<file-or-url> [name]" }, "Or reply to a .lua file"))
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
        ctx:edit(ui.card(ui.icons.err, "Install failed", { ui.code(tostring(result)) }))
    end
end

return M
