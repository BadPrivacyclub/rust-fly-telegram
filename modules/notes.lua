local M = {}

M.meta = { name = "notes", version = "1.0" }

M.commands = {
    note = "note_cmd",
}

function M.note_cmd(ctx, args)
    local action, rest = args:match("^(%S+)%s*(.*)$")

    if action == "set" then
        if rest == "" then
            ctx:edit("**Usage**  \n`.note set <text>`")
            return
        end

        ctx:db_set("note.value", rest)
        ctx:edit("**Note**  \nStatus: `saved`")
        return
    end

    if action == "get" then
        local note = ctx:db_get("note.value")
        if note == nil then
            ctx:edit("**Note**  \nNo note saved. Use `.note set <text>`.")
        else
            ctx:edit("**Note**\n\n```text\n" .. tostring(note) .. "\n```")
        end
        return
    end

    if action == "clear" then
        ctx:db_set("note.value", nil)
        ctx:edit("**Note**  \nStatus: `cleared`")
        return
    end

    ctx:edit("**Subcommands**  \n`set <text>`  \n`get`  \n`clear`")
end

return M
