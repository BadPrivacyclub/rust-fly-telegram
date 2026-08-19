local M = {}

M.meta = { name = "ai", version = "0.1" }

M.commands = {
    ai = "ai_cmd",
    ask = "ask_cmd",
    summarize = "summarize_cmd",
    transcribe = "transcribe_cmd",
    translate = "translate_cmd",
}

local function json_escape(value)
    return tostring(value)
        :gsub("\\", "\\\\")
        :gsub("\"", "\\\"")
        :gsub("\n", "\\n")
        :gsub("\r", "\\r")
end

local function provider(ctx)
    return tostring(ctx:db_get("ai.provider") or "openai")
end

local function model(ctx, key, fallback)
    return tostring(ctx:db_get(key) or fallback)
end

local function first_text_openai(result)
    if result.choices and result.choices[1] and result.choices[1].message then
        return result.choices[1].message.content
    end
    return nil
end

local function first_text_anthropic(result)
    if result.content and result.content[1] then
        return result.content[1].text
    end
    return nil
end

local function first_text_gemini(result)
    if result.candidates and result.candidates[1] and result.candidates[1].content then
        local parts = result.candidates[1].content.parts
        if parts and parts[1] then
            return parts[1].text
        end
    end
    return nil
end

local function complete(ctx, prompt)
    local p = provider(ctx)

    if p == "openai" then
        local key = ctx:env_get("OPENAI_API_KEY")
        if not key then
            return nil, "Missing `OPENAI_API_KEY`."
        end
        local body = "{\"model\":\"" .. json_escape(model(ctx, "ai.openai_model", "gpt-4o-mini")) .. "\",\"messages\":[{\"role\":\"user\",\"content\":\"" .. json_escape(prompt) .. "\"}]}"
        local result = ctx:http_json_request("POST", "https://api.openai.com/v1/chat/completions", body, {
            ["authorization"] = "Bearer " .. key,
            ["content-type"] = "application/json"
        })
        return first_text_openai(result), nil
    end

    if p == "anthropic" then
        local key = ctx:env_get("ANTHROPIC_API_KEY")
        if not key then
            return nil, "Missing `ANTHROPIC_API_KEY`."
        end
        local body = "{\"model\":\"" .. json_escape(model(ctx, "ai.anthropic_model", "claude-3-5-haiku-latest")) .. "\",\"max_tokens\":800,\"messages\":[{\"role\":\"user\",\"content\":\"" .. json_escape(prompt) .. "\"}]}"
        local result = ctx:http_json_request("POST", "https://api.anthropic.com/v1/messages", body, {
            ["x-api-key"] = key,
            ["anthropic-version"] = "2023-06-01",
            ["content-type"] = "application/json"
        })
        return first_text_anthropic(result), nil
    end

    if p == "gemini" then
        local key = ctx:env_get("GEMINI_API_KEY")
        if not key then
            return nil, "Missing `GEMINI_API_KEY`."
        end
        local m = model(ctx, "ai.gemini_model", "gemini-1.5-flash")
        local body = "{\"contents\":[{\"parts\":[{\"text\":\"" .. json_escape(prompt) .. "\"}]}]}"
        local result = ctx:http_json_request("POST", "https://generativelanguage.googleapis.com/v1beta/models/" .. m .. ":generateContent?key=" .. key, body, {
            ["content-type"] = "application/json"
        })
        return first_text_gemini(result), nil
    end

    return nil, "Unknown provider `" .. p .. "`."
end

local function text_or_reply(ctx, args)
    if args ~= "" then
        return args
    end
    return ctx:replied_text()
end

function M.ai_cmd(ctx, args)
    local action, rest = args:match("^(%S+)%s*(.*)$")
    if action == "provider" and rest ~= "" then
        ctx:db_set("ai.provider", rest)
        ctx:edit("**AI**  \nProvider: `" .. rest .. "`")
        return
    end
    if action == "model" and rest ~= "" then
        ctx:db_set("ai." .. provider(ctx) .. "_model", rest)
        ctx:edit("**AI**  \nModel saved for `" .. provider(ctx) .. "`.")
        return
    end
    ctx:edit("**AI**  \nProvider: `" .. provider(ctx) .. "`  \n`.ai provider openai|anthropic|gemini`  \n`.ai model <model>`  \n`.ask <prompt>`  \n`.summarize [text]`  \n`.translate <lang> [text]`")
end

function M.ask_cmd(ctx, args)
    if args == "" then
        ctx:edit("**Usage**  \n`.ask <prompt>`")
        return
    end

    local p = provider(ctx)
    ctx:edit("**AI**  \nThinking with `" .. p .. "`...")

    local output, err = complete(ctx, args)
    ctx:edit(output or ("**AI**  \n" .. tostring(err or "No text returned.")))
end

function M.summarize_cmd(ctx, args)
    local source = text_or_reply(ctx, args)
    if source == "" then
        ctx:edit("**Usage**  \n`.summarize <text>` or reply with `.summarize`")
        return
    end
    ctx:edit("**AI**  \nSummarizing...")
    local output, err = complete(ctx, "Summarize this Telegram text concisely:\n\n" .. source)
    ctx:edit(output or ("**AI**  \n" .. tostring(err or "No text returned.")))
end

function M.translate_cmd(ctx, args)
    local lang, text = args:match("^(%S+)%s*(.*)$")
    if not lang then
        ctx:edit("**Usage**  \n`.translate <lang> <text>` or reply with `.translate <lang>`")
        return
    end
    if text == "" then
        text = ctx:replied_text()
    end
    if text == "" then
        ctx:edit("**Usage**  \n`.translate <lang> <text>` or reply with `.translate <lang>`")
        return
    end
    ctx:edit("**AI**  \nTranslating...")
    local output, err = complete(ctx, "Translate the following text to " .. lang .. ":\n\n" .. text)
    ctx:edit(output or ("**AI**  \n" .. tostring(err or "No text returned.")))
end

function M.transcribe_cmd(ctx, args)
    if provider(ctx) ~= "openai" then
        ctx:edit("**AI**  \nVoice transcription currently uses OpenAI Whisper. Run `.ai provider openai` first.")
        return
    end
    local key = ctx:env_get("OPENAI_API_KEY")
    if not key then
        ctx:edit("**AI**  \nMissing `OPENAI_API_KEY`.")
        return
    end

    ctx:edit("**AI**  \nDownloading replied voice/media...")
    local path = ctx:download_replied_media(nil)
    ctx:edit("**AI**  \nTranscribing...")
    local fields = {
        model = model(ctx, "ai.whisper_model", "whisper-1"),
        response_format = "json",
    }
    if args ~= "" then
        fields.language = args
    end
    local result = ctx:http_json_multipart_file_request("POST", "https://api.openai.com/v1/audio/transcriptions", "file", path, fields, {
        ["authorization"] = "Bearer " .. key
    })
    ctx:edit(result.text or "**AI**  \nNo transcript returned.")
end

return M
