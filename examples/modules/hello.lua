-- The smallest useful module: one command, help text, and a setting.
local M = {}

M.commands = { hello = "hello_cmd" }

M.help = {
    category = "fun",
    description = "Says hello",
    commands = { hello = { args = "[name]", desc = "Greet someone" } },
}

M.config = {
    { key = "greeting", type = "string", default = "Hello", description = "Word used to greet" },
}

function M.hello_cmd(ctx, args)
    local name = args ~= "" and args or "world"
    ctx:edit(ui.ok(ctx:cfg("greeting") .. ", " .. name .. "!"))
end

return M
