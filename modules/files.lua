local M = {}

M.meta = { name = "files", version = "1.0" }

M.commands = {
    dl = "download_cmd",
    sendfile = "sendfile_cmd",
    urlupload = "urlupload_cmd",
    rename = "rename_cmd",
}

local function split2(args)
    local first, rest = args:match("^(%S+)%s*(.*)$")
    return first or "", rest or ""
end

function M.download_cmd(ctx, args)
    local name = args ~= "" and args or nil
    local path = ctx:download_replied_media(name)
    ctx:edit("**Downloaded**  \nPath: `" .. path .. "`")
end

function M.sendfile_cmd(ctx, args)
    local path, caption = split2(args)
    if path == "" then
        ctx:edit("**Usage**  \n`.sendfile <relative-path> [caption]`")
        return
    end
    ctx:send_file(path, caption)
end

function M.urlupload_cmd(ctx, args)
    local url, rest = split2(args)
    if url == "" then
        ctx:edit("**Usage**  \n`.urlupload <url> [file-name]`")
        return
    end
    ctx:edit("**URL Upload**  \nDownloading...")
    local path = ctx:download_url(url, rest ~= "" and rest or nil)
    ctx:edit("**URL Upload**  \nUploading `" .. path .. "`...")
    ctx:send_file(path, "")
end

function M.rename_cmd(ctx, args)
    local name = args ~= "" and args or nil
    local path = ctx:download_replied_media(name)
    ctx:edit("**Renamed copy saved**  \nPath: `" .. path .. "`")
end

return M
