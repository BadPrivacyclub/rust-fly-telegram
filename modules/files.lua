local M = {}

M.meta = { name = "files", version = "2.0" }

M.commands = {
    dl = "download_cmd",
    sendfile = "sendfile_cmd",
    urlupload = "urlupload_cmd",
    rename = "rename_cmd",
}

M.help = {
    category = "files",
    description = "Download replied media, upload files and URLs",
    commands = {
        dl = { args = "[file-name] (reply)", desc = "Save replied media to data/downloads" },
        sendfile = { args = "<relative-path> [caption]", desc = "Upload a local file" },
        urlupload = { args = "<url> [file-name]", desc = "Download a URL and upload it to the chat" },
        rename = { args = "<new-name> (reply)", desc = "Re-upload replied media with a new name" },
    },
}

function M.download_cmd(ctx, args)
    ctx:edit(ui.wait("Downloading…"))
    local path = ctx:download_replied_media(args ~= "" and args or nil)
    ctx:edit(ui.ok("Downloaded", ui.mono(path)))
end

function M.sendfile_cmd(ctx, args)
    local path, caption = ui.split(args)
    if path == "" then
        ctx:edit(ui.usage(ctx:prefix(), "sendfile", { "<relative-path> [caption]" }))
        return
    end
    ctx:edit(ui.wait("Uploading " .. path .. "…"))
    ctx:send_file(path, caption)
    ctx:delete()
end

function M.urlupload_cmd(ctx, args)
    local url, name = ui.split(args)
    if url == "" then
        ctx:edit(ui.usage(ctx:prefix(), "urlupload", { "<url> [file-name]" }))
        return
    end
    ctx:edit(ui.wait("Downloading…"))
    local path = ctx:download_url(url, name ~= "" and name or nil)
    ctx:edit(ui.wait("Uploading…"))
    ctx:send_file(path, "")
    ctx:delete()
end

function M.rename_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "rename", { "<new-name>" }, "Reply to a file"))
        return
    end
    ctx:edit(ui.wait("Renaming…"))
    local path = ctx:download_replied_media(args)
    ctx:send_file(path, "")
    ctx:delete()
end

return M
