local M = {}

M.meta = { name = "automation", version = "1.0" }

M.commands = {
    gifts = "gifts_cmd",
    taskbot = "taskbot_cmd",
}

local function split_action(args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    return action or "status", rest or ""
end

local function bool_text(value)
    return value and "enabled" or "disabled"
end

local function json_escape(value)
    return tostring(value)
        :gsub("\\", "\\\\")
        :gsub("\"", "\\\"")
        :gsub("\n", "\\n")
        :gsub("\r", "\\r")
end

local function call_webhook(ctx, kind, payload)
    local url = tostring(ctx:db_get("automation.webhook_url") or "")
    if url == "" then
        return nil, "automation.webhook_url is not configured"
    end
    local body = "{\"kind\":\"" .. kind .. "\",\"payload\":\"" .. json_escape(payload) .. "\"}"
    return ctx:http_json_request("POST", url, body, {
        ["content-type"] = "application/json"
    }), nil
end

function M.gifts_cmd(ctx, args)
    local action, rest = split_action(args)

    if action == "on" then
        ctx:db_set("gifts.enabled", true)
        ctx:edit("**Gifts Automation**  \nStatus: `enabled`")
        return
    end
    if action == "off" then
        ctx:db_set("gifts.enabled", false)
        ctx:edit("**Gifts Automation**  \nStatus: `disabled`")
        return
    end
    if action == "dryrun" then
        local enabled = rest ~= "off"
        ctx:db_set("gifts.dry_run", enabled)
        ctx:edit("**Gifts Automation**  \nDry-run: `" .. bool_text(enabled) .. "`")
        return
    end
    if action == "budget" then
        local value = tonumber(rest)
        if not value or value < 0 then
            ctx:edit("**Usage**  \n`.gifts budget <amount>`")
            return
        end
        ctx:db_set("gifts.max_budget", math.floor(value))
        ctx:edit("**Gifts Automation**  \nBudget: `" .. tostring(math.floor(value)) .. "`")
        return
    end
    if action == "filter" then
        ctx:db_set("gifts.filter", rest)
        ctx:edit("**Gifts Automation**  \nFilter saved.")
        return
    end
    if action == "webhook" then
        ctx:db_set("automation.webhook_url", rest)
        ctx:edit("**Automation**  \nWebhook: `" .. (rest ~= "" and rest or "not set") .. "`")
        return
    end
    if action == "test" then
        local result, err = call_webhook(ctx, "gift_test", rest)
        if err then
            ctx:edit("**Automation**  \n" .. err)
            return
        end
        ctx:edit("**Automation**  \nWebhook status: `" .. tostring(result.status or "ok") .. "`")
        return
    end

    local enabled = bool_text(ctx:db_get("gifts.enabled"))
    local dry = ctx:db_get("gifts.dry_run")
    if dry == nil then
        dry = true
    end
    ctx:edit("**Gifts Automation**  \nStatus: `" .. enabled .. "`  \nDry-run: `" .. bool_text(dry) .. "`  \nBudget: `" .. tostring(ctx:db_get("gifts.max_budget") or 0) .. "`  \nFilter: `" .. tostring(ctx:db_get("gifts.filter") or "") .. "`")
end

function M.taskbot_cmd(ctx, args)
    local action, rest = split_action(args)
    if action == "webhook" then
        ctx:db_set("automation.webhook_url", rest)
        ctx:edit("**Task Automation**  \nWebhook saved.")
        return
    end
    if action == "run" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.taskbot run <payload>`")
            return
        end
        local result, err = call_webhook(ctx, "task", rest)
        if err then
            ctx:edit("**Task Automation**  \n" .. err)
            return
        end
        ctx:edit("**Task Automation**  \nStatus: `" .. tostring(result.status or "ok") .. "`")
        return
    end
    ctx:edit("**Task Automation**  \n`.taskbot webhook <url>`  \n`.taskbot run <payload>`  \n`.gifts status|on|off|dryrun|budget|filter|test`")
end

return M
