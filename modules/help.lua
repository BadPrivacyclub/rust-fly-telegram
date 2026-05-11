-- Help module: lists available commands.

local M = {}

M.meta = { name = "help", version = "1.0" }

M.commands = { help = "help_cmd" }

function M.help_cmd(ctx, args)
    local text = "**fly-telegram**\n\n"
        .. "**Commands**  \n"
        .. "`.ping` - check connectivity  \n"
        .. "`.eval <code>` - evaluate Lua expression  \n"
        .. "`.term <cmd>` - run shell command  \n"
        .. "`.install <file-or-url> [name]` - install Lua module  \n"
        .. "`.install` as reply - install replied `.lua` module  \n"
        .. "`.note set|get|clear` - manage a saved note  \n"
        .. "`.alias set|get|del` - manage text aliases  \n"
        .. "`.del <count>` - delete recent messages  \n"
        .. "`.info` - show chat, user, DC, and group activity info  \n"
        .. "`.sd <seconds> <text>` - self-destruct message  \n"
        .. "`.ytdl <url>` - run yt-dlp for a media URL  \n"
        .. "`.afk on [text]|off` - auto-reply when mentioned  \n"
        .. "`.autoread on|off` - mark incoming messages as read  \n"
        .. "`.antidelete on|off` - log deleted cached messages  \n"
        .. "`.restart` - restart the process  \n"
        .. "`.update` - pull changes and rebuild when needed  \n"
        .. "`.help` - show this message"
    ctx:edit(text)
end

return M
