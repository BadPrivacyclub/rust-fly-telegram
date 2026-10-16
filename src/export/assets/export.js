"use strict";
(() => {
  const $ = (selector, root = document) => root.querySelector(selector);
  const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];
  const store = {
    get(key) { try { return localStorage.getItem(key); } catch { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch { /* private mode */ } },
  };

  const theme = store.get("fly-export-theme");
  if (theme) document.documentElement.dataset.theme = theme;
  $("#theme")?.addEventListener("click", () => {
    const dark = matchMedia("(prefers-color-scheme: dark)").matches;
    const current = document.documentElement.dataset.theme || (dark ? "dark" : "light");
    const next = current === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    store.set("fly-export-theme", next);
  });

  $$(".tabs button").forEach((button) => button.addEventListener("click", () => {
    $$(".tabs button").forEach((b) => b.classList.toggle("on", b === button));
    $$("[data-panel]").forEach((panel) => panel.classList.toggle("hidden", panel.dataset.panel !== button.dataset.tab));
    history.replaceState(null, "", `#${button.dataset.tab}`);
  }));
  const initial = location.hash.slice(1);
  if (initial) $(`.tabs button[data-tab="${initial}"]`)?.click();

  document.addEventListener("click", (event) => {
    const spoiler = event.target.closest(".spoiler");
    if (spoiler) spoiler.classList.toggle("shown");
    const jump = event.target.closest("a[data-jump]");
    if (jump) {
      const target = document.getElementById(jump.dataset.jump);
      if (target) {
        event.preventDefault();
        $('.tabs button[data-tab="messages"]')?.click();
        target.classList.remove("hidden");
        target.scrollIntoView({ behavior: "smooth", block: "center" });
        target.classList.remove("flash");
        void target.offsetWidth;
        target.classList.add("flash");
      }
    }
  });

  function bindFilter(input, select, items, counter) {
    if (!input || !items.length) return;
    let timer = 0;
    const apply = () => {
      const query = input.value.trim().toLowerCase();
      const flag = select ? select.value : "";
      let shown = 0;
      for (const item of items) {
        const text = item.dataset.search || "";
        const flags = item.dataset.flags || "";
        const visible = (!query || text.includes(query)) && (!flag || flags.split(" ").includes(flag));
        item.classList.toggle("hidden", !visible);
        if (visible) shown += 1;
      }
      $$(".day").forEach((day) => {
        let next = day.nextElementSibling;
        let any = false;
        while (next && !next.classList.contains("day")) {
          if (!next.classList.contains("hidden")) { any = true; break; }
          next = next.nextElementSibling;
        }
        day.classList.toggle("hidden", !any);
      });
      if (counter) counter.textContent = shown === items.length ? "" : `${shown} / ${items.length}`;
    };
    input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(apply, 120); });
    select?.addEventListener("change", apply);
  }

  bindFilter($("#q"), $("#filter"), $$(".timeline [data-search]"), $("#count"));
  bindFilter($("#chat-q"), $("#chat-kind"), $$("[data-chat]"), $("#chat-count"));
  bindFilter($("#contact-q"), null, $$("[data-contact]"), null);
})();
