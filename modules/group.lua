local M = {}

M.meta = { name = "group", version = "1.0" }

M.commands = {
    cleanjoins = "cleanjoins_cmd",
    captcha = "captcha_cmd",
    group = "group_cmd",
}

local function bool_text(value)
    return value and "enabled" or "disabled"
end

function M.cleanjoins_cmd(ctx, args)
    local action = args:match("^(%S+)$") or ""
    if action == "on" then
        ctx:db_set("group.clean_joins.enabled", true)
        ctx:edit("**Clean Joins**  \nStatus: `enabled`")
        return
    end
    if action == "off" then
        ctx:db_set("group.clean_joins.enabled", false)
        ctx:edit("**Clean Joins**  \nStatus: `disabled`")
        return
    end
    if action == "status" or action == "" then
        ctx:edit("**Clean Joins**  \nStatus: `" .. bool_text(ctx:db_get("group.clean_joins.enabled")) .. "`")
        return
    end
    ctx:edit("**Usage**  \n`.cleanjoins on|off|status`")
end

function M.group_cmd(ctx, args)
    ctx:edit("**Group Tools**  \n`.cleanjoins on|off|status`: auto-delete join/leave service messages  \n`.captcha on|off|status|text <text>`: challenge new members")
end

function M.captcha_cmd(ctx, args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    action = action or "status"
    rest = rest or ""

    if action == "on" then
        ctx:db_set("group.captcha.enabled", true)
        ctx:edit("**Group CAPTCHA**  \nStatus: `enabled`")
        return
    end
    if action == "off" then
        ctx:db_set("group.captcha.enabled", false)
        ctx:edit("**Group CAPTCHA**  \nStatus: `disabled`")
        return
    end
    if action == "text" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.captcha text Welcome. Code: {code}`")
            return
        end
        ctx:db_set("group.captcha.text", rest)
        ctx:edit("**Group CAPTCHA**  \nText saved.")
        return
    end
    if action == "status" then
        ctx:edit("**Group CAPTCHA**  \nStatus: `" .. bool_text(ctx:db_get("group.captcha.enabled")) .. "`")
        return
    end
    ctx:edit("**Usage**  \n`.captcha on|off|status`  \n`.captcha text <text with {code}>`")
end

return M
