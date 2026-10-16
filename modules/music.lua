local M = {}

M.meta = { name = "music", version = "2.0" }

M.commands = {
    play = "play_cmd",
    vplay = "vplay_cmd",
    queue = "queue_cmd",
    skip = "skip_cmd",
    seek = "seek_cmd",
    loop = "loop_cmd",
    shuffle = "shuffle_cmd",
    stop = "stop_cmd",
    toptracks = "toptracks_cmd",
}

M.help = {
    category = "music",
    description = "Voice chat music through the external music-worker process",
    commands = {
        play = { args = "<query-or-url>", desc = "Play audio in the voice chat" },
        vplay = { args = "<query-or-url>", desc = "Play video" },
        queue = "Show the queue",
        skip = "Skip the current track",
        seek = { args = "<seconds>", desc = "Seek in the current track" },
        loop = "Toggle loop",
        shuffle = "Shuffle the queue",
        stop = "Stop and clear the queue",
        toptracks = "Most played tracks",
    },
}

M.config = {
    { key = "worker_url", type = "string", default = "http://127.0.0.1:9475", description = "music-worker control URL" },
}

local function worker_url(ctx)
    local legacy = ctx:db_get("music.worker_url")
    if legacy and legacy ~= "" and ctx:cfg("worker_url") == "http://127.0.0.1:9475" then
        return tostring(legacy)
    end
    return tostring(ctx:cfg("worker_url"))
end

local function call_worker(ctx, action, payload)
    local url = worker_url(ctx):gsub("/+$", "")
    local res = ctx:fetch(url .. "/v1/control", {
        json = { action = action, payload = payload or "" },
        timeout = 30,
    })
    if res.status == 0 then
        return {
            ok = false,
            error = "music-worker is not running at " .. url
                .. ". Start the music-worker program, or change the address with "
                .. ctx:prefix() .. "cfg music worker_url <url>",
        }
    end
    if type(res.json) == "table" then
        if not res.ok and res.json.ok == nil then
            res.json.ok = false
            res.json.error = res.json.error or res.error
        end
        return res.json
    end
    return { ok = res.ok, error = res.error, text = ui.clip(res.text, 3000) }
end

local function show_result(ctx, title, result)
    if result.ok == false then
        ctx:edit(ui.err(title, tostring(result.error or "worker rejected request")))
        return
    end
    ctx:edit("🎵 " .. ui.bold(title) .. " · " .. ui.mono(result.status or "ok"))
end

function M.play_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "play", { "<query-or-url>" }))
        return
    end
    show_result(ctx, "Music", call_worker(ctx, "play", args))
end

function M.vplay_cmd(ctx, args)
    if args == "" then
        ctx:edit(ui.usage(ctx:prefix(), "vplay", { "<query-or-url>" }))
        return
    end
    show_result(ctx, "Video", call_worker(ctx, "vplay", args))
end

function M.queue_cmd(ctx, args)
    local result = call_worker(ctx, "queue", args)
    ctx:edit(ui.card("📜", "Queue", { ui.code(result.text or result.status or result.error or "empty") }))
end

function M.skip_cmd(ctx, args)
    show_result(ctx, "Skip", call_worker(ctx, "skip", args))
end

function M.seek_cmd(ctx, args)
    show_result(ctx, "Seek", call_worker(ctx, "seek", args))
end

function M.loop_cmd(ctx, args)
    show_result(ctx, "Loop", call_worker(ctx, "loop", args))
end

function M.shuffle_cmd(ctx, args)
    show_result(ctx, "Shuffle", call_worker(ctx, "shuffle", args))
end

function M.stop_cmd(ctx, args)
    show_result(ctx, "Stop", call_worker(ctx, "stop", args))
end

function M.toptracks_cmd(ctx, args)
    local result = call_worker(ctx, "toptracks", args)
    ctx:edit(ui.card("🏆", "Top tracks", { ui.code(result.text or result.error or "No stats yet.") }))
end

return M
