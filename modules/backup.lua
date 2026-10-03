local M = {}

M.meta = { name = "backup", version = "1.0" }

M.commands = {
    backup = "backup_cmd",
    restore = "restore_cmd",
}

M.help = {
    category = "core",
    description = "Back up settings, database, and modules to Saved Messages",
    commands = {
        backup = "Create a backup archive and send it to Saved Messages",
        restore = { args = "(reply to a backup file)", desc = "Restore a backup and restart" },
    },
}

function M.backup_cmd(ctx, args)
    ctx:edit(ui.wait("Creating backup…"))
    local name = ctx:backup()
    ctx:edit(ui.ok("Backup sent to Saved Messages", ui.mono(name)))
end

function M.restore_cmd(ctx, args)
    ctx:edit(ui.wait("Restoring backup…"))
    local ok, restored = pcall(function()
        return ctx:restore_backup()
    end)
    if not ok then
        ctx:edit(ui.err("Restore failed", tostring(restored)))
        return
    end
    ctx:edit(ui.ok("Restored " .. #restored .. " file(s)", "The previous database was kept in " .. ui.mono("backups/") .. ". Restarting…"))
    ctx:sleep(1)
    ctx:restart()
end

return M
