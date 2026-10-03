local M = {}

M.meta = { name = "updater", version = "2.0" }

M.commands = {
    restart = "restart_cmd",
    update = "update_cmd",
}

M.help = {
    category = "core",
    description = "Restart in place and update from git or GitHub Releases",
    commands = {
        restart = "Restart the userbot process",
        update = { args = "[now]", desc = "Check for updates; `now` installs the latest release" },
    },
}

function M.restart_cmd(ctx, args)
    ctx:restart()
end

function M.update_cmd(ctx, args)
    ctx:edit(ui.wait("Checking for updates…"))
    local info = ctx:check_update()
    if info.source_checkout then
        -- Source installs pull and rebuild with cargo.
        ctx:update_project()
        return
    end

    if not info.newer then
        ctx:edit(ui.ok("Up to date", ui.mono("v" .. tostring(info.current))))
        return
    end

    if args ~= "now" then
        local notes = tostring(info.notes or "")
        if #notes > 900 then
            notes = notes:sub(1, 900) .. "…"
        end
        local lines = {
            ui.kv("Installed", "v" .. tostring(info.current)),
            ui.kv("Available", "v" .. tostring(info.latest)),
        }
        if not info.has_binary then
            table.insert(lines, ui.icons.warn .. " No prebuilt binary for this platform; download it from the release page.")
        end
        if notes ~= "" then
            table.insert(lines, "")
            table.insert(lines, ui.code(notes))
        end
        ctx:edit(ui.card("⬆️", "Update available", lines,
            "Install with " .. ctx:prefix() .. "update now · " .. tostring(info.url)))
        return
    end

    ctx:edit(ui.wait("Downloading v" .. tostring(info.latest) .. "…"))
    ctx:self_update()
end

return M
