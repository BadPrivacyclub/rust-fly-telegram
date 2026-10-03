local M = {}

M.meta = { name = "help", version = "2.0" }

M.commands = { help = "help_cmd" }

M.help = {
    category = "core",
    description = "Command reference generated from installed modules",
    commands = {
        help = { args = "[module or command]", desc = "Show commands or details for one module" },
    },
}

M.config = {
    {
        key = "inline",
        type = "bool",
        default = true,
        description = "Show help with navigation buttons through the control bot (needs inline mode in @BotFather)",
    },
}

function M.help_cmd(ctx, args)
    if ctx:cfg("inline") then
        local ok, sent = pcall(function()
            return ctx:inline_help(args)
        end)
        if ok and sent then
            return
        end
    end
    ctx:edit(ctx:help(args))
end

return M
