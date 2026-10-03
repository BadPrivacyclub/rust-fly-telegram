local M = {}

M.meta = { name = "ai", version = "2.0" }

M.commands = {
    ai = "ai_cmd",
    ask = "ask_cmd",
    summarize = "summarize_cmd",
    transcribe = "transcribe_cmd",
    translate = "translate_cmd",
}

M.help = {
    category = "ai",
    description = "Ask, summarize, translate, and transcribe with OpenAI, Anthropic, or Gemini",
    commands = {
        ai = "Show provider and setup status",
        ask = { args = "<prompt> (or reply)", desc = "Ask the model; the replied message is included as context" },
        summarize = { args = "[text] (or reply)", desc = "Short summary" },
        translate = { args = "<lang> [text] (or reply)", desc = "Translate text" },
        transcribe = { args = "[language] (reply to voice)", desc = "Speech to text with Whisper (OpenAI)" },
    },
}

M.config = {
    { key = "provider", type = "select", options = { "openai", "anthropic", "gemini" }, default = "openai", description = "Which AI provider to use" },
    { key = "openai_api_key", type = "string", secret = true, description = "OpenAI API key (or env OPENAI_API_KEY)" },
    { key = "anthropic_api_key", type = "string", secret = true, description = "Anthropic API key (or env ANTHROPIC_API_KEY)" },
    { key = "gemini_api_key", type = "string", secret = true, description = "Google Gemini API key (or env GEMINI_API_KEY)" },
    { key = "openai_model", type = "string", default = "gpt-4o-mini", description = "OpenAI model" },
    { key = "anthropic_model", type = "string", default = "claude-haiku-4-5", description = "Anthropic model" },
    { key = "gemini_model", type = "string", default = "gemini-2.5-flash", description = "Gemini model" },
    { key = "max_tokens", type = "number", default = 1024, description = "Maximum answer length in tokens" },
    { key = "system", type = "text", default = "You are a concise assistant inside Telegram. Answer in the user's language. Use plain text or simple Markdown.", description = "System instructions" },
}

local ENV_KEYS = {
    openai = "OPENAI_API_KEY",
    anthropic = "ANTHROPIC_API_KEY",
    gemini = "GEMINI_API_KEY",
}

local function provider(ctx)
    local value = ctx:cfg("provider")
    if value == nil or value == "" then
        value = ctx:db_get("ai.provider") or "openai"
    end
    return tostring(value)
end

local function api_key(ctx, name)
    local key = ctx:cfg(name .. "_api_key")
    if key ~= nil and key ~= "" then
        return key
    end
    return ctx:env_get(ENV_KEYS[name])
end

local function missing_key(ctx, name)
    return "No API key for " .. ui.mono(name) .. ". Set it with "
        .. ui.mono(ctx:prefix() .. "cfg ai " .. name .. "_api_key <key>")
        .. " or in the web panel."
end

local function complete(ctx, prompt)
    local p = provider(ctx)
    local key = api_key(ctx, p)
    if not key then
        return nil, missing_key(ctx, p)
    end
    local system = tostring(ctx:cfg("system") or "")
    local max_tokens = math.floor(tonumber(ctx:cfg("max_tokens")) or 1024)

    if p == "openai" then
        local messages = {}
        if system ~= "" then
            table.insert(messages, { role = "system", content = system })
        end
        table.insert(messages, { role = "user", content = prompt })
        local result = ctx:http_json_request("POST", "https://api.openai.com/v1/chat/completions",
            json.encode({ model = ctx:cfg("openai_model"), messages = messages, max_tokens = max_tokens }),
            { ["authorization"] = "Bearer " .. key, ["content-type"] = "application/json" })
        local choice = result.choices and result.choices[1]
        return choice and choice.message and choice.message.content, nil
    end

    if p == "anthropic" then
        local body = {
            model = ctx:cfg("anthropic_model"),
            max_tokens = max_tokens,
            messages = { { role = "user", content = prompt } },
        }
        if system ~= "" then
            body.system = system
        end
        local result = ctx:http_json_request("POST", "https://api.anthropic.com/v1/messages",
            json.encode(body),
            { ["x-api-key"] = key, ["anthropic-version"] = "2023-06-01", ["content-type"] = "application/json" })
        local parts = {}
        for _, block in ipairs(result.content or {}) do
            if block.type == "text" then
                table.insert(parts, block.text)
            end
        end
        return #parts > 0 and table.concat(parts, "\n") or nil, nil
    end

    if p == "gemini" then
        local body = { contents = { { parts = { { text = prompt } } } } }
        if system ~= "" then
            body.systemInstruction = { parts = { { text = system } } }
        end
        local result = ctx:http_json_request("POST",
            "https://generativelanguage.googleapis.com/v1beta/models/" .. tostring(ctx:cfg("gemini_model")) .. ":generateContent",
            json.encode(body),
            { ["x-goog-api-key"] = key, ["content-type"] = "application/json" })
        local candidate = result.candidates and result.candidates[1]
        local part = candidate and candidate.content and candidate.content.parts and candidate.content.parts[1]
        return part and part.text, nil
    end

    return nil, "Unknown provider " .. ui.mono(p)
end

local function run(ctx, title, prompt)
    ctx:edit("🤖 " .. ui.italic(title .. " · " .. provider(ctx)))
    local ok, output, err = pcall(complete, ctx, prompt)
    if not ok then
        ctx:edit(ui.err("AI request failed", tostring(output)))
        return
    end
    if not output or output == "" then
        ctx:edit(ui.warn("No answer", err or "The provider returned no text."))
        return
    end
    ctx:edit(output)
end

function M.ai_cmd(ctx, args)
    local action, rest = ui.split(args)
    -- Kept for older muscle memory: .ai provider <name>
    if action == "provider" and rest ~= "" then
        ctx:cfg_set("provider", rest)
        ctx:edit(ui.ok("Provider", ui.mono(rest)))
        return
    end
    local lines = { ui.kv("Provider", provider(ctx)) }
    for _, name in ipairs({ "openai", "anthropic", "gemini" }) do
        table.insert(lines, ui.status(api_key(ctx, name) ~= nil, name .. " key set", name .. " key missing"))
    end
    table.insert(lines, "")
    table.insert(lines, ui.italic("Settings: " .. ctx:prefix() .. "cfg ai"))
    ctx:edit(ui.card("🤖", "AI", lines))
end

function M.ask_cmd(ctx, args)
    local context = ctx:replied_text()
    if args == "" and context == "" then
        ctx:edit(ui.usage(ctx:prefix(), "ask", { "<prompt>" }, "Reply to a message to include it as context"))
        return
    end
    local prompt = args
    if context ~= "" then
        prompt = "Message:\n" .. context .. "\n\n" .. (args ~= "" and args or "Respond to this message.")
    end
    run(ctx, "Thinking", prompt)
end

function M.summarize_cmd(ctx, args)
    local source = args ~= "" and args or ctx:replied_text()
    if source == "" then
        ctx:edit(ui.usage(ctx:prefix(), "summarize", { "<text>" }, "Or reply to a message"))
        return
    end
    run(ctx, "Summarizing", "Summarize this Telegram text concisely:\n\n" .. source)
end

function M.translate_cmd(ctx, args)
    local lang, text = ui.split(args)
    if text == "" then
        text = ctx:replied_text()
    end
    if lang == "" or text == "" then
        ctx:edit(ui.usage(ctx:prefix(), "translate", { "<lang> <text>" }, "Or reply with " .. ctx:prefix() .. "translate <lang>"))
        return
    end
    run(ctx, "Translating", "Translate the following text to " .. lang .. ". Reply with the translation only.\n\n" .. text)
end

function M.transcribe_cmd(ctx, args)
    local key = api_key(ctx, "openai")
    if not key then
        ctx:edit(ui.warn("Transcription uses OpenAI Whisper", missing_key(ctx, "openai")))
        return
    end
    ctx:edit(ui.wait("Downloading audio…"))
    local path = ctx:download_replied_media(nil)
    ctx:edit(ui.wait("Transcribing…"))
    local fields = { model = "whisper-1", response_format = "json" }
    if args ~= "" then
        fields.language = args
    end
    local result = ctx:http_json_multipart_file_request("POST", "https://api.openai.com/v1/audio/transcriptions", "file", path, fields, {
        ["authorization"] = "Bearer " .. key,
    })
    if result.text and result.text ~= "" then
        ctx:edit("🎙 " .. result.text)
    else
        ctx:edit(ui.warn("No transcript returned"))
    end
end

return M
