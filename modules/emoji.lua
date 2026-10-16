local M = {}

M.meta = { name = "emoji", version = "1.0" }

M.commands = {
    emoji = "emoji_cmd",
}

M.help = {
    category = "core",
    description = "Premium emoji in userbot replies: icon pack, per-icon ids, and an id finder",
    commands = {
        emoji = {
            args = "[mode auto|on|off | pack <link>|off | set <icon> <id>|reset | id | preview]",
            desc = "Show or change premium emoji settings",
        },
    },
}

local function status_card(ctx)
    local status = ctx:premium_status()
    local lines = {
        ui.kv("Mode", status.mode),
        ui.kv("Pack", status.pack ~= nil and status.pack ~= json.null and status.pack or "none"),
    }
    if status.account_premium ~= nil and status.account_premium ~= json.null then
        table.insert(lines, ui.kv("This account has Premium", status.account_premium and "yes" or "no"))
    end
    local mapped = 0
    for _, icon in ipairs(status.icons or {}) do
        if icon.id ~= nil and icon.id ~= json.null then
            mapped = mapped + 1
        end
    end
    table.insert(lines, ui.kv("Icons with premium emoji", mapped .. "/" .. #(status.icons or {})))
    return ui.card(ui.icons.star, "Premium emoji", lines,
        "Accounts without Premium see the plain emoji. " .. ctx:prefix() .. "emoji preview shows every icon.")
end

local function preview(ctx)
    local status = ctx:premium_status()
    local lines = {}
    for _, icon in ipairs(status.icons or {}) do
        local shown = ui.icons[icon.name] or icon.fallback
        local id = (icon.id ~= nil and icon.id ~= json.null) and icon.id or "plain"
        table.insert(lines, shown .. " " .. icon.name .. " " .. ui.mono(id))
    end
    return ui.card(ui.icons.chart, "Icons", lines)
end

local function ids_in(ctx)
    local message = ctx:replied() or ctx:message()
    local found = message and message.custom_emoji or {}
    if #found == 0 then
        return ui.info("No premium emoji found", "Reply to a message that contains premium emoji.")
    end
    local lines = {}
    for _, item in ipairs(found) do
        table.insert(lines, ui.emoji(item.id, item.emoji) .. " " .. ui.mono(item.id))
    end
    return ui.card(ui.icons.search, "Premium emoji ids", lines,
        "Use one with " .. ctx:prefix() .. "emoji set <icon> <id>")
end

function M.emoji_cmd(ctx, args)
    local action, rest = ui.split(args)
    action = action:lower()
    if action == "" then
        ctx:edit(status_card(ctx))
    elseif action == "preview" then
        ctx:edit(preview(ctx))
    elseif action == "id" or action == "ids" then
        ctx:edit(ids_in(ctx))
    elseif action == "mode" then
        local value = ui.trim(rest):lower()
        local ok, err = ui.try(ctx.premium_set, ctx, "mode", value)
        if err then
            ctx:edit(ui.err("Mode not changed", err))
        else
            ctx:edit(status_card(ctx))
        end
    elseif action == "pack" then
        local value = ui.trim(rest)
        if value:lower() == "off" or value:lower() == "none" then
            value = ""
        end
        ctx:edit(ui.wait("Reading the pack"))
        local count, err = ui.try(ctx.premium_set, ctx, "pack", value)
        if err then
            ctx:edit(ui.err("Pack not used", err))
        elseif value == "" then
            ctx:edit(ui.ok("Pack removed"))
        else
            ctx:edit(ui.ok("Pack in use", count .. " icons matched by emoji"))
        end
    elseif action == "set" then
        local name, value = ui.split(rest)
        if name == "" then
            ctx:edit(ui.usage(ctx:prefix(), "emoji", { "set <icon> <id>", "set <icon> reset" }))
            return
        end
        local id = value:lower() == "reset" and "" or value
        local _, err = ui.try(ctx.premium_set, ctx, "icon", name, id)
        if err then
            ctx:edit(ui.err("Icon not changed", err))
        else
            ctx:edit(ui.ok("Icon updated", (ui.icons[name] or "") .. " " .. name))
        end
    else
        ctx:edit(ui.usage(ctx:prefix(), "emoji", {
            "", "preview", "id", "mode auto|on|off", "pack <t.me/addemoji/...>|off", "set <icon> <id>|reset",
        }, "Premium emoji in replies"))
    end
end

return M
