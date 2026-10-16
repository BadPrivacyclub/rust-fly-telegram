"use strict";
(() => {
  const search = document.getElementById("search");
  const deleted = document.getElementById("deleted-only");
  const articles = [...document.querySelectorAll("article")];
  const filter = () => {
    const query = search.value.toLocaleLowerCase();
    for (const message of articles) {
      message.hidden = !message.textContent.toLocaleLowerCase().includes(query)
        || (deleted.checked && !message.classList.contains("deleted"));
    }
    document.getElementById("matches").textContent = `${articles.filter(m => !m.hidden).length} / ${articles.length}`;
  };
  search.addEventListener("input", filter);
  deleted.addEventListener("change", filter);
  filter();
  const key = `fly-export-reading:${document.body.dataset.archive}`;
  const resume = document.getElementById("resume");
  try {
    const last = localStorage.getItem(key);
    if (last && document.getElementById(last)) {
      resume.hidden = false;
      resume.href = `#${last}`;
    }
  } catch {}
  document.addEventListener("click", event => {
    const message = event.target.closest("article");
    if (!message) return;
    try { localStorage.setItem(key, message.id); } catch {}
  });
})();
