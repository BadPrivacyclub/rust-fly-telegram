import base64
import html
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
browser = shutil.which("chromium") or shutil.which("google-chrome")
if Path('/usr/lib/chromium/chromium').exists():
    browser = '/usr/lib/chromium/chromium'
if not browser:
    raise SystemExit("Chromium is required for panel smoke tests")

sources = []
for path in [*(ROOT / "src/web").rglob("*.js"), *(ROOT / "src/export").glob("*.js")]:
    sources.append([path.name, path.read_text()])
for path in (ROOT / "src/web").glob("*.html"):
    for i, script in enumerate(re.findall(r"<script(?:\s[^>]*)?>(.*?)</script>", path.read_text(), re.S)):
        sources.append([f"{path.name}:{i}", script])

def encoded(text):
    return base64.b64encode(text.encode()).decode()

runner = r'''
const failures=[];
window.onerror=(message)=>failures.push(String(message));
const check=(ok,name)=>{if(!ok)failures.push(name);};
const calls=[];
const options={method:"custom",format:"both",chats:[],excluded_chats:[],profile:true,avatars:true,contacts:false,sessions:false,settings:false,photos:true,videos:false,stickers:false,gifs:false,video_notes:false,voice:true,files:false,max_file_bytes:20971520,max_total_bytes:1073741824,max_messages_per_chat:100000,from_date:null,to_date:null,exclude_text:[],exclude_senders:[],exclude_media_messages:[],include_deleted:true};
const exportFixture={rules:{defaults:options,accounts:{}},schedules:[],exports:[]};
const account={id:17,name:'<img src=x onerror="window.INJECTED=true">',session:"test.session",archive_id:"17"};
const recoveryFixture={exists:true,unlocked:false,credentials:[],accounts:[account],policy:{enabled:false,accounts:{}}};
const Fly={lang:"ru",esc:s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),toast:(s,kind)=>{if(kind==="error")failures.push(s);},api:async(path,init={})=>{
  if(init.body!==undefined){calls.push({path,body:structuredClone(init.body)});if(path.endsWith('/restore'))return {ok:true,need_password:true,token:"test-token"};return {ok:true,jobs:["test-job"]};}
  if(path==="/api/exports")return structuredClone(exportFixture);
  if(path==="/api/accounts")return [account];
  if(path==="/api/recovery")return structuredClone(recoveryFixture);
  if(path.endsWith('/chats'))return [{id:99,title:"Test chat"}];
  throw Error("Unexpected route: "+path);
}};
'''
test = r'''
(async()=>{
 try{
  for(const [name,source] of SOURCES){try{new Function(source);}catch(e){failures.push(name+": "+e.message);}}
  await window.FlyData.load("exports");
  let form=document.getElementById("export-form");
  check(!!form,"export form renders");
  form.elements["account-17"].checked=true;
  form.elements.photos.checked=false;
  form.elements.exclude_text.value="private\nsecret";
  form.elements.max_file_mb.value="5";
  await form.onsubmit(new Event("submit",{cancelable:true}));
  const request=calls.find(c=>c.path==="/api/exports");
  check(request.body.options.photos===false,"media exclusion submitted");
  check(request.body.options.max_file_bytes===5242880,"file size converted");
  check(request.body.options.exclude_text.length===2,"content filters submitted");
  form.elements.use_saved_rules.checked=true;
  await form.onsubmit(new Event("submit",{cancelable:true}));
  check(calls.filter(c=>c.path==="/api/exports").at(-1).body.options===null,"account defaults requested");
  check(!window.INJECTED && !document.querySelector('#export-tools img'),"account names escaped");
  await window.FlyData.load("recovery");
  document.getElementById("vault-password").value="test vault password";
  await document.getElementById("vault-unlock").onsubmit(new Event("submit",{cancelable:true}));
  check(document.getElementById("vault-password").value==="","vault input cleared");
  await document.querySelector("[data-restore]").onclick();
  let dialog=document.querySelector("dialog");
  check(!!dialog,"2FA dialog shown");
  dialog.querySelector("input").value="one-time 2FA test";
  await dialog.querySelector("form").onsubmit(new Event("submit",{cancelable:true}));
  check(!document.querySelector("dialog"),"2FA dialog cleared after login");
  check(calls.some(c=>c.path==="/api/recovery/password"&&c.body.token==="test-token"),"2FA routed to pending login");
  check(!document.body.textContent.includes("one-time 2FA test"),"2FA not rendered");
 }catch(e){failures.push(e.stack||e.message);}
 document.getElementById("results").textContent=JSON.stringify({ok:failures.length===0,failures});
})();
'''
decode = lambda value: f"new TextDecoder().decode(Uint8Array.from(atob('{encoded(value)}'),c=>c.charCodeAt(0)))"
with tempfile.TemporaryDirectory(prefix="fly-panel-test-") as directory:
    directory = Path(directory)
    page = directory / "test.html"
    code = runner + "\nconst SOURCES=JSON.parse(" + decode(json.dumps(sources)) + ");\n"
    code += "eval(" + decode((ROOT / "src/web/assets/data-tools.js").read_text()) + ");\n" + test
    page.write_text('<!doctype html><meta charset="utf-8"><div id="export-tools"></div><div id="recovery-tools"></div><pre id="results">PENDING</pre><script>' + code + '</script>')
    env = os.environ.copy()
    env.pop('LD_PRELOAD', None)
    result = subprocess.run([browser, "--headless", "--no-sandbox", "--disable-gpu", "--disable-background-networking", "--disable-component-update", "--no-first-run", "--disable-dev-shm-usage", f"--user-data-dir={directory / 'profile'}", "--virtual-time-budget=4000", "--dump-dom", page.as_uri()], capture_output=True, text=True, timeout=45, env=env)
    match = re.search(r'<pre id="results">(.*?)</pre>', result.stdout, re.S)
    if not match or match[1] == "PENDING":
        raise SystemExit("Browser test did not complete: " + result.stderr[-2000:])
    report = json.loads(html.unescape(match[1]))
    print(json.dumps(report, ensure_ascii=False, indent=2))
    raise SystemExit(0 if report["ok"] else 1)
