-- Text animation commands.

local M = {}

M.meta = { name = "animate", version = "1.1" }

M.commands = {
    type = "type_cmd",
    scroll = "scroll_cmd",
    magic = "magic_cmd",
    heart = "heart_cmd",
}

-- Returns a table of individual UTF-8 characters (safe for any language).
local function chars(s)
    local t = {}
    for ch in s:gmatch(utf8.charpattern) do
        t[#t + 1] = ch
    end
    return t
end

-- Clamps text to 120 characters (by codepoint, not bytes).
local function clamp_text(args, usage)
    if args == "" then
        return nil, usage
    end
    local cs = chars(args)
    if #cs > 120 then
        local out = {}
        for i = 1, 120 do out[i] = cs[i] end
        return table.concat(out), nil
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
    for _, ch in ipairs(chars(text)) do
        current = current .. ch
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
    local padding = "          "
    local padded = chars(padding .. text .. padding)
    local window = 10
    for i = 1, math.max(1, #padded - window + 1) do
        local slice = {}
        for j = i, math.min(i + window - 1, #padded) do
            slice[#slice + 1] = padded[j]
        end
        ctx:edit("`" .. table.concat(slice) .. "`")
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
