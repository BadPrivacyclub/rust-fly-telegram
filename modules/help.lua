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

function M.help_cmd(ctx, args)
    ctx:edit(ctx:help(args))
end

return M
