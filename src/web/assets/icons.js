"use strict";
(() => {
  const paths = {
    check:'<path d="m5 12 4 4L19 6"/>', close:'<path d="m6 6 12 12M6 18 18 6"/>',
    clock:'<circle cx="12" cy="12" r="9"/><path d="M12 6v6l4 2"/>',
    key:'<circle cx="8" cy="8" r="5"/><path d="m12 12 9 9m-3-3 3-3m-6 0 3-3"/>',
    download:'<path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5"/>',
    box:'<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M3 10h18M9 5V3h6v2M9 14h6"/>',
    brush:'<path d="m14 3 7 7-9 9-7-7zM5 12l-3 9 9-3"/>',
    bell:'<path d="M6 8a6 6 0 0 1 12 0c0 9 3 8 3 10H3c0-2 3-1 3-10M9 21h6"/>',
    chat:'<path d="M3 4h18v13H8l-5 4zM7 8h10M7 12h7"/>',
    cpu:'<rect x="5" y="5" width="14" height="14" rx="2"/><path d="M9 1v4m6-4v4M9 19v4m6-4v4M1 9h4m-4 6h4m14-6h4m-4 6h4"/>',
    plane:'<path d="m3 10 18-7-7 18-3-8zM11 13 21 3"/>',
    user:'<circle cx="12" cy="7" r="4"/><path d="M4 22v-4a8 8 0 0 1 16 0v4"/>',
    shield:'<path d="m12 2 9 4v6c0 5-9 10-9 10S3 17 3 12V6zM8 12l3 3 5-6"/>',
    file:'<path d="M5 2h9l5 5v15H5zM14 2v6h5M8 12h8M8 16h8"/>',
    settings:'<circle cx="12" cy="12" r="4"/><path d="M12 2v4m0 12v4M2 12h4m12 0h4M5 5l3 3m8 8 3 3M5 19l3-3m8-8 3-3"/>',
    warning:'<path d="m12 3 10 18H2zM12 9v5m0 3v1"/>',
    trash:'<path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7"/>',
    spark:'<path d="m12 2 3 7 7 3-7 3-3 7-3-7-7-3 7-3z"/>',
    play:'<path d="m7 3 14 9-14 9z"/>',
    search:'<circle cx="10" cy="10" r="7"/><path d="m15 15 7 7"/>',
    plus:'<path d="M12 3v18M3 12h18"/>',
    menu:'<path d="M3 6h18M3 12h18M3 18h18"/>',
    bolt:'<path d="m14 2-10 12h7l-1 8 10-12h-7z"/>',
    edit:'<path d="m15 3 6 6-12 12H3v-6zM12 6l6 6"/>',
    calendar:'<rect x="3" y="5" width="18" height="16" rx="2"/><path d="M7 2v6m10-6v6M3 11h18"/>',
    moon:'<path d="M21 13A9 9 0 0 1 11 3 9 9 0 1 0 21 13Z"/>',
    repeat:'<path d="m17 2 4 4-4 4M3 11V6h18M7 22l-4-4 4-4m14-1v5H3"/>',
    eye:'<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Z"/><circle cx="12" cy="12" r="3"/>',
    chart:'<path d="M3 3v18h18M7 17v-5m5 5V8m5 9V4"/>',
    door:'<path d="M3 21V3h12v18M3 21h18M11 11v2m5-5 5 4-5 4m-2-4h7"/>',
  };
  const map = {"✅":"check","✓":"check","✔":"check","❌":"close","✕":"close","⏱":"clock","⏰":"clock","⏳":"clock","🔑":"key","🔐":"key","💾":"download","📦":"box","🎨":"brush","🧹":"brush","🔕":"bell","🔔":"bell","📨":"chat","💬":"chat","⌨":"cpu","✈":"plane","🚀":"plane","👤":"user","👥":"user","🛡":"shield","📎":"file","📄":"file","⚙":"settings","⚠":"warning","❗":"warning","🗑":"trash","✨":"spark","🎵":"play","🎶":"play","🔎":"search","🔍":"search","＋":"plus"};
  Object.assign(map,{"☰":"menu","🤖":"cpu","⚡":"bolt","✏":"edit","📣":"bell","👑":"shield","🗓":"calendar","🌙":"moon","🧨":"trash","🕐":"clock","🔁":"repeat","🟢":"check","👀":"eye","📊":"chart","📋":"file","🧩":"box","🧾":"file","🔄":"repeat","🚪":"door","🎉":"spark"});
  function icon(name) {
    const el=document.createElementNS("http://www.w3.org/2000/svg","svg");
    el.setAttribute("viewBox","0 0 24 24");el.setAttribute("fill","none");el.setAttribute("stroke","currentColor");el.setAttribute("stroke-width","1.8");el.setAttribute("stroke-linecap","round");el.setAttribute("stroke-linejoin","round");el.setAttribute("aria-hidden","true");el.classList.add("fly-icon");el.innerHTML=paths[name]||paths.spark;return el;
  }
  function apply(root) {
    root.querySelectorAll?.("[data-icon]").forEach(el=>{el.replaceChildren(icon(el.dataset.icon));el.removeAttribute("data-icon");});
    const walk=document.createTreeWalker(root,NodeFilter.SHOW_TEXT);
    const nodes=[];let node;
    while((node=walk.nextNode())) {
      const p=node.parentElement;
      if(!p||p.closest("script,style,textarea,pre,code,svg,input,.text,.message-text,.deleted-text"))continue;
      if(!p.closest("button,h1,h2,h3,.brand-logo,.stat-icon,.empty-state,.badge,.nav-item,.toast,.checklist"))continue;
      if([...node.textContent].some(c=>map[c]))nodes.push(node);
    }
    for(const text of nodes){const fragment=document.createDocumentFragment();for(const c of text.textContent){if(map[c])fragment.append(icon(map[c]));else if(c!=="\ufe0f")fragment.append(document.createTextNode(c));}text.replaceWith(fragment);}
  }
  const style=document.createElement("style");style.textContent=".fly-icon{display:inline-block;width:1.15em;height:1.15em;vertical-align:-.18em;flex-shrink:0}.nav-item>.fly-icon{width:20px;height:20px}.brand-logo .fly-icon{width:28px;height:28px}.stat-icon .fly-icon{width:22px;height:22px}";document.head.append(style);
  let scheduled=false;
  const observer=new MutationObserver(()=>{if(scheduled)return;scheduled=true;requestAnimationFrame(()=>{observer.disconnect();apply(document.body);observer.observe(document.body,{childList:true,subtree:true});scheduled=false;});});
  apply(document.body);observer.observe(document.body,{childList:true,subtree:true});
})();
