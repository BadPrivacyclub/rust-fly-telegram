-- Lightweight OSINT helpers.

local M = {}

M.meta = { name = "osint", version = "1.0" }

M.commands = {
    ip = "ip_cmd",
    domain = "domain_cmd",
    rdap = "rdap_cmd",
}

local function urlencode(value)
    return tostring(value):gsub("([^%w%-%._~])", function(ch)
        return string.format("%%%02X", string.byte(ch))
    end)
end

local function text(value)
    if value == nil then
        return "unknown"
    end
    return tostring(value)
end

function M.ip_cmd(ctx, args)
    local query = args:match("^(%S+)$")
    if not query then
        ctx:edit("**Usage**  \n`.ip <ip-or-host>`")
        return
    end

    local data = ctx:http_json_get("https://ipapi.co/" .. urlencode(query) .. "/json/")
    local out = "**IP OSINT**  \n"
        .. "IP: `" .. text(data.ip) .. "`  \n"
        .. "City: `" .. text(data.city) .. "`  \n"
        .. "Region: `" .. text(data.region) .. "`  \n"
        .. "Country: `" .. text(data.country_name) .. "`  \n"
        .. "ASN: `" .. text(data.asn) .. "`  \n"
        .. "Org: `" .. text(data.org) .. "`"
    ctx:edit(out)
end

function M.domain_cmd(ctx, args)
    local query = args:match("^(%S+)$")
    if not query then
        ctx:edit("**Usage**  \n`.domain <domain>`")
        return
    end

    local data = ctx:http_json_get("https://dns.google/resolve?name=" .. urlencode(query) .. "&type=A")
    local answers = {}
    if data.Answer then
        for _, answer in ipairs(data.Answer) do
            table.insert(answers, tostring(answer.data))
        end
    end
    if #answers == 0 then
        table.insert(answers, "no A records")
    end
    ctx:edit("**DNS**  \nDomain: `" .. query .. "`  \nA: `" .. table.concat(answers, ", ") .. "`")
end

function M.rdap_cmd(ctx, args)
    local query = args:match("^(%S+)$")
    if not query then
        ctx:edit("**Usage**  \n`.rdap <domain-or-ip>`")
        return
    end

    local kind = (query:match("^%d+%.") or query:find(":")) and "ip" or "domain"
    local data = ctx:http_json_get("https://rdap.org/" .. kind .. "/" .. urlencode(query))
    local handle = text(data.handle)
    local name = text(data.ldhName or data.name)
    local status = data.status and table.concat(data.status, ", ") or "unknown"
    ctx:edit("**RDAP**  \nName: `" .. name .. "`  \nHandle: `" .. handle .. "`  \nStatus: `" .. status .. "`")
end

return M
