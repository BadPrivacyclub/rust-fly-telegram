-- Core module: runtime checks.

local M = {}

M.meta = { name = "core", version = "1.2" }

M.commands = { ping = "ping_cmd" }

local function format_uptime(seconds)
    local hours = math.floor(seconds / 3600)
    local minutes = math.floor((seconds % 3600) / 60)
    local secs = seconds % 60
    return string.format("%dh %dm %ds", hours, minutes, secs)
end

function M.ping_cmd(ctx, args)
    local started = ctx:now_ms()
    ctx:edit("**Pinging...**")
    local delay_ms = ctx:now_ms() - started
    local stats = ctx:runtime_stats()
    local cpu = stats.cpu_percent
    local cpu_text = "unavailable"
    if cpu ~= nil then
        cpu_text = string.format("%.1f%%", cpu)
    end

    local text = "**Pong**  \n"
        .. "**Response:** `" .. tostring(delay_ms) .. " ms` (`"
        .. string.format("%.3f", delay_ms / 1000) .. " s`)  \n"
        .. "**Uptime:** `" .. format_uptime(stats.uptime_seconds or 0) .. "`  \n"
        .. "**CPU:** `" .. cpu_text .. "`"

    ctx:edit(text)
end

return M
