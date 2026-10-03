local M = {}

M.meta = { name = "tools", version = "2.0" }

M.commands = {
    del = "del_cmd",
    info = "info_cmd",
    sd = "sd_cmd",
    ytdl = "ytdl_cmd",
}

M.help = {
    category = "info",
    description = "Message tools: delete, chat and user info, self-destruct, yt-dlp",
    commands = {
        del = { args = "<count>", desc = "Delete your last N messages in this chat" },
        info = { args = "[reply]", desc = "Chat, user, DC, and group activity info" },
        sd = { args = "<seconds> <text>", desc = "Send a self-destructing message" },
        ytdl = { args = "<url> [yt-dlp options]", desc = "Download media with yt-dlp" },
    },
}

function M.del_cmd(ctx, args)
    local count = tonumber(args)
    if not count or count < 1 then
        ctx:edit(ui.usage(ctx:prefix(), "del", { "<count>" }))
        return
    end
    ctx:delete_last_own(math.min(math.floor(count), 500))
end

function M.info_cmd(ctx, args)
    ctx:edit(ui.wait("Collecting info…"))
    ctx:edit(ctx:message_info())
end

function M.sd_cmd(ctx, args)
    local seconds, text = args:match("^(%d+)%s+(.+)$")
    seconds = tonumber(seconds)
    if not seconds or not text then
        ctx:edit(ui.usage(ctx:prefix(), "sd", { "<seconds> <text>" }))
        return
    end
    ctx:edit(text)
    ctx:sleep(math.min(math.floor(seconds), 86400))
    ctx:delete()
end

function M.ytdl_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "ytdl", { "<url> [options]" }))
        return
    end
    -- Arguments go straight to the process without a shell, so they cannot chain commands.
    local argv = { "-P", "data/downloads", "--no-playlist" }
    for part in args:gmatch("%S+") do
        table.insert(argv, part)
    end
    ctx:run_process("yt-dlp", argv)
end

return M
