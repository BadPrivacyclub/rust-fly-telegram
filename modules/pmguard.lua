-- PM security controls.

local M = {}

M.meta = { name = "pmguard", version = "1.0" }

M.commands = {
    pmguard = "pmguard_cmd",
}

local function split_action(args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    return action or "", rest or ""
end

local function csv_has(csv, value)
    csv = tostring(csv or "")
    for item in csv:gmatch("[^,]+") do
        if item:match("^%s*(.-)%s*$") == value then
            return true
        end
    end
    return false
end

local function csv_add(csv, value)
    csv = tostring(csv or "")
    if value == "" or csv_has(csv, value) then
        return csv
    end
    if csv == "" then
        return value
    end
    return csv .. "," .. value
end

local function csv_del(csv, value)
    local out = {}
    csv = tostring(csv or "")
    for item in csv:gmatch("[^,]+") do
        item = item:match("^%s*(.-)%s*$")
        if item ~= "" and item ~= value then
            table.insert(out, item)
        end
    end
    return table.concat(out, ",")
end

function M.pmguard_cmd(ctx, args)
    local action, rest = split_action(args)

    if action == "on" then
        ctx:db_set("pmguard.enabled", true)
        if rest ~= "" then
            ctx:db_set("pmguard.challenge_text", rest)
        end
        ctx:edit("**PM Guard**  \nStatus: `enabled`")
        return
    end

    if action == "off" then
        ctx:db_set("pmguard.enabled", false)
        ctx:edit("**PM Guard**  \nStatus: `disabled`")
        return
    end

    if action == "status" then
        local enabled = ctx:db_get("pmguard.enabled") and "enabled" or "disabled"
        local allow = tostring(ctx:db_get("pmguard.allow") or "")
        local deny = tostring(ctx:db_get("pmguard.deny") or "")
        ctx:edit("**PM Guard**  \nStatus: `" .. enabled .. "`  \nAllow: `" .. allow .. "`  \nDeny: `" .. deny .. "`")
        return
    end

    if action == "allow" or action == "deny" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.pmguard " .. action .. " <user_id>`")
            return
        end
        local key = "pmguard." .. action
        ctx:db_set(key, csv_add(ctx:db_get(key), rest))
        ctx:edit("**PM Guard**  \n`" .. rest .. "` added to `" .. action .. "`")
        return
    end

    if action == "unallow" or action == "undeny" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.pmguard " .. action .. " <user_id>`")
            return
        end
        local key = action == "unallow" and "pmguard.allow" or "pmguard.deny"
        ctx:db_set(key, csv_del(ctx:db_get(key), rest))
        ctx:edit("**PM Guard**  \n`" .. rest .. "` removed")
        return
    end

    if action == "text" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.pmguard text <challenge text>`")
            return
        end
        ctx:db_set("pmguard.challenge_text", rest)
        ctx:edit("**PM Guard**  \nChallenge text saved.")
        return
    end

    if action == "denytext" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.pmguard denytext <text>`")
            return
        end
        ctx:db_set("pmguard.deny_text", rest)
        ctx:edit("**PM Guard**  \nDeny text saved.")
        return
    end

    ctx:edit("**PM Guard**  \n`.pmguard on [text]`  \n`.pmguard off`  \n`.pmguard status`  \n`.pmguard allow|deny <user_id>`")
end

return M
