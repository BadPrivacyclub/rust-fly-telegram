-- Everyday utility commands.

local M = {}

M.meta = { name = "tools", version = "1.0" }

M.commands = {
    del = "del_cmd",
    id  = "id_cmd",
    sd  = "sd_cmd",
    ytdl = "ytdl_cmd",
}

function M.del_cmd(ctx, args)
    local count = tonumber(args)
    if not count or count < 1 then
        ctx:edit("**Usage**  \n`.del <count>`")
        return
    end

    ctx:delete_last_own(math.floor(count))
end

function M.id_cmd(ctx, args)
    ctx:edit(ctx:message_info())
end

function M.sd_cmd(ctx, args)
    local seconds, text = args:match("^(%d+)%s+(.+)$")
    seconds = tonumber(seconds)
    if not seconds or not text then
        ctx:edit("**Usage**  \n`.sd <seconds> <text>`")
        return
    end

    ctx:edit(text)
    ctx:sleep(math.floor(seconds))
    ctx:delete()
end

function M.ytdl_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.ytdl <url>`")
        return
    end

    ctx:run_term("yt-dlp " .. args)
end

return M
