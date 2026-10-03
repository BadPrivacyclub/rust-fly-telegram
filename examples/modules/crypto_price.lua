-- HTTP + JSON: fetch a public API and format the result.
-- Needs the network permission when installed as a third-party module.
local M = {}

M.commands = { price = "price_cmd" }

M.help = {
    category = "utils",
    description = "Cryptocurrency prices from CoinGecko",
    commands = { price = { args = "[coin] [currency]", desc = "e.g. bitcoin usd" } },
}

M.config = {
    { key = "currency", type = "select", options = { "usd", "eur", "rub" }, default = "usd", description = "Default currency" },
}

function M.price_cmd(ctx, args)
    local coin, currency = args:match("^(%S*)%s*(%S*)$")
    coin = coin ~= "" and coin:lower() or "bitcoin"
    currency = currency ~= "" and currency:lower() or ctx:cfg("currency")
    ctx:edit(ui.wait("Fetching " .. coin .. "…"))

    local ok, data = pcall(function()
        return ctx:http_json_get("https://api.coingecko.com/api/v3/simple/price?ids=" .. coin
            .. "&vs_currencies=" .. currency .. "&include_24hr_change=true")
    end)
    if not ok or type(data) ~= "table" or not data[coin] then
        return ctx:edit(ui.err("Unknown coin", ui.mono(coin)))
    end
    local price = data[coin][currency]
    local change = data[coin][currency .. "_24h_change"] or 0
    ctx:edit(ui.card(change >= 0 and "📈" or "📉", coin, {
        ui.kv("Price", string.format("%.2f %s", price, currency:upper())),
        ui.kv("24h", string.format("%+.2f%%", change)),
    }))
end

return M
