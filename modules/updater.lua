local M = {}

M.meta = { name = "updater", version = "1.1" }

M.commands = {
    restart = "restart_cmd",
    update  = "update_cmd",
}

function M.restart_cmd(ctx, args)
    ctx:edit("**Restarting...**")
    ctx:db_set("restart_pending", true)
    os.exit(0)
end

function M.update_cmd(ctx, args)
    ctx:update_project()
end

return M
