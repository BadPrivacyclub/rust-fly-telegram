local M = {}

M.meta = { name = "settings", version = "1.0" }

M.commands = {
    prefix = "prefix_cmd",
    lang = "lang_cmd",
    modules = "modules_cmd",
    module = "module_cmd",
    cfg = "cfg_cmd",
    sudo = "sudo_cmd",
    panel = "panel_cmd",
}

M.help = {
    category = "core",
    description = "Userbot settings: prefixes, language, modules, sudo users, web panel",
    commands = {
        prefix = { args = "[prefix ...]", desc = "Show or set command prefixes, e.g. `. !`" },
        lang = { args = "en|ru", desc = "Interface language for core messages and the bot" },
        modules = "List modules and their state",
        module = { args = "on|off|reload|remove|restore <name>", desc = "Manage a module" },
        cfg = { args = "[module] [key] [value|reset]", desc = "View and change module settings" },
        sudo = { args = "add|del [user_id] | list", desc = "Users allowed to run safe commands" },
        panel = "One-time login link for the web panel",
    },
}

local function usage(ctx, command, variants, description)
    ctx:edit(ui.usage(ctx:prefix(), command, variants, description))
end

local function find_module(list, name)
    for _, module in ipairs(list) do
        if module.name == name then
            return module
        end
    end
    return nil
end

function M.prefix_cmd(ctx, args)
    if args == "" then
        local current = ctx:prefixes()
        local shown = {}
        for _, prefix in ipairs(current) do
            table.insert(shown, ui.mono(prefix))
        end
        ctx:edit(ui.card(ui.icons.gear, "Command prefixes", {
            "Current: " .. table.concat(shown, " "),
            ui.italic("Change: " .. current[1] .. "prefix . !"),
        }))
        return
    end
    local values = {}
    for value in args:gmatch("%S+") do
        table.insert(values, value)
    end
    local ok, result = pcall(function()
        return ctx:set_prefixes(values)
    end)
    if not ok then
        ctx:edit(ui.err("Prefix not changed", tostring(result)))
        return
    end
    local shown = {}
    for _, prefix in ipairs(result) do
        table.insert(shown, ui.mono(prefix))
    end
    ctx:edit(ui.ok("Prefixes saved", table.concat(shown, " ")))
end

function M.lang_cmd(ctx, args)
    local code = args:lower()
    if code ~= "en" and code ~= "ru" then
        ctx:edit(ui.card("🌐", "Language", {
            ui.kv("Current", ctx:lang()),
            ui.italic(ctx:prefix() .. "lang en | " .. ctx:prefix() .. "lang ru"),
        }))
        return
    end
    local saved = ctx:set_lang(code)
    ctx:edit(ui.ok(saved == "ru" and "Язык: русский" or "Language: English"))
end

function M.modules_cmd(ctx, args)
    local list = ctx:modules_list()
    local filter = args:lower()
    local lines = {}
    local enabled = 0
    for _, module in ipairs(list) do
        if module.enabled then
            enabled = enabled + 1
        end
        if filter == "" or module.name:lower():find(filter, 1, true) then
            local flags = {}
            if not module.trusted then table.insert(flags, "sandbox") end
            if module.settings > 0 then table.insert(flags, "⚙️" .. module.settings) end
            table.insert(lines, string.format(
                "%s %s %s · %d cmd%s",
                module.enabled and ui.icons.on or ui.icons.off,
                ui.bold(module.name),
                ui.mono("v" .. module.version),
                #module.commands,
                #flags > 0 and (" · " .. table.concat(flags, " ")) or ""
            ))
        end
    end
    ctx:edit(ui.card(ui.icons.module, "Modules", lines,
        string.format("%d/%d enabled · %smodule on|off|reload <name>", enabled, #list, ctx:prefix())))
end

function M.module_cmd(ctx, args)
    local action, name = ui.split(args)
    if action == "" then
        usage(ctx, "module", { "<name>", "on <name>", "off <name>", "reload <name>", "remove <name>", "restore <name|all>" })
        return
    end
    if name == "" and action ~= "restore" then
        ctx:edit(ctx:help(action))
        return
    end

    local list = ctx:modules_list()
    if action == "restore" then
        local target = (name == "" or name == "all") and nil or name
        local restored = ctx:module_restore(target)
        if #restored == 0 then
            ctx:edit(ui.warn("Nothing restored", "No bundled module named " .. ui.mono(name)))
        else
            ctx:edit(ui.ok("Restored bundled files", ui.list(restored)))
        end
        return
    end

    local module = find_module(list, name)
    if not module then
        ctx:edit(ui.err("Unknown module", ui.mono(name)))
        return
    end

    if action == "on" or action == "off" then
        if module.name == "settings" and action == "off" then
            ctx:edit(ui.warn("Refusing to disable the settings module", "You would lose access to this command."))
            return
        end
        ctx:module_set_enabled(name, action == "on")
        ctx:edit(ui.toggle(name, action == "on"))
    elseif action == "reload" then
        ctx:module_reload(name)
        ctx:edit(ui.ok("Reloaded", ui.mono(name)))
    elseif action == "remove" then
        if module.bundled then
            ctx:edit(ui.warn("Bundled module", "It would be reinstalled on restart. Use " .. ui.mono(ctx:prefix() .. "module off " .. name) .. " instead."))
            return
        end
        ctx:module_remove(name)
        ctx:edit(ui.ok("Removed", ui.mono(name)))
    else
        ctx:edit(ctx:help(action))
    end
end

local function render_config(ctx, name)
    local entries = ctx:module_config(name)
    if #entries == 0 then
        ctx:edit(ui.info("No settings", ui.mono(name) .. " has nothing to configure."))
        return
    end
    local lines = {}
    for _, entry in ipairs(entries) do
        local line = ui.bold(entry.key) .. " = " .. ui.mono(entry.value)
        if entry.type == "select" and #entry.options > 0 then
            line = line .. " " .. ui.italic("(" .. table.concat(entry.options, " | ") .. ")")
        elseif entry.type == "bool" then
            line = line .. " " .. ui.italic("(on | off)")
        end
        table.insert(lines, line)
        if entry.description ~= "" then
            table.insert(lines, "    " .. ui.italic(entry.description))
        end
    end
    ctx:edit(ui.card(ui.icons.gear, name .. " settings", lines,
        ctx:prefix() .. "cfg " .. name .. " <key> <value>  ·  reset"))
end

function M.cfg_cmd(ctx, args)
    local name, rest = ui.split(args)
    if name == "" then
        local lines = {}
        for _, module in ipairs(ctx:modules_list()) do
            if module.settings > 0 then
                table.insert(lines, ui.mono(ctx:prefix() .. "cfg " .. module.name) .. " · " .. module.settings)
            end
        end
        ctx:edit(ui.card(ui.icons.gear, "Configurable modules", lines))
        return
    end
    local key, value = ui.split(rest)
    if key == "" then
        render_config(ctx, name)
        return
    end
    if value == "" then
        ctx:edit(ui.usage(ctx:prefix(), "cfg", { name .. " " .. key .. " <value>", name .. " " .. key .. " reset" }))
        return
    end
    local ok, result = pcall(function()
        return ctx:module_config_set(name, key, value ~= "reset" and value or nil)
    end)
    if ok then
        ctx:edit(ui.ok("Saved", ui.mono(name .. "." .. key) .. " = " .. ui.mono(result)))
    else
        ctx:edit(ui.err("Not saved", tostring(result)))
    end
end

function M.sudo_cmd(ctx, args)
    local action, rest = ui.split(args)
    local users = ctx:sudo_list()
    if action == "add" or action == "del" then
        local id = tonumber(rest)
        if not id then
            local reply = ctx:replied()
            id = reply and reply.sender_id
        end
        if not id then
            usage(ctx, "sudo", { "add <user_id>", "del <user_id>" }, "Or reply to the user's message")
            return
        end
        local next_users = {}
        for _, existing in ipairs(users) do
            if existing ~= id then
                table.insert(next_users, existing)
            end
        end
        if action == "add" then
            table.insert(next_users, id)
        end
        ctx:sudo_set(next_users)
        ctx:edit(ui.ok(action == "add" and "Sudo user added" or "Sudo user removed", ui.mono(id)))
        return
    end
    local lines = {}
    for _, id in ipairs(users) do
        table.insert(lines, ui.mention(id, tostring(id)))
    end
    if #lines == 0 then
        lines = { ui.italic("No sudo users") }
    end
    table.insert(lines, "")
    table.insert(lines, ui.italic("Sudo users cannot run eval, shell, install, update, settings, or file commands."))
    ctx:edit(ui.card(ui.icons.lock, "Sudo users", lines))
end

function M.panel_cmd(ctx, args)
    local link = ctx:panel_link()
    local text = ui.card("🖥", "Web panel", {
        "One-time login link, valid 15 minutes, works only on the computer running fly-telegram:",
        ui.mono(link),
    })
    local msg = ctx:message()
    if msg and msg.is_saved then
        ctx:edit(text)
    else
        ctx:send_saved(text)
        ctx:edit(ui.ok("Panel link sent to Saved Messages"))
    end
end

return M
