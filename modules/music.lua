local M = {}

M.meta = { name = "music", version = "0.1" }

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

local function worker_url(ctx)
    return tostring(ctx:db_get("music.worker_url") or "http://127.0.0.1:9475")
end

local function json_escape(value)
    return tostring(value)
        :gsub("\\", "\\\\")
        :gsub("\"", "\\\"")
        :gsub("\n", "\\n")
        :gsub("\r", "\\r")
end

local function call_worker(ctx, action, payload)
    payload = payload or ""
    local body = "{\"action\":\"" .. action .. "\",\"payload\":\"" .. json_escape(payload) .. "\"}"
    return ctx:http_json_request("POST", worker_url(ctx) .. "/v1/control", body, {
        ["content-type"] = "application/json"
    })
end

local function show_result(ctx, title, result)
    if result.ok == false then
        ctx:edit("**" .. title .. "**  \nError: `" .. tostring(result.error or "worker rejected request") .. "`")
        return
    end
    ctx:edit("**" .. title .. "**  \nStatus: `" .. tostring(result.status or "ok") .. "`")
end

function M.play_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.play <query-or-url>`")
        return
    end
    show_result(ctx, "Music", call_worker(ctx, "play", args))
end

function M.vplay_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.vplay <query-or-url>`")
        return
    end
    show_result(ctx, "Video", call_worker(ctx, "vplay", args))
end

function M.queue_cmd(ctx, args)
    local result = call_worker(ctx, "queue", args)
    ctx:edit("**Queue**\n\n```text\n" .. tostring(result.text or result.status or "empty") .. "\n```")
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
    ctx:edit("**Top Tracks**\n\n```text\n" .. tostring(result.text or "No stats yet.") .. "\n```")
end

return M
