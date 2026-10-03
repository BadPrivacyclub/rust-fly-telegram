local M = {}

M.meta = { name = "utils", version = "1.0" }

M.commands = {
    weather = "weather_cmd",
    cur = "currency_cmd",
    qr = "qr_cmd",
    calc = "calc_cmd",
    paste = "paste_cmd",
}

M.help = {
    category = "utils",
    description = "Everyday helpers: weather, currency, QR codes, calculator, paste",
    commands = {
        weather = { args = "[city]", desc = "Current weather and a short forecast" },
        cur = { args = "<amount> <from> <to>", desc = "Convert currencies, e.g. 100 usd eur" },
        qr = { args = "<text>", desc = "QR code image for text or a link (or reply)" },
        calc = { args = "<expression>", desc = "Calculator: + - * / ^ % ( ) sqrt sin cos log pi" },
        paste = { args = "[text]", desc = "Upload text (or the replied message) to paste.rs" },
    },
}

M.config = {
    { key = "city", type = "string", default = "", description = "Default city for .weather" },
    { key = "units", type = "select", options = { "metric", "imperial" }, default = "metric", description = "Weather units" },
}

local function urlencode(value)
    return (tostring(value):gsub("([^%w%-%._~])", function(ch)
        return string.format("%%%02X", string.byte(ch))
    end))
end

local function text_or_reply(ctx, args)
    if args ~= "" then
        return args
    end
    return ctx:replied_text()
end

-- Weather ------------------------------------------------------------------

function M.weather_cmd(ctx, args)
    local city = args ~= "" and args or tostring(ctx:cfg("city") or "")
    if city == "" then
        ctx:edit(ui.usage(ctx:prefix(), "weather", { "<city>" }, "Or set a default: " .. ctx:prefix() .. "cfg utils city Berlin"))
        return
    end
    ctx:edit(ui.wait("Fetching weather…"))
    local data = ctx:http_json_get("https://wttr.in/" .. urlencode(city) .. "?format=j1")
    local current = data.current_condition and data.current_condition[1]
    if not current then
        ctx:edit(ui.err("No weather data", ui.mono(city)))
        return
    end
    local imperial = ctx:cfg("units") == "imperial"
    local temp = imperial and (current.temp_F .. "°F") or (current.temp_C .. "°C")
    local feels = imperial and (current.FeelsLikeF .. "°F") or (current.FeelsLikeC .. "°C")
    local wind = imperial and (current.windspeedMiles .. " mph") or (current.windspeedKmph .. " km/h")
    local desc = current.weatherDesc and current.weatherDesc[1] and current.weatherDesc[1].value or ""
    local area = data.nearest_area and data.nearest_area[1]
    local place = city
    if area and area.areaName and area.areaName[1] then
        place = area.areaName[1].value
        if area.country and area.country[1] then
            place = place .. ", " .. area.country[1].value
        end
    end
    local lines = {
        ui.italic(desc),
        ui.kv("Temperature", temp .. " (feels " .. feels .. ")"),
        ui.kv("Humidity", tostring(current.humidity) .. "%"),
        ui.kv("Wind", wind),
    }
    for index, day in ipairs(data.weather or {}) do
        if index > 3 then break end
        local low = imperial and day.mintempF or day.mintempC
        local high = imperial and day.maxtempF or day.maxtempC
        table.insert(lines, ui.mono(day.date) .. "  " .. low .. "…" .. high .. (imperial and "°F" or "°C"))
    end
    ctx:edit(ui.card("🌤", place, lines))
end

-- Currency -----------------------------------------------------------------

function M.currency_cmd(ctx, args)
    local amount, from, to = args:match("^([%d%.,]+)%s+(%a%a%a)%s+(%a%a%a)$")
    if not amount then
        from, to = args:match("^(%a%a%a)%s+(%a%a%a)$")
        amount = from and "1" or nil
    end
    if not amount then
        ctx:edit(ui.usage(ctx:prefix(), "cur", { "100 usd eur", "eur rub" }))
        return
    end
    amount = tonumber((amount:gsub(",", ".")))
    from, to = from:upper(), to:upper()
    local data = ctx:http_json_get("https://open.er-api.com/v6/latest/" .. from)
    if data.result ~= "success" or not data.rates or not data.rates[to] then
        ctx:edit(ui.err("Unknown currency", ui.mono(from .. " → " .. to)))
        return
    end
    local rate = data.rates[to]
    ctx:edit(ui.card("💱", "Currency", {
        ui.mono(string.format("%.2f %s = %.2f %s", amount, from, amount * rate, to)),
        ui.italic(string.format("1 %s = %.4f %s", from, rate, to)),
    }))
end

-- QR code ------------------------------------------------------------------

function M.qr_cmd(ctx, args)
    local text = text_or_reply(ctx, args)
    if text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "qr", { "<text or link>" }, "Or reply to a message"))
        return
    end
    ctx:edit(ui.wait("Generating QR code…"))
    local url = "https://api.qrserver.com/v1/create-qr-code/?size=512x512&margin=12&data=" .. urlencode(text)
    local path = ctx:download_url(url, "qr-" .. tostring(ctx:now_ms()) .. ".png")
    ctx:send_file(path, "")
    ctx:delete()
end

-- Calculator ---------------------------------------------------------------
-- A small recursive-descent parser: no load(), so expressions cannot run code.

local functions = {
    sqrt = math.sqrt, abs = math.abs, floor = math.floor, ceil = math.ceil,
    sin = math.sin, cos = math.cos, tan = math.tan, log = math.log, exp = math.exp,
    round = function(x) return math.floor(x + 0.5) end,
}
local constants = { pi = math.pi, e = math.exp(1) }

local function tokenize(expr)
    local tokens = {}
    local i = 1
    while i <= #expr do
        local ch = expr:sub(i, i)
        if ch:match("%s") then
            i = i + 1
        elseif ch:match("[%d%.]") then
            local number = expr:match("^%d*%.?%d+[eE][%+%-]?%d+", i) or expr:match("^%d*%.?%d*", i)
            table.insert(tokens, { kind = "num", value = tonumber(number) })
            if not tokens[#tokens].value then error("bad number " .. number) end
            i = i + #number
        elseif ch:match("%a") then
            local name = expr:match("^%a+", i)
            table.insert(tokens, { kind = "name", value = name:lower() })
            i = i + #name
        elseif ch:match("[%+%-%*/%^%%%(%)]") then
            table.insert(tokens, { kind = "op", value = ch })
            i = i + 1
        else
            error("unexpected '" .. ch .. "'")
        end
    end
    return tokens
end

local function evaluate(expr)
    local tokens = tokenize(expr)
    local pos = 1
    local function peek() return tokens[pos] end
    local function take() pos = pos + 1; return tokens[pos - 1] end
    local function is_op(value) local t = peek(); return t and t.kind == "op" and t.value == value end

    local parse_sum, parse_power

    local function parse_atom()
        local token = take()
        if not token then error("unexpected end") end
        if token.kind == "num" then return token.value end
        if token.kind == "op" and token.value == "(" then
            local value = parse_sum()
            if not is_op(")") then error("missing )") end
            take()
            return value
        end
        -- Unary minus binds looser than ^, so -2^2 is -4 as in standard notation.
        if token.kind == "op" and token.value == "-" then return -parse_power() end
        if token.kind == "name" then
            if constants[token.value] then return constants[token.value] end
            local fn = functions[token.value]
            if not fn then error("unknown name " .. token.value) end
            return fn(parse_atom())
        end
        error("unexpected " .. tostring(token.value))
    end

    parse_power = function()
        local base = parse_atom()
        if is_op("^") then
            take()
            return base ^ parse_power()
        end
        return base
    end

    local function parse_product()
        local value = parse_power()
        while is_op("*") or is_op("/") or is_op("%") do
            local op = take().value
            local rhs = parse_power()
            if op == "*" then value = value * rhs
            elseif op == "/" then value = value / rhs
            else value = value % rhs end
        end
        return value
    end

    parse_sum = function()
        local value = parse_product()
        while is_op("+") or is_op("-") do
            local op = take().value
            local rhs = parse_product()
            value = op == "+" and value + rhs or value - rhs
        end
        return value
    end

    local result = parse_sum()
    if pos <= #tokens then error("unexpected " .. tostring(tokens[pos].value)) end
    return result
end

M._evaluate = evaluate

function M.calc_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "calc", { "2 * (3 + 4) ^ 2", "sqrt(2) * pi" }))
        return
    end
    local ok, result = pcall(evaluate, args)
    if not ok then
        ctx:edit(ui.err("Cannot calculate", tostring(result):gsub("^.-:%d+: ", "")))
        return
    end
    local shown = result
    if math.type(result) == "float" and result == math.floor(result) and math.abs(result) < 1e15 then
        shown = math.floor(result)
    elseif math.type(result) == "float" then
        shown = string.format("%.10g", result)
    end
    ctx:edit("🧮 " .. ui.mono(args) .. " = " .. ui.bold(tostring(shown)))
end

-- Paste --------------------------------------------------------------------

function M.paste_cmd(ctx, args)
    local text = text_or_reply(ctx, args)
    if text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "paste", { "<text>" }, "Or reply to a message"))
        return
    end
    ctx:edit(ui.wait("Uploading…"))
    local url = ctx:http_request("POST", "https://paste.rs/", text, { ["content-type"] = "text/plain; charset=utf-8" })
    url = tostring(url):gsub("%s+$", "")
    ctx:edit(ui.ok("Pasted", url))
end

return M
