-- Curated marketplace client.

local M = {}

M.meta = { name = "market", version = "1.0" }

M.commands = {
    market = "market_cmd",
}

local bundled = {
    { name = "pmguard", description = "PM security controls", url = "" },
    { name = "osint", description = "IP, DNS, RDAP lookups", url = "" },
    { name = "animate", description = "Text animations", url = "" },
    { name = "files", description = "Download/upload file helpers", url = "" },
    { name = "group", description = "Group management controls", url = "" },
    { name = "ai", description = "Multi-provider AI and Whisper transcription helpers", url = "" },
    { name = "music", description = "External music worker controls", url = "" },
    { name = "automation", description = "Dry-run gift and task automation hooks", url = "" },
}

local function catalog(ctx)
    local url = tostring(ctx:db_get("marketplace.catalog_url") or "")
    if url ~= "" then
        local ok, data = pcall(function()
            return ctx:http_json_get(url)
        end)
        if ok and data then
            return data.modules or data
        end
    end
    return bundled
end

local function find_module(items, name)
    for _, item in ipairs(items) do
        if tostring(item.name) == name then
            return item
        end
    end
    return nil
end

local function render_items(items, filter)
    local lines = {}
    filter = tostring(filter or ""):lower()
    for _, item in ipairs(items) do
        local name = tostring(item.name or "")
        local desc = tostring(item.description or "")
        if filter == "" or name:lower():find(filter, 1, true) or desc:lower():find(filter, 1, true) then
            table.insert(lines, "`" .. name .. "` - " .. desc)
        end
    end
    if #lines == 0 then
        return "No modules found."
    end
    return table.concat(lines, "\n")
end

function M.market_cmd(ctx, args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    action = action or "search"
    rest = rest or ""

    if action == "source" then
        ctx:db_set("marketplace.catalog_url", rest)
        ctx:edit("**Marketplace**  \nCatalog URL: `" .. (rest ~= "" and rest or "bundled") .. "`")
        return
    end

    local items = catalog(ctx)
    if action == "search" or action == "list" then
        ctx:edit("**Marketplace**\n\n" .. render_items(items, rest))
        return
    end

    if action == "info" then
        local item = find_module(items, rest)
        if not item then
            ctx:edit("**Marketplace**  \nModule not found.")
            return
        end
        ctx:edit("**" .. tostring(item.name) .. "**  \n" .. tostring(item.description or "") .. "  \nURL: `" .. tostring(item.url or "bundled") .. "`")
        return
    end

    if action == "install" then
        local item = find_module(items, rest)
        if not item or tostring(item.url or "") == "" then
            ctx:edit("**Marketplace**  \nOnly remote catalog modules with URL can be installed. Bundled modules are already available.")
            return
        end
        ctx:edit(ctx:install_module(tostring(item.url), tostring(item.name)))
        return
    end

    if action == "audit" then
        ctx:edit("**Marketplace Audit**  \nInstalled module permissions are visible in the local dashboard.")
        return
    end

    ctx:edit("**Marketplace**  \n`.market search [text]`  \n`.market info <name>`  \n`.market install <name>`  \n`.market source <catalog-url>`")
end

return M
