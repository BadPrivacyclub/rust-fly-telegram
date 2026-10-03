local M = {}

M.meta = { name = "market", version = "2.0" }

M.commands = {
    market = "market_cmd",
}

M.help = {
    category = "modules",
    description = "Browse and install modules from a JSON catalog",
    commands = {
        market = { args = "search [text] | info <name> | install <name> | source <url>", desc = "Module catalog" },
    },
}

M.config = {
    { key = "catalog_url", type = "string", default = "", description = "JSON catalog: { \"modules\": [{ \"name\", \"description\", \"url\" }] }" },
}

local function catalog_url(ctx)
    local url = tostring(ctx:cfg("catalog_url") or "")
    if url == "" then
        -- Older versions stored the URL in the database.
        url = tostring(ctx:db_get("marketplace.catalog_url") or "")
    end
    return url
end

local function catalog(ctx)
    local url = catalog_url(ctx)
    if url == "" then
        return nil, "No catalog configured. Set one with " .. ctx:prefix() .. "market source <url>"
    end
    local ok, data = pcall(function()
        return ctx:http_json_get(url)
    end)
    if not ok or type(data) ~= "table" then
        return nil, "Catalog unavailable: " .. tostring(data)
    end
    return data.modules or data, nil
end

local function find(items, name)
    for _, item in ipairs(items) do
        if tostring(item.name) == name then
            return item
        end
    end
    return nil
end

function M.market_cmd(ctx, args)
    local action, rest = ui.split(args)
    action = action ~= "" and action or "search"

    if action == "source" then
        ctx:cfg_set("catalog_url", rest ~= "" and rest or nil)
        ctx:edit(ui.ok("Catalog source", ui.mono(rest ~= "" and rest or "not set")))
        return
    end

    local items, err = catalog(ctx)
    if not items then
        ctx:edit(ui.warn("Marketplace", err))
        return
    end

    if action == "search" or action == "list" then
        local filter = rest:lower()
        local lines = {}
        for _, item in ipairs(items) do
            local name = tostring(item.name or "")
            local desc = tostring(item.description or "")
            if filter == "" or name:lower():find(filter, 1, true) or desc:lower():find(filter, 1, true) then
                table.insert(lines, ui.bold(name) .. " · " .. desc)
            end
        end
        if #lines == 0 then
            lines = { ui.italic("Nothing found") }
        end
        ctx:edit(ui.card("🛍", "Marketplace", lines, ctx:prefix() .. "market install <name>"))
    elseif action == "info" then
        local item = find(items, rest)
        if not item then
            ctx:edit(ui.warn("Not in catalog", ui.mono(rest)))
            return
        end
        ctx:edit(ui.card(ui.icons.module, tostring(item.name), {
            tostring(item.description or ""),
            ui.kv("URL", item.url or "—"),
        }))
    elseif action == "install" then
        local item = find(items, rest)
        if not item or tostring(item.url or "") == "" then
            ctx:edit(ui.warn("Cannot install", "The catalog has no URL for " .. ui.mono(rest)))
            return
        end
        ctx:edit(ui.wait("Installing " .. rest .. "…"))
        ctx:edit(ctx:install_module(tostring(item.url), tostring(item.name)))
    else
        ctx:edit(ui.usage(ctx:prefix(), "market", { "search [text]", "info <name>", "install <name>", "source <url>" }))
    end
end

return M
