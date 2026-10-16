local M = {}

M.meta = { name = "osint", version = "2.1" }

M.commands = {
    ip = "ip_cmd",
    domain = "domain_cmd",
    rdap = "rdap_cmd",
}

M.help = {
    category = "info",
    description = "Network lookups: IP geolocation, DNS records, RDAP registration data",
    commands = {
        ip = { args = "[ip-or-host]", desc = "Geolocation and network owner (no argument: your public IP)" },
        domain = { args = "<domain> [type]", desc = "DNS records: A, AAAA, MX, TXT, NS, CNAME, SOA, CAA, SRV, PTR" },
        rdap = { args = "<domain | ip | AS number>", desc = "Registration data: registrar, dates, status, name servers" },
    },
}

local DNS_TYPES = {
    A = 1, NS = 2, CNAME = 5, SOA = 6, PTR = 12, MX = 15, TXT = 16, AAAA = 28, SRV = 33, CAA = 257,
}

local DNS_STATUS = {
    [1] = "malformed query", [2] = "server failure", [3] = "the domain does not exist",
    [5] = "the query was refused",
}

local function v(value)
    if value == nil or value == "" then
        return "-"
    end
    return tostring(value)
end

-- I reduce "https://Example.com/path" to "example.com".
local function clean_host(text)
    text = ui.trim(text):lower()
    text = text:gsub("^%a[%w+.-]*://", ""):gsub("[/?#].*$", ""):gsub("^[^@]*@", "")
    if not text:find(":.*:") then
        text = text:gsub(":%d+$", "")
    end
    return (text:gsub("%.$", ""))
end

local function is_ipv4(text)
    local a, b, c, d = text:match("^(%d+)%.(%d+)%.(%d+)%.(%d+)$")
    if not a then return false end
    for _, part in ipairs({ a, b, c, d }) do
        if tonumber(part) > 255 then return false end
    end
    return true
end

local function is_ipv6(text)
    return text:find(":", 1, true) ~= nil and text:match("^[%x:%.]+$") ~= nil
end

local function is_domain(text)
    return text:match("^[%w%-%.]+%.[%a%d%-]+$") ~= nil and not text:find("..", 1, true)
end

local function resolve_host(ctx, host)
    local res = ctx:fetch("https://dns.google/resolve", { query = { name = host, type = "A" } })
    for _, answer in ipairs(ui.get(res, "json.Answer", {})) do
        if answer.type == 1 and is_ipv4(tostring(answer.data)) then
            return answer.data
        end
    end
    return nil
end

local function ip_from_ipapi(ctx, ip)
    local url = ip and ("https://ipapi.co/" .. ui.urlencode(ip) .. "/json/") or "https://ipapi.co/json/"
    local res = ctx:fetch(url, { timeout = 15 })
    local data = res.json
    if not res.ok or type(data) ~= "table" or data.error then
        return nil, ui.first(ui.get(data, "reason"), res.error, "no data")
    end
    return {
        ip = data.ip,
        city = data.city, region = data.region, country = data.country_name,
        asn = data.asn, org = data.org, timezone = data.timezone,
        coords = data.latitude and (ui.num(data.latitude, 4) .. ", " .. ui.num(data.longitude, 4)),
    }
end

local function ip_from_ipwhois(ctx, ip)
    local res = ctx:fetch("https://ipwho.is/" .. (ip and ui.urlencode(ip) or ""), { timeout = 15 })
    local data = res.json
    if not res.ok or type(data) ~= "table" or data.success == false then
        return nil, ui.first(ui.get(data, "message"), res.error, "no data")
    end
    local asn = ui.get(data, "connection.asn")
    return {
        ip = data.ip,
        city = data.city, region = data.region, country = data.country,
        asn = asn and ("AS" .. tostring(asn)),
        org = ui.first(ui.get(data, "connection.org"), ui.get(data, "connection.isp")),
        timezone = ui.get(data, "timezone.id"),
        coords = data.latitude and (ui.num(data.latitude, 4) .. ", " .. ui.num(data.longitude, 4)),
    }
end

function M.ip_cmd(ctx, args)
    local query = clean_host(args)
    local ip = nil
    if query ~= "" then
        if is_ipv4(query) or is_ipv6(query) then
            ip = query
        elseif is_domain(query) then
            ctx:edit(ui.wait("Resolving " .. query .. "…"))
            ip = resolve_host(ctx, query)
            if not ip then
                ctx:edit(ui.err("Could not resolve", ui.mono(query)))
                return
            end
        else
            ctx:edit(ui.usage(ctx:prefix(), "ip", { "", "1.1.1.1", "example.com" }))
            return
        end
    end
    ctx:edit(ui.wait("Looking up " .. (ip or "your public IP") .. "…"))

    local info, first_error = ip_from_ipapi(ctx, ip)
    if not info then
        local second_error
        info, second_error = ip_from_ipwhois(ctx, ip)
        if not info then
            ctx:edit(ui.err("Lookup failed", ui.mono(ui.clip(first_error .. "; " .. second_error, 300))))
            return
        end
    end

    local place = {}
    for _, part in ipairs({ info.city, info.region, info.country }) do
        if part and part ~= "" then table.insert(place, tostring(part)) end
    end
    local title = "IP " .. v(info.ip)
    if query ~= "" and query ~= ip then
        title = title .. " (" .. query .. ")"
    end
    ctx:edit(ui.card("🌐", title, {
        ui.kv("Location", #place > 0 and table.concat(place, ", ") or "-"),
        ui.kv("Coordinates", v(info.coords)),
        ui.kv("ASN", v(info.asn)),
        ui.kv("Organization", v(info.org)),
        ui.kv("Timezone", v(info.timezone)),
    }))
end

function M.domain_cmd(ctx, args)
    local words = ui.args(args)
    local query = clean_host(words[1] or "")
    local kind = (words[2] or "A"):upper()
    if query == "" or not DNS_TYPES[kind] then
        ctx:edit(ui.usage(ctx:prefix(), "domain", { "example.com", "example.com MX", "example.com TXT" },
            "Types: A AAAA MX TXT NS CNAME SOA CAA SRV PTR"))
        return
    end
    local res = ctx:fetch("https://dns.google/resolve", { query = { name = query, type = kind } })
    local data = res.json
    if type(data) ~= "table" then
        ctx:edit(ui.err("DNS lookup failed", ui.mono(v(res.error))))
        return
    end
    local status = tonumber(data.Status) or 0
    if status ~= 0 then
        ctx:edit(ui.err(query .. " · " .. kind, DNS_STATUS[status] or ("DNS status " .. status)))
        return
    end
    local answers = {}
    for _, answer in ipairs(data.Answer or {}) do
        if answer.data then
            local line = ui.mono(ui.clip(answer.data, 300))
            if answer.TTL then
                line = line .. " " .. ui.italic("ttl " .. tostring(answer.TTL))
            end
            table.insert(answers, line)
        end
        if #answers >= 25 then break end
    end
    if #answers == 0 then
        answers = { ui.italic("No " .. kind .. " records") }
    end
    ctx:edit(ui.card("🧭", query .. " · " .. kind, answers))
end

local function rdap_kind(query)
    if is_ipv4(query) or is_ipv6(query) or query:match("^[%d%.]+/%d+$") or query:match("^[%x:]+/%d+$") then
        return "ip", query
    end
    local asn = query:match("^as(%d+)$") or query:match("^(%d+)$")
    if asn then
        return "autnum", asn
    end
    if is_domain(query) then
        return "domain", query
    end
    return nil
end

-- I find the registry's RDAP base URL for a TLD in the IANA bootstrap file.
local function bootstrap_base(ctx, tld)
    local res = ctx:fetch("https://data.iana.org/rdap/dns.json", { timeout = 20 })
    for _, service in ipairs(ui.get(res, "json.services", {})) do
        for _, name in ipairs(service[1] or {}) do
            if tostring(name):lower() == tld then
                local base = ui.get(service, "2.1")
                if base then
                    return (tostring(base):gsub("/?$", "/"))
                end
            end
        end
    end
    return nil
end

local function rdap_fetch(ctx, kind, value)
    local headers = { Accept = "application/rdap+json, application/json" }
    local res = ctx:fetch("https://rdap.org/" .. kind .. "/" .. ui.urlencode(value), { headers = headers })
    if res.ok and type(res.json) == "table" then
        return res.json
    end
    if res.status == 404 then
        return nil, "not_found"
    end
    -- rdap.org only redirects, so when it fails I ask the registry directly.
    local fallback
    if kind == "domain" then
        local base = bootstrap_base(ctx, value:match("([^%.]+)$"))
        fallback = base and (base .. "domain/" .. ui.urlencode(value))
    else
        fallback = "https://rdap.arin.net/registry/" .. kind .. "/" .. ui.urlencode(value)
    end
    if fallback then
        local second = ctx:fetch(fallback, { headers = headers })
        if second.ok and type(second.json) == "table" then
            return second.json
        end
        if second.status == 404 then
            return nil, "not_found"
        end
        return nil, ui.first(second.error, res.error)
    end
    return nil, ui.first(res.error, "no RDAP server for this domain")
end

local function vcard_field(entity, field)
    for _, item in ipairs(ui.get(entity, "vcardArray.2", {})) do
        if type(item) == "table" and item[1] == field then
            local value = item[4]
            if type(value) == "table" then
                value = table.concat(value, " ")
            end
            if value and value ~= "" then
                return tostring(value)
            end
        end
    end
    return nil
end

local function has_role(entity, role)
    for _, r in ipairs(entity.roles or {}) do
        if r == role then return true end
    end
    return false
end

-- I walk nested entities, since registries put the abuse contact under the registrar.
local function find_entity(entities, role, depth)
    depth = depth or 0
    if type(entities) ~= "table" or depth > 3 then return nil end
    for _, entity in ipairs(entities) do
        if type(entity) == "table" then
            if has_role(entity, role) then
                return entity
            end
            local nested = find_entity(entity.entities, role, depth + 1)
            if nested then return nested end
        end
    end
    return nil
end

local function as_list(value)
    if type(value) == "table" then
        local out = {}
        for _, item in ipairs(value) do
            table.insert(out, tostring(item))
        end
        return out
    end
    if value ~= nil and value ~= "" then
        return { tostring(value) }
    end
    return {}
end

local EVENT_LABELS = {
    registration = "Registered",
    expiration = "Expires",
    ["last changed"] = "Updated",
    transfer = "Transferred",
}

function M.rdap_cmd(ctx, args)
    local query = clean_host(args)
    local kind, value = rdap_kind(query)
    if not kind then
        ctx:edit(ui.usage(ctx:prefix(), "rdap", { "example.com", "8.8.8.8", "AS13335" }))
        return
    end
    ctx:edit(ui.wait("RDAP lookup for " .. query .. "…"))

    local data, problem = rdap_fetch(ctx, kind, value)
    if not data then
        if problem == "not_found" then
            ctx:edit(ui.warn("Not found", "No registration data for " .. ui.mono(query)
                .. ". The domain may be free, or its registry does not publish RDAP."))
        else
            ctx:edit(ui.err("RDAP lookup failed", ui.mono(ui.clip(v(problem), 300))))
        end
        return
    end

    local lines = {}
    local function add(label, text)
        if text ~= nil and text ~= "" then
            table.insert(lines, ui.kv(label, ui.clip(text, 200)))
        end
    end

    add("Name", ui.first(data.unicodeName, data.ldhName, data.name))
    add("Handle", data.handle)
    if kind == "ip" then
        if data.startAddress then
            add("Range", tostring(data.startAddress) .. " - " .. tostring(data.endAddress or "?"))
        end
        add("Type", data.type)
        add("Country", data.country)
    elseif kind == "autnum" then
        add("AS", data.startAutnum and ("AS" .. tostring(data.startAutnum)))
        add("Country", data.country)
    end

    local registrar = find_entity(data.entities, "registrar")
    if registrar then
        add("Registrar", ui.first(vcard_field(registrar, "fn"), registrar.handle))
    end
    local registrant = find_entity(data.entities, "registrant")
    if registrant then
        add("Registrant", ui.first(vcard_field(registrant, "org"), vcard_field(registrant, "fn")))
    end
    local abuse = find_entity(data.entities, "abuse")
    if abuse then
        add("Abuse", vcard_field(abuse, "email"))
    end

    local status = as_list(data.status)
    if #status > 0 then
        add("Status", table.concat(status, ", "))
    end

    for _, event in ipairs(type(data.events) == "table" and data.events or {}) do
        local label = EVENT_LABELS[tostring(event.eventAction)]
        if label and event.eventDate then
            add(label, tostring(event.eventDate):sub(1, 10))
        end
    end

    local servers = {}
    for _, ns in ipairs(type(data.nameservers) == "table" and data.nameservers or {}) do
        local name = ns.ldhName or ns.unicodeName
        if name then table.insert(servers, tostring(name):lower()) end
        if #servers >= 6 then break end
    end
    if #servers > 0 then
        add("Name servers", table.concat(servers, ", "))
    end

    if #lines == 0 then
        lines = { ui.italic("The registry returned no details") }
    end
    ctx:edit(ui.card("📇", "RDAP " .. query, lines))
end

return M
