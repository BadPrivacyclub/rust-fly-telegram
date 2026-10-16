"use strict";
window.FlyData = (() => {
  const $ = (id) => document.getElementById(id);
  const esc = Fly.esc;
  const t = (en, ru) => Fly.lang === "ru" ? ru : en;
  let exportState, accounts = [], recoveryState;
  const checkbox = (name, label, checked) => `<label class="row"><input type="checkbox" name="${name}" ${checked ? "checked" : ""}>${esc(label)}</label>`;
  const field = (name, label, value, type = "number") => `<label class="field"><span>${esc(label)}</span><input class="input" name="${name}" type="${type}" value="${esc(String(value ?? ""))}"></label>`;
  const date = (n) => n ? new Date(n * 1000).toLocaleString() : t("Not yet", "Пока нет");
  const report = (error) => Fly.toast(error.message || String(error), "error");
  async function load(page) {
    try { if (page === "exports") await exportsPage(); if (page === "recovery") await recoveryPage(); } catch (error) { report(error); }
  }

  async function exportsPage() {
    [exportState, accounts] = await Promise.all([Fly.api("/api/exports"), Fly.api("/api/accounts")]);
    const o = exportState.rules.defaults;
    $("export-tools").innerHTML = `
      <div class="page-head"><div><h1>${t("Chat exports", "Экспорт чатов")}</h1><p>${t("Account rules, searchable archives and scheduled snapshots.", "Правила аккаунтов, поиск в архивах и экспорт по расписанию.")}</p></div></div>
      <form id="export-form" class="stack">
        <div class="card"><h2>${t("Accounts and chats", "Аккаунты и чаты")}</h2><div class="grid grid-3">${accounts.map(a => checkbox(`account-${a.id}`, `${a.name} (${a.id})`, JSON.parse(Fly.store?.get("fly.accounts.selected", "[]") || "[]").includes(a.id))).join("")}</div>
          <p>${t("Choose one account to select individual chats. With several accounts, all available chats are exported using the selected filters.", "Выберите один аккаунт для выбора отдельных чатов. Для нескольких аккаунтов экспортируются все доступные чаты с выбранными фильтрами.")}</p>
          <button type="button" class="btn" id="export-load-chats">${t("Choose chats", "Выбрать чаты")}</button>
          <select id="export-chats" class="input" multiple size="6" style="width:100%;margin-top:12px" aria-label="Chats"></select>
          <p class="small muted">${t("No selection means all chats. Ctrl/Cmd selects several chats.", "Без выбора экспортируются все чаты. Ctrl/Cmd позволяет выбрать несколько.")}</p>
        </div>
        <div class="card"><div class="grid grid-3">
          <label class="field"><span>${t("Method", "Метод")}</span><select class="input" name="method"><option value="custom">${t("Custom (no Takeout delay)", "Кастомный (без ожидания Takeout)")}</option><option value="official">Telegram Takeout API</option></select></label>
          <label class="field"><span>${t("Format", "Формат")}</span><select class="input" name="format"><option value="both">HTML + JSON</option><option value="html">HTML</option><option value="json">JSON</option><option value="pdf">PDF (${t("one chat", "один чат")})</option></select></label>
          <label class="field"><span>${t("Rules to load", "Загрузить правила")}</span><select class="input" id="export-rule-scope"><option value="global">${t("All accounts", "Все аккаунты")}</option>${accounts.map(a => `<option value="${a.id}">${esc(a.name)}</option>`).join("")}</select></label>
        </div><p id="export-method-note" class="small muted"></p>
        <div class="grid grid-3">${[
          ["profile",t("Profile and phone", "Профиль и телефон")], ["avatars",t("All profile photos", "Все аватарки")], ["contacts",t("Contacts", "Контакты")], ["sessions",t("Authorized devices", "Авторизованные устройства")], ["settings",t("Settings", "Настройки")], ["include_deleted",t("Deleted messages", "Удалённые сообщения")],
          ["photos",t("Photos", "Фото")], ["videos",t("Videos", "Видео")], ["stickers",t("Stickers", "Стикеры")], ["gifs","GIF"], ["video_notes",t("Video notes", "Кружочки")], ["voice",t("Voice messages", "Голосовые сообщения")], ["files",t("Files and audio", "Файлы и аудио")]
        ].map(([key,label]) => checkbox(key,label,o[key])).join("")}</div>
        <p class="small muted">${t("Excluded attachments keep a placeholder. Device information includes no session authorization keys or 2FA passwords.", "Вместо исключённых вложений остаётся заглушка. В информации об устройствах нет ключей авторизации сессий и паролей 2FA.")}</p>
        <div class="grid grid-3">${field("max_file_mb",t("Per file, MiB", "На файл, МиБ"),o.max_file_bytes/1048576)}${field("max_total_mb",t("Archive limit, MiB", "Лимит архива, МиБ"),o.max_total_bytes/1048576)}${field("max_messages_per_chat",t("Messages per chat", "Сообщений на чат"),o.max_messages_per_chat)}${field("from_date",t("From date", "Начальная дата"),"","datetime-local")}${field("to_date",t("To date", "Конечная дата"),"","datetime-local")}</div>
        <div class="grid grid-3"><label class="field"><span>${t("Exclude messages containing (one phrase per line)", "Исключить сообщения с текстом (фраза на строку)")}</span><textarea class="input custom-filter" name="exclude_text" rows="4"></textarea></label>
        ${field("exclude_senders",t("Exclude sender IDs (comma separated)", "Исключить ID отправителей (через запятую)"),"","text")}${field("excluded_chats",t("Exclude chat IDs", "Исключить ID чатов"),"","text")}
        <label class="field"><span>${t("Exclude whole messages with media types", "Исключить целиком сообщения с типами медиа")}</span><select class="input" name="exclude_media_messages" multiple><option value="photo">${t("Photo","Фото")}</option><option value="video">${t("Video","Видео")}</option><option value="sticker">${t("Sticker","Стикер")}</option><option value="gif">GIF</option><option value="video_note">${t("Video note","Кружочек")}</option><option value="file">${t("File","Файл")}</option></select></label></div>
        ${checkbox("use_saved_rules",t("Use each account's saved rules instead of this form", "Использовать сохранённые правила каждого аккаунта вместо настроек формы"),false)}
        <div class="row"><button class="btn btn-primary" type="submit">${t("Start export", "Начать экспорт")}</button><button class="btn" type="button" id="export-save-rules">${t("Save as defaults", "Сохранить правила по умолчанию")}</button><button class="btn" type="button" id="export-clear-rule">${t("Inherit global rules", "Наследовать общие правила")}</button></div></div>
        <div class="card"><h2>${t("Automatic snapshots", "Автоматический экспорт")}</h2><p>${t("History is checked at the chosen interval. An archive is created only when the selected content changes, including edits and archived deletions.", "История проверяется с заданным интервалом. Архив создаётся при изменении выбранного содержимого, включая правки и сохранённые удаления.")}</p>
        <div class="row">${field("every_hours",t("Every N hours", "Каждые N часов"),24)}<button class="btn" type="button" id="export-add-schedule">${t("Add schedule with these settings", "Добавить расписание с этими настройками")}</button></div><div id="export-schedules"></div></div>
      </form><div class="card" style="margin-top:20px"><div class="row between"><h2>${t("Ready archives", "Готовые архивы")}</h2><button class="btn" id="export-refresh">${t("Refresh", "Обновить")}</button></div><div id="export-records"></div></div>`;
    const form = $("export-form");
    function selectedAccounts() { return accounts.filter(a => form.elements[`account-${a.id}`].checked).map(a => a.id); }
    function read() {
      const result = { ...exportState.rules.defaults };
      const data = new FormData(form);
      for (const key of ["profile","avatars","contacts","sessions","settings","include_deleted","photos","videos","stickers","gifs","video_notes","voice","files"]) result[key] = data.has(key);
      result.method = data.get("method"); result.format = data.get("format");
      result.chats = [...$("export-chats").selectedOptions].map(o => Number(o.value));
      for (const key of ["max_file","max_total"]) result[`${key}_bytes`] = Math.round(Number(data.get(`${key}_mb`)) * 1048576);
      result.max_messages_per_chat = Number(data.get("max_messages_per_chat"));
      for (const key of ["from_date","to_date"]) result[key] = data.get(key) ? Math.floor(new Date(data.get(key)).getTime()/1000) : null;
      result.exclude_text = String(data.get("exclude_text")).split("\n").map(s=>s.trim()).filter(Boolean);
      for (const key of ["exclude_senders","excluded_chats"]) result[key] = String(data.get(key)).split(",").map(s=>s.trim()).filter(Boolean).map(Number);
      result.exclude_media_messages = [...form.elements.exclude_media_messages.selectedOptions].map(o=>o.value);
      if (result.method === "official") { result.exclude_text=[]; result.exclude_senders=[]; result.exclude_media_messages=[]; result.include_deleted=false; }
      return result;
    }
    function fill(options) {
      for (const [key,value] of Object.entries(options)) {
        const input = form.elements[key]; if (!input || ["from_date","to_date","exclude_media_messages"].includes(key)) continue;
        if (input.type === "checkbox") input.checked = value;
        else input.value = Array.isArray(value) ? value.join(key === "exclude_text" ? "\n" : ",") : value ?? "";
      }
      for (const key of ["max_file","max_total"]) form.elements[`${key}_mb`].value = options[`${key}_bytes`]/1048576;
      for (const key of ["from_date","to_date"]) {
        const date = options[key] ? new Date(options[key]*1000) : null;
        form.elements[key].value = date ? new Date(date.getTime()-date.getTimezoneOffset()*60000).toISOString().slice(0,16) : "";
      }
      [...form.elements.exclude_media_messages.options].forEach(o=>o.selected=options.exclude_media_messages.includes(o.value));
      [...$("export-chats").options].forEach(o=>o.selected=options.chats.includes(Number(o.value)));
      methodNote();
    }
    function methodNote() {
      $("export-method-note").textContent = form.elements.method.value === "official" ? t("Telegram may require an approval and a waiting period for Takeout. The exact delay appears in Tasks.", "Telegram может потребовать подтверждение и ожидание Takeout. Точная задержка появится в задачах.") : t("Uses normal history requests. Telegram rate limits still apply. PDF requires Chromium on the server.", "Использует обычные запросы истории. Лимиты Telegram сохраняются. Для PDF нужен Chromium на сервере.");
    }
    fill(o); form.elements.method.onchange = methodNote;
    $("export-load-chats").onclick = async () => { try {
      const ids = selectedAccounts(); if (ids.length !== 1) throw Error(t("Select exactly one account", "Выберите ровно один аккаунт"));
      const chats = await Fly.api(`/api/accounts/${ids[0]}/chats`);
      $("export-chats").innerHTML = chats.map(c=>`<option value="${c.id}">${esc(c.title)} (${c.id})</option>`).join("");
      $("export-chats").dataset.account = String(ids[0]);
    } catch(e) { report(e); } };
    form.querySelectorAll('[name^="account-"]').forEach(input => input.onchange = () => {
      const ids=selectedAccounts(); if(ids.length!==1 || $("export-chats").dataset.account!==String(ids[0])) $("export-chats").innerHTML="";
    });
    $("export-rule-scope").onchange = async () => {
      try {
        const scope=$("export-rule-scope").value;
        if(scope!=="global") {
          accounts.forEach(a=>form.elements[`account-${a.id}`].checked=String(a.id)===scope);
          await $("export-load-chats").onclick();
        }
        fill(exportState.rules.accounts[scope] || exportState.rules.defaults);
      } catch(e) {report(e);}
    };
    form.onsubmit = async event => { event.preventDefault(); const button=form.querySelector('[type="submit"]'); button.disabled=true; try {
      const result=await Fly.api("/api/exports",{body:{accounts:selectedAccounts(),options:form.elements.use_saved_rules.checked ? null : read()}}); Fly.toast(`${t("Started tasks", "Запущены задачи")}: ${result.jobs.join(", ")}`);
    } catch(e) { report(e); } finally {button.disabled=false;} };
    $("export-save-rules").onclick = async () => {try{
      const scope=$("export-rule-scope").value; const options=read();
      if(scope==="global") exportState.rules.defaults=options; else exportState.rules.accounts[scope]=options;
      await Fly.api("/api/exports/rules",{body:exportState.rules}); Fly.toast(t("Saved", "Сохранено"));
    }catch(e){report(e);}};
    $("export-clear-rule").onclick=async()=>{try{const scope=$("export-rule-scope").value;if(scope!=="global"){delete exportState.rules.accounts[scope];await Fly.api("/api/exports/rules",{body:exportState.rules});fill(exportState.rules.defaults);}}catch(e){report(e);}};
    $("export-add-schedule").onclick=async()=>{try{
      const ids=selectedAccounts(); if(!ids.length)throw Error(t("Select accounts","Выберите аккаунты"));
      const options=read(); if(options.method!=="custom")throw Error(t("Schedules require Custom export","Для расписания нужен кастомный экспорт"));
      const next=[...exportState.schedules,...ids.map(account=>({id:crypto.randomUUID(),account,options,every_hours:Number(form.elements.every_hours.value),enabled:true}))];
      await Fly.api("/api/exports/schedules",{body:next}); exportState.schedules=next; drawSchedules();
    }catch(e){report(e);}};
    function drawSchedules(){
      $("export-schedules").innerHTML=exportState.schedules.map(s=>`<div class="row between" style="margin-top:12px"><span>${s.account} · ${s.every_hours} h · ${esc(date(s.last_checked))} ${esc(s.last_error||"")}</span><button class="btn btn-sm" type="button" data-schedule="${esc(s.id)}">${t("Remove","Удалить")}</button></div>`).join("");
      $("export-schedules").querySelectorAll("[data-schedule]").forEach(b=>b.onclick=async()=>{try{const next=exportState.schedules.filter(s=>s.id!==b.dataset.schedule);await Fly.api("/api/exports/schedules",{body:next});exportState.schedules=next;drawSchedules();}catch(e){report(e);}});
    }
    function drawRecords(){ $("export-records").innerHTML=exportState.exports.map(r=>`<div class="row between" style="margin-top:14px"><div><b>${esc(r.account_name)}</b><p>${esc(date(r.finished_at))} · ${r.messages} ${t("messages","сообщений")} · ${(r.bytes/1048576).toFixed(1)} MiB</p>${r.warnings.map(w=>`<p>${esc(w)}</p>`).join("")}</div><div class="row"><a class="btn" href="/api/exports/${r.id}/download">${t("Download archive","Скачать архив")}</a>${r.files.map(f=>`<a class="btn" target="_blank" rel="noopener" href="/exports/${r.id}/${encodeURIComponent(f)}">${esc(f)}</a>`).join("")}</div></div>`).join("")||`<p>${t("No completed exports yet. Progress and errors appear in Tasks.","Готовых экспортов пока нет. Ход работы и ошибки доступны в задачах.")}</p>`; }
    $("export-refresh").onclick=async()=>{try{exportState=await Fly.api("/api/exports");drawRecords();drawSchedules();}catch(e){report(e);}};
    drawSchedules();drawRecords();
  }

  async function recoveryPage(){
    recoveryState=await Fly.api("/api/recovery");
    const r=recoveryState;
    $("recovery-tools").innerHTML=`<div class="page-head"><div><h1>${t("Session recovery","Восстановление сессий")}</h1><p>${t("Telegram passkeys in a separate encrypted vault.","Ключи доступа Telegram в отдельном зашифрованном хранилище.")}</p></div></div>
      <div class="card"><h2>${t("Recovery vault","Хранилище ключей")}</h2><p>${r.unlocked?(r.background?t("Unlocked for automatic registration until you lock it or stop the app.","Открыто для автоматической регистрации до блокировки или остановки приложения."):t("Unlocked. Locks after 15 minutes of inactivity.","Разблокировано. Блокируется после 15 минут бездействия.")):t("Locked. The vault password is separate from Telegram 2FA.","Заблокировано. Пароль хранилища отличается от пароля 2FA Telegram.")}</p>
      <form id="vault-unlock" class="row"><input class="input" id="vault-password" type="password" autocomplete="off" minlength="10" required placeholder="${t("Vault password (10+ characters)","Пароль хранилища (от 10 символов)")}"><button class="btn btn-primary">${r.exists?t("Unlock","Разблокировать"):t("Create vault","Создать хранилище")}</button><button class="btn" type="button" id="vault-lock">${t("Lock","Заблокировать")}</button></form>
      <p class="small muted">${t("New sessions wait before registering a passkey. Unlock the vault after restarting to resume registration. Telegram may request 2FA during recovery; that password is never saved.","Перед созданием ключа новая сессия должна пройти период ожидания. После перезапуска разблокируйте хранилище для продолжения регистрации. При восстановлении Telegram может запросить 2FA; этот пароль не сохраняется.")}</p>
      ${checkbox("recovery-default",t("Create passkeys automatically for all accounts, including new ones","Автоматически создавать ключи для всех аккаунтов, включая новые"),r.policy.enabled)}<button class="btn" id="recovery-save-policy" style="margin-top:12px">${t("Save policy","Сохранить настройки")}</button></div>
      <div class="stack" style="margin-top:20px">${r.accounts.map(a=>`<div class="card"><div class="row between"><div><h2>${esc(a.name)}</h2><p>${a.id} · ${esc(a.session)}</p><p>${r.credentials.includes(String(a.id))?t("Credential in unlocked vault","Ключ есть в разблокированном хранилище"):t("Unlock the vault to check the credential","Разблокируйте хранилище для проверки ключа")}</p>${a.error?`<p>${esc(a.error)} · ${esc(date(a.retry_at))}</p>`:""}</div><select class="input recovery-account-policy" data-id="${a.id}"><option value="inherit">${t("Global policy","Общая настройка")}</option><option value="true" ${r.policy.accounts[a.id]===true?"selected":""}>${t("Enabled","Включено")}</option><option value="false" ${r.policy.accounts[a.id]===false?"selected":""}>${t("Disabled","Выключено")}</option></select></div><div class="row"><button class="btn" data-provision="${a.id}">${t("Register key","Создать ключ")}</button><button class="btn btn-primary" data-restore="${a.id}">${t("Restore session","Восстановить сессию")}</button><button class="btn btn-danger" data-forget="${a.id}">${t("Delete access and deleted-message archive","Удалить доступ и архив удалённых сообщений")}</button></div></div>`).join("")}</div>`;
    $("vault-unlock").onsubmit=async event=>{event.preventDefault();const password=$("vault-password").value;$("vault-password").value="";try{await Fly.api("/api/recovery/unlock",{body:{password}});await recoveryPage();}catch(e){report(e);}};
    $("vault-lock").onclick=async()=>{await Fly.api("/api/recovery/lock",{body:{}});await recoveryPage();};
    $("recovery-save-policy").onclick=async()=>{try{
      const policy={enabled:document.querySelector('[name="recovery-default"]').checked,accounts:{}};
      document.querySelectorAll(".recovery-account-policy").forEach(el=>{if(el.value!=="inherit")policy.accounts[el.dataset.id]=el.value==="true";});
      await Fly.api("/api/recovery/policy",{body:policy});Fly.toast(t("Saved","Сохранено"));
    }catch(e){report(e);}};
    document.querySelectorAll("[data-provision]").forEach(button=>button.onclick=async()=>{button.disabled=true;try{await Fly.api(`/api/recovery/${button.dataset.provision}/provision`,{body:{}});await recoveryPage();}catch(e){report(e);button.disabled=false;}});
    document.querySelectorAll("[data-restore]").forEach(button=>button.onclick=async()=>{button.disabled=true;try{
      const result=await Fly.api(`/api/recovery/${button.dataset.restore}/restore`,{body:{}});
      if(result.need_password)showPassword(result.token);else{Fly.toast(t("Session restored","Сессия восстановлена"));await recoveryPage();}
    }catch(e){report(e);}finally{button.disabled=false;}});
    document.querySelectorAll("[data-forget]").forEach(button=>button.onclick=async()=>{const id=button.dataset.forget;const confirmation=prompt(t(`Delete local access and all archived deleted messages for ${id}? Type DELETE ${id}. Existing exports and backups are separate.`,`Удалить локальный доступ и все сохранённые удалённые сообщения аккаунта ${id}? Введите DELETE ${id}. Готовые экспорты и резервные копии хранятся отдельно.`));if(!confirmation)return;try{const result=await Fly.api(`/api/recovery/${id}`,{method:"DELETE",body:{confirmation}});if(!result.server_passkey_revoked)Fly.toast(t("Local key removed. Revoke the remote passkey from another Telegram session.","Локальный ключ удалён. Отзовите ключ на сервере из другой сессии Telegram."));await recoveryPage();}catch(e){report(e);}});
  }
  function showPassword(token){
    const dialog=document.createElement("dialog");dialog.className="card";
    dialog.innerHTML=`<form><h2>${t("Telegram 2FA","Двухфакторная защита Telegram")}</h2><p>${t("Enter your Telegram password. It is used only for this login.","Введите пароль Telegram. Он используется только для этого входа.")}</p><input class="input" type="password" autocomplete="off" required><div class="row" style="margin-top:16px"><button class="btn btn-primary">${t("Continue","Продолжить")}</button><button class="btn" type="button">${t("Cancel","Отмена")}</button></div><p class="error"></p></form>`;
    document.body.append(dialog);dialog.showModal();dialog.querySelector('[type="button"]').onclick=()=>{dialog.close();dialog.remove();};
    dialog.querySelector("form").onsubmit=async event=>{event.preventDefault();const input=dialog.querySelector("input");const password=input.value;input.value="";try{await Fly.api("/api/recovery/password",{body:{token,password}});dialog.close();dialog.remove();await recoveryPage();}catch(e){dialog.querySelector(".error").textContent=e.message;}};
  }
  setTimeout(()=>load(location.hash.slice(1)),0);
  return {load};
})();
