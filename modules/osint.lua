local M = {}

M.meta = { name = "osint", version = "2.0" }

M.commands = {
    ip = "ip_cmd",
    domain = "domain_cmd",
    rdap = "rdap_cmd",
}

M.help = {
    category = "info",
    description = "Network lookups: IP geolocation, DNS records, RDAP registration data",
    commands = {
        ip = { args = "<ip-or-host>", desc = "Geolocation and network owner" },
        domain = { args = "<domain> [type]", desc = "DNS records (A, AAAA, MX, TXT, NS…)" },
        rdap = { args = "<domain-or-ip>", desc = "Registration data via RDAP" },
    },
}

local function urlencode(value)
    return (tostring(value):gsub("([^%w%-%._~])", function(ch)
        return string.format("%%%02X", string.byte(ch))
    end))
end

local function v(value)
    if value == nil or value == "" then
        return "—"
    end
    return tostring(value)
end

function M.ip_cmd(ctx, args)
    local query = args:match("^(%S+)$")
    if not query then
        ctx:edit(ui.usage(ctx:prefix(), "ip", { "<ip-or-host>" }))
        return
    end
    ctx:edit(ui.wait("Looking up " .. query .. "…"))
    local data = ctx:http_json_get("https://ipapi.co/" .. urlencode(query) .. "/json/")
    if data.error then
        ctx:edit(ui.err("Lookup failed", v(data.reason)))
        return
    end
    ctx:edit(ui.card("🌐", "IP " .. v(data.ip), {
        ui.kv("Location", v(data.city) .. ", " .. v(data.region) .. ", " .. v(data.country_name)),
        ui.kv("ASN", v(data.asn)),
        ui.kv("Organization", v(data.org)),
        ui.kv("Timezone", v(data.timezone)),
    }))
end

function M.domain_cmd(ctx, args)
    local query, kind = args:match("^(%S+)%s*(%a*)$")
    if not query then
        ctx:edit(ui.usage(ctx:prefix(), "domain", { "<domain>", "<domain> MX" }))
        return
    end
    kind = kind ~= "" and kind:upper() or "A"
    local data = ctx:http_json_get("https://dns.google/resolve?name=" .. urlencode(query) .. "&type=" .. kind)
    local answers = {}
    for _, answer in ipairs(data.Answer or {}) do
        table.insert(answers, ui.mono(answer.data) .. " " .. ui.italic("ttl " .. tostring(answer.TTL)))
    end
    if #answers == 0 then
        answers = { ui.italic("No " .. kind .. " records") }
    end
    ctx:edit(ui.card("🧭", query .. " · " .. kind, answers))
end

function M.rdap_cmd(ctx, args)
    local query = args:match("^(%S+)$")
    if not query then
        ctx:edit(ui.usage(ctx:prefix(), "rdap", { "<domain-or-ip>" }))
        return
    end
    local kind = (query:match("^%d+%.") or query:find(":")) and "ip" or "domain"
    local data = ctx:http_json_get("https://rdap.org/" .. kind .. "/" .. urlencode(query))
    local lines = {
        ui.kv("Name", v(data.ldhName or data.name)),
        ui.kv("Handle", v(data.handle)),
        ui.kv("Status", data.status and table.concat(data.status, ", ") or "—"),
    }
    for _, event in ipairs(data.events or {}) do
        if event.eventAction and event.eventDate then
            table.insert(lines, ui.kv(event.eventAction, event.eventDate:sub(1, 10)))
        end
    end
    ctx:edit(ui.card("📇", "RDAP " .. query, lines))
end

return M
