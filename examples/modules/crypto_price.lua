-- HTTP and JSON: I fetch a public API and format the result.
-- Installed as a third-party module, I need the network permission.
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
    local words = ui.args(args)
    local coin = (words[1] or "bitcoin"):lower()
    local currency = (words[2] or ctx:cfg("currency") or "usd"):lower()
    ctx:edit(ui.wait("Fetching " .. coin .. "…"))

    -- I check res.ok and res.error: ctx:fetch does not raise on network errors.
    local res = ctx:fetch("https://api.coingecko.com/api/v3/simple/price", {
        query = { ids = coin, vs_currencies = currency, include_24hr_change = "true" },
    })
    if not res.ok then
        return ctx:edit(ui.err("CoinGecko is unavailable", ui.mono(res.error)))
    end
    -- I read nested JSON with ui.get, which returns nil instead of failing on a missing field.
    local price = tonumber(ui.get(res.json, coin .. "." .. currency))
    if not price then
        return ctx:edit(ui.err("Unknown coin or currency", ui.mono(coin .. " / " .. currency)))
    end
    local change = tonumber(ui.get(res.json, coin .. "." .. currency .. "_24h_change")) or 0
    ctx:edit(ui.card(change >= 0 and "📈" or "📉", coin, {
        ui.kv("Price", ui.num(price, 2) .. " " .. currency:upper()),
        ui.kv("24h", string.format("%+.2f%%", change)),
    }))
end

return M
