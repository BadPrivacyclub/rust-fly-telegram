local M = {}

M.meta = { name = "automation", version = "2.0" }

M.help = {
    category = "automation",
    description = "Dry-run gift and task automation hooks that call your webhook",
    commands = {
        gifts = { args = "status|on|off|dryrun [off]|budget <n>|filter <text>|webhook <url>|test [payload]", desc = "Gift automation settings" },
        taskbot = { args = "webhook <url> | run <payload>", desc = "Send a task to your webhook" },
    },
}

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

local function call_webhook(ctx, kind, payload)
    local url = tostring(ctx:db_get("automation.webhook_url") or "")
    if url == "" then
        return nil, "automation.webhook_url is not configured"
    end
    local body = json.encode({ kind = kind, payload = payload })
    return ctx:http_json_request("POST", url, body, {
        ["content-type"] = "application/json"
    }), nil
end

local function status(ctx, title, lines)
    ctx:edit(ui.card("🤖", title, lines))
end

function M.gifts_cmd(ctx, args)
    local action, rest = split_action(args)

    if action == "on" or action == "off" then
        ctx:db_set("gifts.enabled", action == "on")
        ctx:edit(ui.toggle("Gift automation", action == "on"))
        return
    end
    if action == "dryrun" then
        local enabled = rest ~= "off"
        ctx:db_set("gifts.dry_run", enabled)
        status(ctx, "Gift automation", { ui.kv("Dry-run", bool_text(enabled)) })
        return
    end
    if action == "budget" then
        local value = tonumber(rest)
        if not value or value < 0 then
            ctx:edit(ui.usage(ctx:prefix(), "gifts", { "budget <amount>" }))
            return
        end
        ctx:db_set("gifts.max_budget", math.floor(value))
        status(ctx, "Gift automation", { ui.kv("Budget", math.floor(value)) })
        return
    end
    if action == "filter" then
        ctx:db_set("gifts.filter", rest)
        ctx:edit(ui.ok("Filter saved"))
        return
    end
    if action == "webhook" then
        ctx:db_set("automation.webhook_url", rest)
        status(ctx, "Automation", { ui.kv("Webhook", rest ~= "" and rest or "not set") })
        return
    end
    if action == "test" then
        local result, err = call_webhook(ctx, "gift_test", rest)
        if err then
            ctx:edit(ui.warn("Automation", err))
            return
        end
        status(ctx, "Automation", { ui.kv("Webhook status", result.status or "ok") })
        return
    end

    local dry = ctx:db_get("gifts.dry_run")
    if dry == nil then
        dry = true
    end
    status(ctx, "Gift automation", {
        ui.kv("Status", bool_text(ctx:db_get("gifts.enabled"))),
        ui.kv("Dry-run", bool_text(dry)),
        ui.kv("Budget", ctx:db_get("gifts.max_budget") or 0),
        ui.kv("Filter", ctx:db_get("gifts.filter") or "—"),
    })
end

function M.taskbot_cmd(ctx, args)
    local action, rest = split_action(args)
    if action == "webhook" then
        ctx:db_set("automation.webhook_url", rest)
        ctx:edit(ui.ok("Webhook saved"))
        return
    end
    if action == "run" then
        if rest == "" then
            ctx:edit(ui.usage(ctx:prefix(), "taskbot", { "run <payload>" }))
            return
        end
        local result, err = call_webhook(ctx, "task", rest)
        if err then
            ctx:edit(ui.warn("Task automation", err))
            return
        end
        status(ctx, "Task automation", { ui.kv("Status", result.status or "ok") })
        return
    end
    ctx:edit(ui.usage(ctx:prefix(), "taskbot", { "webhook <url>", "run <payload>" }))
end

return M
