local M = {}

M.meta = { name = "executor", version = "2.0" }

M.help = {
    category = "core",
    description = "Run Lua expressions and shell commands (owner only)",
    commands = {
        eval = { args = "<lua>", desc = "Evaluate a Lua expression or chunk; `ctx` is available" },
        e = { args = "<lua>", desc = "Alias for eval" },
        term = { args = "<command>", desc = "Run a shell command with live output" },
    },
}

M.commands = {
    eval = "eval_cmd",
    e    = "eval_cmd",
    term = "term_cmd",
}

function M.eval_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "eval", { "<expression>" }))
        return
    end

    -- Expose ctx to the evaluated chunk for quick experiments.
    local env = setmetatable({ ctx = ctx }, { __index = _G })
    local fn, err = load("return " .. args, "=eval", "t", env)
    if not fn then
        fn, err = load(args, "=eval", "t", env)
    end

    if not fn then
        ctx:edit(ui.card(ui.icons.err, "Syntax error", { ui.code(ctx:sanitize(tostring(err))) }))
        return
    end

    local ok, result = pcall(fn)
    if ok then
        local shown = type(result) == "table" and json.encode(result) or tostring(result)
        ctx:edit(ui.card("🧪", "Result", { ui.code(ctx:sanitize(shown)) }))
    else
        ctx:edit(ui.card(ui.icons.err, "Runtime error", { ui.code(ctx:sanitize(tostring(result))) }))
    end
end

function M.term_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "term", { "<command>" }))
        return
    end

    ctx:run_term(args)
end

return M
