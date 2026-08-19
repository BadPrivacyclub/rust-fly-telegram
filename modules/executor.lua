local M = {}

M.meta = { name = "executor", version = "1.1" }

M.commands = {
    eval = "eval_cmd",
    e    = "eval_cmd",
    term = "term_cmd",
}

function M.eval_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.eval <expression>`")
        return
    end

    local fn, err = load("return " .. args)
    if not fn then
        fn, err = load(args)
    end

    if not fn then
        ctx:edit("**Syntax error**\n\n```text\n" .. ctx:sanitize(tostring(err)) .. "\n```")
        return
    end

    local ok, result = pcall(fn)
    if ok then
        ctx:edit("**Result**\n\n```text\n" .. ctx:sanitize(tostring(result)) .. "\n```")
    else
        ctx:edit("**Runtime error**\n\n```text\n" .. ctx:sanitize(tostring(result)) .. "\n```")
    end
end

function M.term_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.term <command>`")
        return
    end

    ctx:run_term(args)
end

return M
