local M = {}

M.commands = {
    coin = "coin_cmd",
    dice = "dice_cmd",
    random = "random_cmd",
    countdown = "countdown_cmd",
    time = "time_cmd",
    pass = "pass_cmd",
    b64 = "b64_cmd",
    hash = "hash_cmd",
    uuid = "uuid_cmd",
    short = "short_cmd",
    id = "id_cmd",
}

M.help = {
    category = "utils",
    description = "Small handy tools: random picks, timers, world clock, passwords, encoding",
    commands = {
        coin = "Flip a coin",
        dice = { args = "[sides] [count]", desc = "Roll dice, e.g. 20 or 6 3" },
        random = { args = "<min> <max> | a, b, c", desc = "Random number or pick from a list" },
        countdown = { args = "<seconds> [text]", desc = "Live countdown (up to 60 s)" },
        time = { args = "[city or Area/City]", desc = "Current time in a city" },
        pass = { args = "[length]", desc = "Strong random password" },
        b64 = { args = "enc|dec <text>", desc = "Base64 encode or decode" },
        hash = { args = "<text>", desc = "SHA-256 of text or the replied message" },
        uuid = "Random UUID",
        short = { args = "<url>", desc = "Shorten a link with is.gd" },
        id = "IDs of this chat, message, and the replied sender",
    },
}

-- City names → IANA time zones for .time; anything with a slash is passed through.
local ZONES = {
    moscow = "Europe/Moscow", ["москва"] = "Europe/Moscow", london = "Europe/London",
    berlin = "Europe/Berlin", paris = "Europe/Paris", kyiv = "Europe/Kyiv", kiev = "Europe/Kyiv",
    minsk = "Europe/Minsk", warsaw = "Europe/Warsaw", istanbul = "Europe/Istanbul",
    dubai = "Asia/Dubai", tokyo = "Asia/Tokyo", seoul = "Asia/Seoul", beijing = "Asia/Shanghai",
    shanghai = "Asia/Shanghai", delhi = "Asia/Kolkata", almaty = "Asia/Almaty",
    tashkent = "Asia/Tashkent", tbilisi = "Asia/Tbilisi", yerevan = "Asia/Yerevan",
    novosibirsk = "Asia/Novosibirsk", vladivostok = "Asia/Vladivostok", ["new york"] = "America/New_York",
    nyc = "America/New_York", ["los angeles"] = "America/Los_Angeles", la = "America/Los_Angeles",
    chicago = "America/Chicago", toronto = "America/Toronto", sydney = "Australia/Sydney",
    ["sao paulo"] = "America/Sao_Paulo", utc = "UTC",
}

function M.coin_cmd(ctx, args)
    ctx:edit("🪙 " .. ui.italic("Flipping…"))
    ctx:sleep_ms(700)
    ctx:edit("🪙 " .. ui.bold(math.random(2) == 1 and "Heads" or "Tails"))
end

function M.dice_cmd(ctx, args)
    local sides, count = args:match("^(%d+)%s*(%d*)$")
    sides = math.max(2, math.min(tonumber(sides) or 6, 1000))
    count = math.max(1, math.min(tonumber(count) or 1, 20))
    local rolls, total = {}, 0
    for _ = 1, count do
        local value = math.random(sides)
        total = total + value
        table.insert(rolls, tostring(value))
    end
    local text = "🎲 d" .. sides .. ": " .. ui.bold(table.concat(rolls, " "))
    if count > 1 then
        text = text .. "  " .. ui.italic("= " .. total)
    end
    ctx:edit(text)
end

function M.random_cmd(ctx, args)
    local low, high = args:match("^(%-?%d+)%s+(%-?%d+)$")
    if low then
        low, high = tonumber(low), tonumber(high)
        if low > high then low, high = high, low end
        ctx:edit("🎰 " .. ui.bold(tostring(math.random(low, high))) .. "  " .. ui.italic(low .. "…" .. high))
        return
    end
    local items = {}
    for item in args:gmatch("[^,]+") do
        item = item:match("^%s*(.-)%s*$")
        if item ~= "" then table.insert(items, item) end
    end
    if #items < 2 then
        ctx:edit(ui.usage(ctx:prefix(), "random", { "1 100", "pizza, sushi, burgers" }))
        return
    end
    ctx:edit("🎯 " .. ui.bold(items[math.random(#items)]))
end

function M.countdown_cmd(ctx, args)
    local seconds, text = args:match("^(%d+)%s*(.*)$")
    seconds = tonumber(seconds)
    if not seconds or seconds < 1 then
        ctx:edit(ui.usage(ctx:prefix(), "countdown", { "10", "30 Lunch!" }))
        return
    end
    seconds = math.min(seconds, 60)
    for left = seconds, 1, -1 do
        ctx:edit("⏳ " .. ui.bold(tostring(left)) .. "  " .. ui.bar(left / seconds, 10))
        ctx:sleep(1)
    end
    ctx:edit("⏰ " .. ui.bold(text ~= "" and text or "Time's up!"))
end

function M.time_cmd(ctx, args)
    local query = args:lower():gsub("^%s+", ""):gsub("%s+$", "")
    if query == "" then query = "utc" end
    local zone = ZONES[query] or (args:find("/") and args) or nil
    if not zone then
        ctx:edit(ui.warn("Unknown city", "Use a city like " .. ui.mono("Berlin") .. " or a zone like " .. ui.mono("Europe/Berlin")))
        return
    end
    local data = ctx:http_json_get("https://timeapi.io/api/time/current/zone?timeZone=" .. zone)
    if not data.time then
        ctx:edit(ui.err("No time for " .. zone))
        return
    end
    ctx:edit("🕰 " .. ui.bold(data.time) .. "  " .. ui.italic(string.format("%s, %02d.%02d · %s", data.dayOfWeek or "", data.day or 0, data.month or 0, zone)))
end

function M.pass_cmd(ctx, args)
    local length = math.max(8, math.min(tonumber(args) or 20, 128))
    local sets = { "abcdefghijkmnopqrstuvwxyz", "ABCDEFGHJKLMNPQRSTUVWXYZ", "23456789", "!@#$%^&*-_=+?" }
    local all = table.concat(sets)
    local chars = {}
    -- At least one character from every set, then shuffle.
    for _, set in ipairs(sets) do
        local i = math.random(#set)
        table.insert(chars, set:sub(i, i))
    end
    while #chars < length do
        local i = math.random(#all)
        table.insert(chars, all:sub(i, i))
    end
    for i = #chars, 2, -1 do
        local j = math.random(i)
        chars[i], chars[j] = chars[j], chars[i]
    end
    ctx:edit("🔑 " .. ui.mono(table.concat(chars)))
end

local B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"

local function b64_encode(data)
    return ((data:gsub(".", function(c)
        local bits, byte = "", c:byte()
        for i = 8, 1, -1 do bits = bits .. (byte % 2 ^ i - byte % 2 ^ (i - 1) > 0 and "1" or "0") end
        return bits
    end) .. "0000"):gsub("%d%d%d?%d?%d?%d?", function(bits)
        if #bits < 6 then return "" end
        local n = 0
        for i = 1, 6 do n = n + (bits:sub(i, i) == "1" and 2 ^ (6 - i) or 0) end
        return B64:sub(n + 1, n + 1)
    end) .. ({ "", "==", "=" })[#data % 3 + 1])
end

local function b64_decode(data)
    data = data:gsub("[^" .. B64 .. "=]", "")
    return (data:gsub(".", function(c)
        if c == "=" then return "" end
        local bits, n = "", B64:find(c, 1, true) - 1
        for i = 6, 1, -1 do bits = bits .. (n % 2 ^ i - n % 2 ^ (i - 1) > 0 and "1" or "0") end
        return bits
    end):gsub("%d%d%d?%d?%d?%d?%d?%d?", function(bits)
        if #bits ~= 8 then return "" end
        local n = 0
        for i = 1, 8 do n = n + (bits:sub(i, i) == "1" and 2 ^ (8 - i) or 0) end
        return string.char(n)
    end))
end

M._b64_encode, M._b64_decode = b64_encode, b64_decode

function M.b64_cmd(ctx, args)
    local mode, text = ui.split(args)
    if text == "" then text = ctx:replied_text() end
    if (mode ~= "enc" and mode ~= "dec") or text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "b64", { "enc <text>", "dec <base64>" }))
        return
    end
    local result = mode == "enc" and b64_encode(text) or b64_decode(text)
    ctx:edit(ui.code(result))
end

function M.hash_cmd(ctx, args)
    local text = args ~= "" and args or ctx:replied_text()
    if text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "hash", { "<text>" }, "Or reply to a message"))
        return
    end
    ctx:edit("#️⃣ SHA-256  \n" .. ui.mono(ctx:sha256(text)))
end

function M.uuid_cmd(ctx, args)
    ctx:edit("🆔 " .. ui.mono(ctx:uuid()))
end

function M.short_cmd(ctx, args)
    if not args:match("^https?://") then
        ctx:edit(ui.usage(ctx:prefix(), "short", { "https://example.com/very/long/link" }))
        return
    end
    local encoded = args:gsub("([^%w%-%._~])", function(ch) return string.format("%%%02X", ch:byte()) end)
    local short = ctx:http_get("https://is.gd/create.php?format=simple&url=" .. encoded):gsub("%s+$", "")
    if not short:match("^https?://") then
        ctx:edit(ui.err("Could not shorten", short))
        return
    end
    ctx:edit("🔗 " .. short)
end

function M.id_cmd(ctx, args)
    local msg = ctx:message()
    local lines = {
        ui.kv("Chat", msg.chat_id),
        ui.kv("Message", msg.id),
    }
    if msg.sender_id then
        table.insert(lines, ui.kv("You", msg.sender_id))
    end
    local replied = ctx:replied()
    if replied then
        table.insert(lines, ui.kv("Replied message", replied.id))
        if replied.sender_id then
            table.insert(lines, ui.kv("Replied sender", replied.sender_id))
        end
    end
    ctx:edit(ui.card("🆔", "IDs", lines))
end

return M
