local M = {}

M.meta = { name = "utils", version = "1.1" }

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
    { key = "city", type = "string", default = "", description = "Default city for .weather (set automatically the first time you ask for a city)" },
    { key = "units", type = "select", options = { "metric", "imperial" }, default = "metric", description = "Weather units" },
    { key = "days", type = "number", default = 3, description = "Forecast days for .weather (1-7)" },
}

local function urlencode(value)
    return ui.urlencode(value)
end

local function text_or_reply(ctx, args)
    if args ~= "" then
        return args
    end
    return ctx:replied_text()
end

-- I use Open-Meteo (free, no key) with wttr.in as a fallback.

local WMO = {
    [0] = { "☀️", "Clear sky", "Ясно" },
    [1] = { "🌤", "Mainly clear", "Преимущественно ясно" },
    [2] = { "⛅", "Partly cloudy", "Переменная облачность" },
    [3] = { "☁️", "Overcast", "Пасмурно" },
    [45] = { "🌫", "Fog", "Туман" },
    [48] = { "🌫", "Rime fog", "Изморозь" },
    [51] = { "🌦", "Light drizzle", "Слабая морось" },
    [53] = { "🌦", "Drizzle", "Морось" },
    [55] = { "🌧", "Dense drizzle", "Сильная морось" },
    [56] = { "🌧", "Freezing drizzle", "Ледяная морось" },
    [57] = { "🌧", "Freezing drizzle", "Ледяная морось" },
    [61] = { "🌦", "Light rain", "Небольшой дождь" },
    [63] = { "🌧", "Rain", "Дождь" },
    [65] = { "🌧", "Heavy rain", "Сильный дождь" },
    [66] = { "🌧", "Freezing rain", "Ледяной дождь" },
    [67] = { "🌧", "Freezing rain", "Ледяной дождь" },
    [71] = { "🌨", "Light snow", "Небольшой снег" },
    [73] = { "🌨", "Snow", "Снег" },
    [75] = { "❄️", "Heavy snow", "Сильный снег" },
    [77] = { "🌨", "Snow grains", "Снежная крупа" },
    [80] = { "🌦", "Rain showers", "Ливень" },
    [81] = { "🌧", "Rain showers", "Ливень" },
    [82] = { "⛈", "Violent showers", "Сильный ливень" },
    [85] = { "🌨", "Snow showers", "Снегопад" },
    [86] = { "❄️", "Heavy snow showers", "Сильный снегопад" },
    [95] = { "⛈", "Thunderstorm", "Гроза" },
    [96] = { "⛈", "Thunderstorm with hail", "Гроза с градом" },
    [99] = { "⛈", "Thunderstorm with hail", "Гроза с градом" },
}

local function describe(code, ru)
    local entry = WMO[math.tointeger(tonumber(code)) or -1]
    if not entry then
        return "🌡", ru and "Нет данных" or "Unknown"
    end
    return entry[1], ru and entry[3] or entry[2]
end

local function open_meteo(ctx, city, imperial, days, ru)
    local lat, lon = city:match("^%s*(%-?%d+%.?%d*)%s*[,; ]%s*(%-?%d+%.?%d*)%s*$")
    local place
    if lat then
        place = ui.num(lat, 3) .. ", " .. ui.num(lon, 3)
    else
        local geo = ctx:fetch("https://geocoding-api.open-meteo.com/v1/search", {
            query = { name = city, count = 1, language = ru and "ru" or "en", format = "json" },
            timeout = 15,
        })
        if not geo.ok then
            return nil, geo.error
        end
        local hit = ui.get(geo, "json.results.1")
        if not hit then
            return nil, "not_found"
        end
        lat, lon = hit.latitude, hit.longitude
        place = tostring(hit.name or city)
        local region = hit.admin1 ~= hit.name and hit.admin1 or nil
        local extra = {}
        for _, part in ipairs({ region, hit.country }) do
            if part and part ~= "" then table.insert(extra, tostring(part)) end
        end
        if #extra > 0 then
            place = place .. ", " .. table.concat(extra, ", ")
        end
    end

    local query = {
        latitude = lat, longitude = lon, timezone = "auto", forecast_days = days,
        current = "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,precipitation",
        daily = "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max",
    }
    if imperial then
        query.temperature_unit = "fahrenheit"
        query.wind_speed_unit = "mph"
        query.precipitation_unit = "inch"
    end
    local res = ctx:fetch("https://api.open-meteo.com/v1/forecast", { query = query, timeout = 15 })
    local current = ui.get(res, "json.current")
    if not res.ok or type(current) ~= "table" then
        return nil, ui.first(ui.get(res, "json.reason"), res.error, "no forecast data")
    end

    local daily = ui.get(res, "json.daily", {})
    local forecast = {}
    for index, date in ipairs(daily.time or {}) do
        table.insert(forecast, {
            date = date,
            code = ui.get(daily, "weather_code." .. index),
            low = ui.get(daily, "temperature_2m_min." .. index),
            high = ui.get(daily, "temperature_2m_max." .. index),
            rain = ui.get(daily, "precipitation_probability_max." .. index),
        })
    end
    return {
        place = place,
        code = current.weather_code,
        temp = current.temperature_2m,
        feels = current.apparent_temperature,
        humidity = current.relative_humidity_2m,
        wind = current.wind_speed_10m,
        forecast = forecast,
    }
end

local function wttr(ctx, city, imperial, days, ru)
    local res = ctx:fetch("https://wttr.in/" .. urlencode(city), {
        query = { format = "j1", lang = ru and "ru" or "en" },
        timeout = 20,
    })
    local current = ui.get(res, "json.current_condition.1")
    if not res.ok or type(current) ~= "table" then
        if res.status == 404 then
            return nil, "not_found"
        end
        return nil, ui.first(res.error, "no weather data")
    end
    local area = ui.get(res, "json.nearest_area.1")
    local place = ui.get(area, "areaName.1.value") or city
    local country = ui.get(area, "country.1.value")
    if country and country ~= "" then
        place = place .. ", " .. country
    end
    local desc = ui.first(ru and ui.get(current, "lang_ru.1.value"), ui.get(current, "weatherDesc.1.value"))
    local forecast = {}
    for index, day in ipairs(ui.get(res, "json.weather", {})) do
        if index > days then break end
        table.insert(forecast, {
            date = day.date,
            low = imperial and day.mintempF or day.mintempC,
            high = imperial and day.maxtempF or day.maxtempC,
        })
    end
    return {
        place = place,
        desc = desc,
        temp = imperial and current.temp_F or current.temp_C,
        feels = imperial and current.FeelsLikeF or current.FeelsLikeC,
        humidity = current.humidity,
        wind = imperial and current.windspeedMiles or current.windspeedKmph,
        forecast = forecast,
    }
end

local WEEKDAYS = {
    en = { "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat" },
    ru = { "Вс", "Пн", "Вт", "Ср", "Чт", "Пт", "Сб" },
}

-- I format "2026-10-07" as "Wed 07.10".
local function day_label(date, ru)
    local y, m, d = tostring(date or ""):match("^(%d+)%-(%d+)%-(%d+)")
    if not y then return tostring(date or "") end
    y, m, d = tonumber(y), tonumber(m), tonumber(d)
    -- I use Sakamoto's day-of-week algorithm.
    local t = { 0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4 }
    local yy = m < 3 and y - 1 or y
    local dow = (yy + yy // 4 - yy // 100 + yy // 400 + t[m] + d) % 7
    return (ru and WEEKDAYS.ru or WEEKDAYS.en)[dow + 1] .. " " .. string.format("%02d.%02d", d, m)
end

function M.weather_cmd(ctx, args)
    local saved = ui.trim(ctx:cfg("city"))
    local city = ui.trim(args)
    if city == "" then
        city = saved
    end
    if city == "" then
        ctx:edit(ui.usage(ctx:prefix(), "weather", { "<city>", "52.52, 13.40" },
            "The first city you ask for becomes the default"))
        return
    end
    local ru = ctx:lang() == "ru"
    local imperial = ctx:cfg("units") == "imperial"
    local days = math.max(1, math.min(7, math.tointeger(tonumber(ctx:cfg("days") or 3) or 3) or 3))
    ctx:edit(ui.wait((ru and "Погода: " or "Weather: ") .. city .. "…"))

    local data, problem = open_meteo(ctx, city, imperial, days, ru)
    if not data and problem ~= "not_found" then
        local second
        data, second = wttr(ctx, city, imperial, days, ru)
        if not data then
            if second == "not_found" then
                problem = "not_found"
            elseif tostring(second) ~= tostring(problem) then
                problem = tostring(problem) .. "; " .. tostring(second)
            end
        end
    end
    if not data then
        if problem == "not_found" then
            ctx:edit(ui.warn(ru and "Город не найден" or "City not found", ui.mono(city)))
        else
            ctx:edit(ui.err(ru and "Погода недоступна" or "Weather is unavailable", ui.mono(ui.clip(problem, 300))))
        end
        return
    end

    local t_unit = imperial and "°F" or "°C"
    local w_unit = imperial and " mph" or (ru and " км/ч" or " km/h")
    local icon, desc = describe(data.code, ru)
    if data.desc then
        desc = data.desc
    end
    local lines = {
        ui.italic(desc),
        ui.kv(ru and "Температура" or "Temperature",
            ui.num(data.temp, 0) .. t_unit .. " (" .. (ru and "ощущается " or "feels ") .. ui.num(data.feels, 0) .. t_unit .. ")"),
        ui.kv(ru and "Влажность" or "Humidity", ui.num(data.humidity, 0) .. "%"),
        ui.kv(ru and "Ветер" or "Wind", ui.num(data.wind, 0) .. w_unit),
    }
    if #data.forecast > 0 then
        table.insert(lines, "")
    end
    for _, day in ipairs(data.forecast) do
        local day_icon = day.code and describe(day.code, ru) or "▸"
        local line = day_icon .. " " .. ui.mono(day_label(day.date, ru)) .. "  "
            .. ui.num(day.low, 0) .. "…" .. ui.num(day.high, 0) .. t_unit
        if tonumber(day.rain) and tonumber(day.rain) > 0 then
            line = line .. "  💧" .. ui.num(day.rain, 0) .. "%"
        end
        table.insert(lines, line)
    end

    local footer = nil
    if saved == "" and ui.trim(args) ~= "" then
        ctx:cfg_set("city", city)
        footer = ru and ("Город по умолчанию: " .. city) or ("Saved as the default city: " .. city)
    end
    ctx:edit(ui.card(icon, data.place, lines, footer))
end

function M.currency_cmd(ctx, args)
    -- I accept "100 usd to eur", "100usd eur", and "100 USD в RUB".
    args = ui.trim(args):gsub("%s+[Tt][Oo]%s+", " "):gsub("%s+[Ii][Nn]%s+", " "):gsub("%s+в%s+", " ")
    args = args:gsub("^([%d%.,]+)(%a)", "%1 %2")
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
    if not amount then
        ctx:edit(ui.err("Bad amount", ui.mono(args)))
        return
    end
    local res = ctx:fetch("https://open.er-api.com/v6/latest/" .. from, { timeout = 15 })
    local data = res.json
    if type(data) ~= "table" then
        ctx:edit(ui.err("Exchange rates are unavailable", ui.mono(ui.first(res.error, "no data"))))
        return
    end
    if data.result ~= "success" or type(data.rates) ~= "table" or not tonumber(data.rates[to]) then
        ctx:edit(ui.err("Unknown currency", ui.mono(from .. " → " .. to)))
        return
    end
    local rate = tonumber(data.rates[to])
    ctx:edit(ui.card("💱", "Currency", {
        ui.mono(string.format("%.2f %s = %.2f %s", amount, from, amount * rate, to)),
        ui.italic(string.format("1 %s = %.4f %s", from, rate, to)),
    }))
end

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

-- I parse with a small recursive-descent parser instead of load(), so expressions cannot run code.

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
        -- I bind unary minus looser than ^, so -2^2 is -4 as in standard notation.
        if token.kind == "op" and token.value == "-" then return -parse_power() end
        if token.kind == "op" and token.value == "+" then return parse_power() end
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
    local expr = args:gsub("×", "*"):gsub("÷", "/"):gsub(":", "/"):gsub("(%d),(%d)", "%1.%2")
    local ok, result = pcall(evaluate, expr)
    if not ok then
        ctx:edit(ui.err("Cannot calculate", ui.error_text(result)))
        return
    end
    local shown = result
    if math.type(result) == "float" and result == math.floor(result) and math.abs(result) < 1e15 then
        shown = math.floor(result)
    elseif result ~= result or result == math.huge or result == -math.huge then
        ctx:edit(ui.err("Cannot calculate", "the result is not a finite number (division by zero?)"))
        return
    elseif math.type(result) == "float" then
        shown = string.format("%.10g", result)
    end
    ctx:edit("🧮 " .. ui.mono(args) .. " = " .. ui.bold(tostring(shown)))
end

function M.paste_cmd(ctx, args)
    local text = text_or_reply(ctx, args)
    if text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "paste", { "<text>" }, "Or reply to a message"))
        return
    end
    ctx:edit(ui.wait("Uploading…"))
    local res = ctx:fetch("https://paste.rs/", {
        method = "POST",
        body = text,
        headers = { ["content-type"] = "text/plain; charset=utf-8" },
    })
    local url = ui.trim(res.text)
    if not res.ok or not url:match("^https?://") then
        ctx:edit(ui.err("Upload failed", ui.mono(ui.first(res.error, ui.clip(url, 200), "no link returned"))))
        return
    end
    ctx:edit(ui.ok("Pasted", url))
end

return M
