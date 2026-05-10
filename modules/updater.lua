-- Updater module: restarts and updates the userbot process.

local M = {}

M.meta = { name = "updater", version = "1.2" }

M.commands = {
    restart = "restart_cmd",
    update  = "update_cmd",
    version = "version_cmd",
}

function M.restart_cmd(ctx, args)
    ctx:edit("**Restarting...**")
    os.exit(0)
end

function M.update_cmd(ctx, args)
    ctx:update_project()
end

function M.version_cmd(ctx, args)
    local commit = ctx:run_capture("git log -1 --pretty=format:'%h — %s (%cr)'")
    local branch = ctx:run_capture("git rev-parse --abbrev-ref HEAD")
    branch = branch:gsub("%s+", "")
    commit = commit:gsub("^%s*(.-)%s*$", "%1")
    if commit == "" then
        ctx:edit("**fly-telegram**  \nNo git history found.")
    else
        ctx:edit("**fly-telegram**  \nBranch: `" .. branch .. "`  \nCommit: `" .. commit .. "`")
    end
end

return M
