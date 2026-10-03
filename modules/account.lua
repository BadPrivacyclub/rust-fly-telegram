local M = {}

M.commands = {
    acc = "acc_cmd",
    cleanup = "cleanup_cmd",
    readall = "readall_cmd",
    archive = "archive_cmd",
    muteall = "muteall_cmd",
    export = "export_cmd",
    jobs = "jobs_cmd",
    profile = "profile_cmd",
}

M.help = {
    category = "security",
    description = "Account tools: smart cleanup, bulk actions, profile, background tasks",
    commands = {
        acc = "Account overview: chats, contacts, unread",
        cleanup = { args = "<contacts|groups|channels|chats|bots|all> [inactive=30] [keep=@a,123] [revoke] [admins] [pinned] [archived] | run <code>", desc = "Preview, then run a cleanup" },
        readall = "Mark every chat as read",
        archive = { args = "[days]", desc = "Archive chats inactive for N days (default 30)" },
        muteall = "Mute every chat",
        export = "Save contacts and chats to backups/exports",
        jobs = { args = "[cancel <id>]", desc = "Background tasks and their progress" },
        profile = { args = "name <first> [last] | bio <text> | username <name>", desc = "Change your profile" },
    },
}

local TARGETS = {
    contacts = "contacts", contact = "contacts",
    groups = "groups", group = "groups",
    channels = "channels", channel = "channels",
    chats = "private_chats", private = "private_chats", pm = "private_chats",
    bots = "bots", bot = "bots",
}

local function started(ctx, title, id)
    ctx:edit(ui.card("⏳", title, {
        ui.kv("Task", id),
        ui.italic("Progress: " .. ctx:prefix() .. "jobs, or the Accounts page in the panel"),
    }))
end

function M.acc_cmd(ctx, args)
    ctx:edit(ui.wait("Counting chats…"))
    local acc = ctx:account()
    local c = ctx:account_counts()
    ctx:edit(ui.card(ui.icons.user, acc.name, {
        ui.kv("ID", acc.id),
        ui.kv("Private chats", c.users),
        ui.kv("Bots", c.bots),
        ui.kv("Groups", c.groups),
        ui.kv("Channels", c.channels),
        ui.kv("Contacts", c.contacts),
        ui.kv("Archived", c.archived),
        ui.kv("Unread", c.unread),
    }))
end

local function parse_cleanup(args)
    local options = { keep_ids = {}, keep_usernames = {} }
    local any = false
    for word in args:gmatch("%S+") do
        local lower = word:lower()
        local key, value = lower:match("^(%w+)=(.+)$")
        if lower == "all" then
            for _, field in pairs(TARGETS) do options[field] = true end
            any = true
        elseif TARGETS[lower] then
            options[TARGETS[lower]] = true
            any = true
        elseif key == "inactive" then
            options.inactive_days = tonumber(value)
        elseif key == "keep" then
            for item in word:sub(6):gmatch("[^,]+") do
                local id = tonumber(item)
                if id then
                    table.insert(options.keep_ids, math.tointeger(id))
                else
                    table.insert(options.keep_usernames, item)
                end
            end
        elseif lower == "revoke" then
            options.revoke = true
        elseif lower == "admins" then
            options.keep_admin = false
        elseif lower == "pinned" then
            options.keep_pinned = false
        elseif lower == "archived" then
            options.archived_only = true
        else
            return nil, "Unknown option: " .. word
        end
    end
    if not any then
        return nil, nil
    end
    return options, nil
end

function M.cleanup_cmd(ctx, args)
    local first, rest = ui.split(args)
    if first == "run" then
        if rest == "" then
            ctx:edit(ui.usage(ctx:prefix(), "cleanup", { "run <code>" }))
            return
        end
        local ok, id = pcall(function() return ctx:cleanup_run(rest) end)
        if not ok then
            ctx:edit(ui.err("Cleanup not started", tostring(id)))
            return
        end
        started(ctx, "Cleanup started", id)
        return
    end

    local options, err = parse_cleanup(args)
    if not options then
        ctx:edit(ui.card("🧹", "Smart cleanup", {
            err and (ui.icons.err .. " " .. err) or nil,
            "Targets: " .. ui.mono("contacts groups channels chats bots all"),
            "Filters: " .. ui.mono("inactive=30") .. " " .. ui.mono("keep=@friend,123") .. " " .. ui.mono("archived"),
            "Also: " .. ui.mono("revoke") .. " (delete chats for both sides), " .. ui.mono("admins") .. ", " .. ui.mono("pinned") .. " (include them)",
            "",
            ui.mono(ctx:prefix() .. "cleanup groups channels inactive=60"),
            "",
            ui.italic("You always see a preview first. Chats and channels you own are never touched."),
        }))
        return
    end

    ctx:edit(ui.wait("Building preview…"))
    local p = ctx:cleanup_preview(options)
    if p.total == 0 then
        ctx:edit(ui.info("Nothing to clean", tostring(p.kept) .. " chat(s) kept by your filters"))
        return
    end
    local lines = {
        ui.kv("Contacts", p.contacts),
        ui.kv("Private chats", p.users),
        ui.kv("Bots", p.bots),
        ui.kv("Groups", p.groups),
        ui.kv("Channels", p.channels),
        ui.kv("Kept by filters", p.kept),
    }
    if p.owned > 0 then
        table.insert(lines, ui.kv("Owned (never touched)", p.owned))
    end
    if #p.sample > 0 then
        table.insert(lines, "")
        table.insert(lines, ui.italic(table.concat(p.sample, ", ") .. (p.total > #p.sample + p.contacts and " …" or "")))
    end
    table.insert(lines, "")
    table.insert(lines, ui.icons.warn .. " " .. ui.bold("This cannot be undone.") .. " An export is saved first.")
    table.insert(lines, "Run: " .. ui.mono(ctx:prefix() .. "cleanup run " .. p.code))
    ctx:edit(ui.card("🧹", "Cleanup preview · " .. p.total .. " item(s)", lines, "The code expires in 15 minutes"))
end

local function bulk(ctx, action, title, days)
    local id = ctx:account_bulk(action, days)
    started(ctx, title, id)
end

function M.readall_cmd(ctx, args)
    bulk(ctx, "read_all", "Marking everything as read")
end

function M.archive_cmd(ctx, args)
    local days = tonumber(args) or 30
    bulk(ctx, "archive_inactive", "Archiving chats inactive for " .. days .. " days", math.floor(days))
end

function M.muteall_cmd(ctx, args)
    bulk(ctx, "mute_all", "Muting all chats")
end

function M.export_cmd(ctx, args)
    bulk(ctx, "export", "Exporting contacts and chats")
end

local STATUS_ICONS = { running = "⏳", done = "✅", failed = "❌", cancelled = "⏹" }

function M.jobs_cmd(ctx, args)
    local action, id = ui.split(args)
    if action == "cancel" and id ~= "" then
        if ctx:job_cancel(id) then
            ctx:edit(ui.ok("Cancelling", ui.mono(id)))
        else
            ctx:edit(ui.warn("No running task", ui.mono(id)))
        end
        return
    end
    local jobs = ctx:jobs()
    if #jobs == 0 then
        ctx:edit(ui.info("No tasks yet"))
        return
    end
    local lines = {}
    for _, job in ipairs(jobs) do
        local progress = ""
        if job.total > 0 then
            progress = " " .. ui.bar(job.done / job.total, 8) .. " " .. job.done .. "/" .. job.total
        end
        table.insert(lines, (STATUS_ICONS[job.status] or "•") .. " " .. ui.bold(job.title) .. " · " .. job.account .. progress)
        table.insert(lines, "    " .. ui.mono(job.id) .. " " .. ui.italic(job.summary or (job.log[#job.log] or "")))
    end
    ctx:edit(ui.card("📋", "Tasks", lines, ctx:prefix() .. "jobs cancel <id>"))
end

function M.profile_cmd(ctx, args)
    local field, value = ui.split(args)
    local update
    if field == "name" and value ~= "" then
        local first, last = value:match("^(%S+)%s*(.*)$")
        update = { first_name = first, last_name = last }
    elseif field == "bio" then
        update = { about = value }
    elseif field == "username" then
        update = { username = value }
    end
    if not update then
        ctx:edit(ui.usage(ctx:prefix(), "profile", { "name <first> [last]", "bio <text>", "username <name>" }))
        return
    end
    local ok, err = pcall(function() ctx:profile_update(update) end)
    if ok then
        ctx:edit(ui.ok("Profile updated"))
    else
        ctx:edit(ui.err("Profile not updated", tostring(err)))
    end
end

return M
