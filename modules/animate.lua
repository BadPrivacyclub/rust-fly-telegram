-- Text animation commands.

local M = {}

M.meta = { name = "animate", version = "1.0" }

M.commands = {
    type = "type_cmd",
    scroll = "scroll_cmd",
    magic = "magic_cmd",
    heart = "heart_cmd",
}

local function clamp_text(args, usage)
    if args == "" then
        return nil, usage
    end
    if #args > 120 then
        return args:sub(1, 120), nil
    end
    return args, nil
end

function M.type_cmd(ctx, args)
    local text, err = clamp_text(args, "**Usage**  \n`.type <text>`")
    if err then
        ctx:edit(err)
        return
    end
    local current = ""
    for i = 1, #text do
        current = text:sub(1, i)
        ctx:edit(current)
        ctx:sleep_ms(90)
    end
end

function M.scroll_cmd(ctx, args)
    local text, err = clamp_text(args, "**Usage**  \n`.scroll <text>`")
    if err then
        ctx:edit(err)
        return
    end
    local padded = "          " .. text .. "          "
    for i = 1, math.max(1, #padded - 9) do
        ctx:edit("`" .. padded:sub(i, i + 9) .. "`")
        ctx:sleep_ms(180)
    end
end

function M.magic_cmd(ctx, args)
    local text, err = clamp_text(args, "**Usage**  \n`.magic <text>`")
    if err then
        ctx:edit(err)
        return
    end
    local frames = { ".", "..", "...", "* " .. text, "**" .. text .. "**", "`" .. text .. "`", text }
    for _, frame in ipairs(frames) do
        ctx:edit(frame)
        ctx:sleep_ms(350)
    end
end

function M.heart_cmd(ctx, args)
    local text = args ~= "" and args or "<3"
    local frames = { "<3", "</3", "<33", "<333", text }
    for _, frame in ipairs(frames) do
        ctx:edit(frame)
        ctx:sleep_ms(300)
    end
end

return M
