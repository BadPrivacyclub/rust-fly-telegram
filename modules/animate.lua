local M = {}

M.meta = { name = "animate", version = "2.0" }

M.commands = {
    type = "type_cmd",
    scroll = "scroll_cmd",
    magic = "magic_cmd",
    heart = "heart_cmd",
}

M.help = {
    category = "fun",
    description = "Text animations made by editing the message",
    commands = {
        type = { args = "<text>", desc = "Typewriter effect" },
        scroll = { args = "<text>", desc = "Scrolling marquee" },
        magic = { args = "<text>", desc = "Reveal with formatting" },
        heart = { args = "[text]", desc = "Beating heart" },
    },
}

-- Iterate over Unicode code points to avoid splitting UTF-8 sequences.
local function chars(s)
    local t = {}
    for ch in s:gmatch(utf8.charpattern) do
        t[#t + 1] = ch
    end
    return t
end

-- Limit by code point so multibyte text is never truncated mid-character.
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
    local text, err = clamp_text(args, ui.usage(ctx:prefix(), "type", { "<text>" }))
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
    local text, err = clamp_text(args, ui.usage(ctx:prefix(), "scroll", { "<text>" }))
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
    local text, err = clamp_text(args, ui.usage(ctx:prefix(), "magic", { "<text>" }))
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
    local text = args ~= "" and args or "❤️"
    local frames = { "🤍", "🩷", "❤️", "🧡", "💛", "💚", "💙", "💜", "❤️‍🔥", text }
    for _, frame in ipairs(frames) do
        ctx:edit(frame)
        ctx:sleep_ms(400)
    end
end

return M
