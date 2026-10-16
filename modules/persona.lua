local M = {}

M.commands = {
    persona = "persona_cmd",
    unpersona = "unpersona_cmd",
}

M.help = {
    category = "security",
    description = "Generated personas: swap name, username, bio and public photo, then restore everything",
    commands = {
        persona = { args = "[anime|tiktok|osint|anon|music|art|random] [!]", desc = "Apply a generated persona; without arguments shows the list and the current state" },
        unpersona = "Restore the original profile, username, public photo and photo privacy",
    },
}

local TEXT = {
    en = {
        list = "Personas",
        active = "Active persona",
        none = "No persona is active",
        real = "Real profile",
        checking = "Checking the account…",
        creating = "Creating a persona: picture, name, username…",
        restoring = "Restoring the original profile…",
        limit_title = "Your old username cannot be protected",
        limit_text = "The account already owns the maximum number of public channels, so @%s cannot be parked on a private channel while the persona is active. Anyone could take it.",
        limit_hint = "Free a public link, or run %s to continue without protection",
        applied = "Persona applied",
        name = "Name",
        username = "Username",
        bio = "Bio",
        photo = "Public photo",
        privacy = "Who sees my photos",
        nobody = "Nobody (everyone sees the public photo)",
        parked = "@%s is parked on a private channel",
        unchanged = "unchanged",
        warnings = "Warnings",
        restore_hint = "Restore: %s",
        restored = "Profile restored",
        restored_partly = "Profile partly restored",
        retry = "Run %s again to retry the failed steps",
        privacy_back = "Photo privacy",
        public_back = "Public photo",
        yes = "restored",
        no = "not restored",
        failed = "Persona not applied",
        restore_failed = "Restore failed",
    },
    ru = {
        list = "Личности",
        active = "Активная личность",
        none = "Личность не активна",
        real = "Настоящий профиль",
        checking = "Проверяю аккаунт…",
        creating = "Создаю личность: картинка, имя, юзернейм…",
        restoring = "Возвращаю исходный профиль…",
        limit_title = "Старый юзернейм не получится защитить",
        limit_text = "У аккаунта уже максимум публичных каналов, поэтому @%s нельзя закрепить за скрытым каналом, пока активна личность. Его сможет занять кто угодно.",
        limit_hint = "Освободите публичную ссылку или выполните %s, чтобы продолжить без защиты",
        applied = "Личность применена",
        name = "Имя",
        username = "Юзернейм",
        bio = "Био",
        photo = "Публичное фото",
        privacy = "Кто видит фото профиля",
        nobody = "Никто (все видят публичное фото)",
        parked = "@%s закреплён за скрытым каналом",
        unchanged = "не изменён",
        warnings = "Предупреждения",
        restore_hint = "Вернуть: %s",
        restored = "Профиль восстановлен",
        restored_partly = "Профиль восстановлен частично",
        retry = "Повторите %s, чтобы доделать оставшиеся шаги",
        privacy_back = "Приватность фото",
        public_back = "Публичное фото",
        yes = "восстановлено",
        no = "не восстановлено",
        failed = "Личность не применена",
        restore_failed = "Не удалось восстановить",
    },
}

local function texts(ctx)
    return TEXT[ctx:lang()] or TEXT.en
end

local function show_list(ctx, t)
    local p = ctx:prefix()
    local lines = {}
    for _, kind in ipairs(ctx:persona_kinds()) do
        table.insert(lines, kind.emoji .. " " .. ui.mono(p .. "persona " .. kind.key) .. " · " .. kind.title)
    end
    table.insert(lines, "🎲 " .. ui.mono(p .. "persona random"))
    table.insert(lines, "")
    local status = ctx:persona_status()
    if status then
        table.insert(lines, ui.kv(t.active, status.title))
        if status.name then
            table.insert(lines, ui.kv(t.name, status.name))
        end
        if status.username then
            table.insert(lines, ui.kv(t.username, "@" .. status.username))
        end
        local real = status.original_name
        if status.original_username then
            real = real .. " · @" .. status.original_username
        end
        table.insert(lines, ui.kv(t.real, real))
        table.insert(lines, ui.italic(string.format(t.restore_hint, p .. "unpersona")))
    else
        table.insert(lines, ui.italic(t.none))
    end
    ctx:edit(ui.card("🎭", t.list, lines))
end

function M.persona_cmd(ctx, args)
    local t = texts(ctx)
    local p = ctx:prefix()
    local kind, rest = ui.split(args)
    -- "!" after the persona (or on its own) means: continue even if the old username stays unprotected.
    local force = rest == "!" or rest == "force" or kind:sub(-1) == "!"
    kind = (kind:gsub("!$", ""))
    if kind == "" and not force then
        show_list(ctx, t)
        return
    end

    ctx:edit(ui.wait(t.checking))
    local ok, check = pcall(function() return ctx:persona_check() end)
    if not ok then
        ctx:edit(ui.err(t.failed, ui.error_text(check)))
        return
    end
    if check.guard and check.guard.status == "limit_reached" and not force then
        local again = p .. "persona " .. (kind ~= "" and kind or "random") .. " !"
        ctx:edit(ui.card(ui.icons.warn, t.limit_title, {
            string.format(t.limit_text, check.username or "?"),
            "",
            ui.italic(string.format(t.limit_hint, ui.mono(again))),
        }))
        return
    end

    ctx:edit(ui.wait(t.creating))
    local applied, report = pcall(function() return ctx:persona_apply(kind) end)
    if not applied then
        ctx:edit(ui.err(t.failed, ui.error_text(report)))
        return
    end
    local lines = {
        ui.kv(t.name, report.name),
        ui.kv(t.username, report.username and ("@" .. report.username) or t.unchanged),
        ui.kv(t.bio, report.about ~= "" and report.about or "—"),
        ui.kv(t.photo, report.image_source or "—"),
        ui.kv(t.privacy, t.nobody),
    }
    if report.username_parked and report.old_username then
        table.insert(lines, ui.icons.lock .. " " .. string.format(t.parked, report.old_username))
    end
    if report.warnings and #report.warnings > 0 then
        table.insert(lines, "")
        table.insert(lines, ui.icons.warn .. " " .. ui.bold(t.warnings))
        for _, warning in ipairs(report.warnings) do
            table.insert(lines, "• " .. warning)
        end
    end
    ctx:edit(ui.card("🎭", t.applied .. " · " .. report.title, lines, string.format(t.restore_hint, p .. "unpersona")))
end

function M.unpersona_cmd(ctx, args)
    local t = texts(ctx)
    if not ctx:persona_status() then
        ctx:edit(ui.info(t.none))
        return
    end
    ctx:edit(ui.wait(t.restoring))
    local ok, report = pcall(function() return ctx:persona_restore() end)
    if not ok then
        ctx:edit(ui.err(t.restore_failed, ui.error_text(report)))
        return
    end
    local lines = {
        ui.kv(t.name, report.name),
        ui.kv(t.username, report.username and ("@" .. report.username) or "—"),
        ui.kv(t.privacy_back, table.concat(report.privacy, ", ")),
        ui.kv(t.public_back, report.public_photo_restored and t.yes or t.no),
    }
    if report.errors and #report.errors > 0 then
        table.insert(lines, "")
        for _, err in ipairs(report.errors) do
            table.insert(lines, ui.icons.err .. " " .. err)
        end
        ctx:edit(ui.card(ui.icons.warn, t.restored_partly, lines, string.format(t.retry, ctx:prefix() .. "unpersona")))
        return
    end
    ctx:edit(ui.card(ui.icons.ok, t.restored, lines))
end

return M
