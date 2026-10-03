local M = {}

M.meta = { name = "notes", version = "2.0" }

M.commands = {
    note = "note_cmd",
    notes = "notes_cmd",
}

M.help = {
    category = "messaging",
    description = "Named notes stored in the local database",
    commands = {
        note = { args = "<name> | save <name> <text> | del <name>", desc = "Show, save (or reply to save), delete a note" },
        notes = "List saved notes",
    },
}

local PREFIX = "notes."

local function valid_name(name)
    return name ~= "" and #name <= 40 and name:match("^[%w_%-%.]+$") ~= nil
end

local function migrate_legacy(ctx)
    local legacy = ctx:db_get("note.value")
    if legacy ~= nil then
        ctx:db_set(PREFIX .. "default", legacy)
        ctx:db_set("note.value", nil)
    end
end

function M.note_cmd(ctx, args)
    migrate_legacy(ctx)
    local action, rest = ui.split(args)
    if action == "save" or action == "set" then
        local name, text = ui.split(rest)
        if action == "set" and text == "" then
            -- Legacy form: .note set <text>
            name, text = "default", rest
        end
        if text == "" then
            local replied = ctx:replied()
            text = replied and replied.text or ""
        end
        if not valid_name(name) or text == "" then
            ctx:edit(ui.usage(ctx:prefix(), "note", { "save <name> <text>", "save <name>  (reply)" }))
            return
        end
        ctx:db_set(PREFIX .. name:lower(), text)
        ctx:edit(ui.ok("Note saved", ui.mono(name:lower())))
        return
    end
    if action == "del" or action == "clear" then
        local name = (rest ~= "" and rest or "default"):lower()
        if ctx:db_get(PREFIX .. name) == nil then
            ctx:edit(ui.warn("No such note", ui.mono(name)))
            return
        end
        ctx:db_set(PREFIX .. name, nil)
        ctx:edit(ui.ok("Note deleted", ui.mono(name)))
        return
    end
    local name = action == "get" and rest or action
    if name == "" then
        M.notes_cmd(ctx, "")
        return
    end
    local value = ctx:db_get(PREFIX .. name:lower())
    if value == nil then
        ctx:edit(ui.warn("No such note", ui.mono(name) .. " · " .. ctx:prefix() .. "notes"))
        return
    end
    ctx:edit(tostring(value))
end

function M.notes_cmd(ctx, args)
    migrate_legacy(ctx)
    local keys = ctx:db_keys(PREFIX)
    local lines = {}
    for _, key in ipairs(keys) do
        table.insert(lines, ui.mono(key:sub(#PREFIX + 1)))
    end
    if #lines == 0 then
        ctx:edit(ui.info("No notes yet", ui.mono(ctx:prefix() .. "note save <name> <text>")))
        return
    end
    ctx:edit(ui.card(ui.icons.note, "Notes", { table.concat(lines, "  ") },
        ctx:prefix() .. "note <name> to show"))
end

return M
