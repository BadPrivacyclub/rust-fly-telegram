local M = {}

M.meta = { name = "core", version = "2.0" }

M.commands = {
    ping = "ping_cmd",
    stats = "stats_cmd",
}

M.help = {
    category = "core",
    description = "Connectivity check and runtime statistics",
    commands = {
        ping = "Response time and uptime",
        stats = "Detailed runtime statistics",
    },
}

function M.ping_cmd(ctx, args)
    local started = ctx:now_ms()
    ctx:edit("🏓 " .. ui.italic("Pinging…"))
    local delay_ms = ctx:now_ms() - started
    local stats = ctx:runtime_stats()
    ctx:edit(ui.card("🏓", "Pong", {
        ui.kv("Response", string.format("%d ms", delay_ms)),
        ui.kv("Uptime", ui.duration(stats.uptime_seconds)),
    }))
end

function M.stats_cmd(ctx, args)
    local stats = ctx:runtime_stats()
    local cpu = stats.cpu_percent and string.format("%.1f%%", stats.cpu_percent) or "—"
    local lines = {
        ui.kv("Version", "v" .. tostring(stats.version)),
        ui.kv("Uptime", ui.duration(stats.uptime_seconds)),
        ui.kv("CPU", cpu),
        ui.kv("Memory", ui.bytes(stats.memory_bytes)),
        ui.kv("Accounts", stats.accounts),
        ui.kv("Modules", stats.modules),
        ui.kv("Updates", stats.updates_seen),
        ui.kv("Commands", stats.commands_seen),
        ui.kv("Errors", stats.errors_seen),
        ui.kv("Platform", tostring(stats.os) .. "/" .. tostring(stats.arch)),
    }
    local top = stats.top_commands or {}
    if #top > 0 then
        local prefix = ctx:prefix()
        local items = {}
        for _, entry in ipairs(top) do
            table.insert(items, ui.mono(prefix .. entry.command) .. " × " .. tostring(entry.count))
        end
        table.insert(lines, "")
        table.insert(lines, ui.bold("Top commands"))
        table.insert(lines, table.concat(items, "  "))
    end
    ctx:edit(ui.card(ui.icons.chart, "fly-telegram stats", lines))
end

return M
