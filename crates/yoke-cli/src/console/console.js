"use strict";
(() => {
  const csrf = document.querySelector('meta[name="yoke-csrf"]').content;
  const view = document.getElementById("view");
  const announce = text => { document.getElementById("announcement").textContent = text; };
  const palette = document.getElementById("palette");
  const help = document.getElementById("help");
  let navigation;
  let activity = 0;
  let prefix = false;
  function reading() {
    const dirty = [...view.querySelectorAll("form[data-filter] input, form[data-filter] select")].some(el =>
      el.tagName === "SELECT" ? el.value !== ([...el.options].find(option => option.defaultSelected) || el.options[0])?.value : el.value !== el.defaultValue);
    return dirty || window.scrollY > 0 || document.querySelector("details[open],dialog[open],.confirmation") || /INPUT|SELECT|TEXTAREA/.test(document.activeElement.tagName);
  }
  function pending() {
    document.getElementById("notice").hidden = false;
    announce("New activity is available. Refresh when ready.");
  }
  function hydrate() {
    document.querySelectorAll("time[data-time]").forEach(el => {
      const date = new Date(Number(el.dataset.time) * 1000);
      el.dateTime = date.toISOString();
      el.textContent = date.toLocaleString(undefined, {month:"short",day:"numeric",hour:"2-digit",minute:"2-digit",second:"2-digit"});
    });
  }
  async function navigate(href, history = true, focus = true, live = false) {
    const url = new URL(href, location.href);
    if (url.origin !== location.origin) return;
    if (navigation) navigation.abort();
    const request = new AbortController();
    navigation = request;
    const version = activity;
    view.setAttribute("aria-busy", "true");
    let loading;
    const loader = setTimeout(() => {
      if (live) return;
      loading = document.createElement("div");
      loading.id = "loading"; loading.className = "panel"; loading.setAttribute("role", "status");
      loading.innerHTML = '<p>Loading your evidence...</p><div class="skeleton"></div><div class="skeleton"></div>';
      view.prepend(loading);
    }, 120);
    try {
      const response = await fetch(url, {headers:{"X-Yoke-Fragment":"1"},signal:request.signal});
      const text = await response.text();
      if (request.signal.aborted) return;
      const parsed = new DOMParser().parseFromString(text, "text/html");
      if (!response.ok) throw new Error(parsed.querySelector("main")?.textContent || text || `Request failed (${response.status})`);
      const main = parsed.querySelector("main");
      if (!main) throw new Error("The server returned an incomplete page.");
      // Interaction can begin after the request was sent.
      if (live && reading()) { pending(); return; }
      view.replaceChildren(main);
      const unread=parsed.querySelector("[data-unread-update]"),badge=document.querySelector("[data-unread-count]");
      if(unread&&badge){badge.hidden=unread.dataset.unreadUpdate==="0";badge.querySelector("[data-unread-value]").textContent=unread.dataset.unreadUpdate;}
      if (history) window.history.pushState({}, "", url);
      document.querySelectorAll("nav[aria-label='Workspace'] a").forEach(a => {
        if (new URL(a.href).pathname === url.pathname) a.setAttribute("aria-current","page");
        else a.removeAttribute("aria-current");
      });
      document.title = `${main.querySelector("h1")?.textContent || "Console"} - Yoke`;
      document.getElementById("notice").hidden = version === activity;
      hydrate();
      if (focus) { main.focus({preventScroll:true}); window.scrollTo(0,0); }
      announce("View updated");
      return true;
    } catch(error) {
      if (error.name === "AbortError") return;
      view.querySelector(".navigation-error")?.remove();
      const panel = document.createElement("section");
      panel.className = "panel error navigation-error"; panel.setAttribute("role","alert");
      const message = document.createElement("p"); message.textContent = error.message;
      const retry = document.createElement("button"); retry.textContent = "Try again"; retry.type = "button"; retry.addEventListener("click",()=>navigate(url));
      panel.append(message,retry); view.prepend(panel);
    } finally {
      clearTimeout(loader); loading?.remove();
      if (navigation === request) { navigation = null; view.removeAttribute("aria-busy"); }
    }
  }
  window.addEventListener("popstate",()=>navigate(location.href,false));
  const theme = document.getElementById("theme");
  let savedTheme = "system";
  try { savedTheme = localStorage.getItem("yoke-theme") || "system"; } catch (_) { /* Private browsing may prohibit storage. */ }
  function setTheme(value) {
    if (value === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = value;
    theme.value = value;
  }
  setTheme(savedTheme);
  theme.addEventListener("change",()=>{setTheme(theme.value); try {localStorage.setItem("yoke-theme",theme.value);} catch(_) {}});
  function openPalette() { palette.showModal(); document.getElementById("command-search").focus(); }
  document.getElementById("palette-open").addEventListener("click",openPalette);
  document.getElementById("help-open").addEventListener("click",()=>help.showModal());
  document.getElementById("command-search").addEventListener("input",event=> {
    palette.querySelectorAll("[data-destination]").forEach(button=>{button.hidden = !button.textContent.toLowerCase().includes(event.target.value.toLowerCase());});
  });
  document.addEventListener("click",event=> {
    const close = event.target.closest("[data-close]");
    if (close) {close.closest("dialog").close();return;}
    const destination = event.target.closest("[data-destination]");
    if (destination) {palette.close();navigate(destination.dataset.destination);return;}
    const link = event.target.closest("a[data-nav]");
    if (link && !event.metaKey && !event.ctrlKey && !event.shiftKey) {event.preventDefault();navigate(link.href);return;}
    if (event.target.closest("[data-refresh]")) navigate(location.href,false,false);
    const action = event.target.closest("[data-action]");
    if (action) { perform(action); return; }
    const forget = event.target.closest("[data-confirm-forget]");
    if (forget) {
      const zone=forget.closest(".action-zone");
      if(zone.querySelector(".confirmation"))return;
      const confirmation=document.createElement("div"); confirmation.className="confirmation";
      const label=document.createElement("p");label.textContent="Forget this memory? You can undo for 30 seconds. A shared file is not removed.";
      const yes=document.createElement("button");yes.textContent="Confirm forget";yes.dataset.action="forget";yes.dataset.id=forget.dataset.confirmForget;
      const no=document.createElement("button");no.textContent="Cancel";no.className="quiet";no.dataset.cancel="";
      confirmation.append(label,yes,no);zone.append(confirmation);yes.focus();return;
    }
    const cancel=event.target.closest("[data-cancel]");
    if(cancel){const zone=cancel.closest(".action-zone");cancel.closest(".confirmation")?.remove();zone?.querySelector("button")?.focus();return;}
    const mode=event.target.closest("[data-diff-mode]");
    if(mode){const diff=mode.parentElement.nextElementSibling;diff.dataset.mode=mode.dataset.diffMode;mode.parentElement.querySelectorAll("button").forEach(b=>b.setAttribute("aria-pressed",String(b===mode)));return;}
    const screenshot=event.target.closest("[data-image]");
    if(screenshot){showImage(screenshot);return;}
  });
  document.addEventListener("submit",event=> {
    const form = event.target.closest("form[data-filter]");
    if (form) {event.preventDefault(); const url = new URL(form.action);url.search=new URLSearchParams(new FormData(form));navigate(url);}
  });
  document.addEventListener("keydown",event=> {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase()==="k") {event.preventDefault();if(palette.open)palette.close();else openPalette();return;}
    if (palette.open && ["ArrowDown","ArrowUp","Enter"].includes(event.key)) {
      const buttons = [...palette.querySelectorAll("[data-destination]")].filter(b=>!b.hidden);
      const at = buttons.indexOf(document.activeElement);
      if(event.key==="Enter" && at===-1) {event.preventDefault();buttons[0]?.click();}
      else if(event.key!=="Enter") {event.preventDefault();buttons[(at+(event.key==="ArrowDown"?1:-1)+buttons.length)%buttons.length]?.focus();}
      return;
    }
    if (document.querySelector("dialog[open]") || /INPUT|TEXTAREA|SELECT/.test(document.activeElement.tagName) || document.activeElement.isContentEditable) return;
    if(event.key==="?") {help.showModal();return;}
    if(prefix) {prefix=false;const pages={d:"decisions",o:"projects",s:"sessions",p:"plans",m:"memory",t:"trust",u:"usage",n:"notifications",w:"settings"};if(pages[event.key]){event.preventDefault();navigate(pages[event.key]);}return;}
    if(event.key==="g") {prefix=true;return;}
    if(event.key==="j" || event.key==="k") {
      event.preventDefault();const rows=[...document.querySelectorAll("[data-row]")];const at=rows.indexOf(document.activeElement);
      rows[Math.max(0,Math.min(rows.length-1,at+(event.key==="j"?1:-1)))]?.focus();
    }
  });
  hydrate();
  const identity=el=>["action","id","ids","slug"].map(name=>el.dataset[name]||"").join("\n");
  async function perform(button) {
    button.disabled=true;
    const zone=button.closest(".action-zone") || button.parentElement;
    let focusTarget=button;
    zone.querySelector(".action-error")?.remove();
    try {
      const body={action:button.dataset.action};
      if(button.dataset.id)body.id=Number(button.dataset.id);
      if(button.dataset.ids)body.ids=button.dataset.ids.split(",").map(Number);
      if(button.dataset.slug)body.slug=button.dataset.slug;
      if(button.dataset.ticket)body.ticket=button.dataset.ticket;
      if(button.dataset.value)body.value=button.dataset.value==="true";
      const response=await fetch("actions",{method:"POST",headers:{"X-Yoke-CSRF":csrf,"Content-Type":"application/json"},body:JSON.stringify(body)});
      const result=await response.json();
      if(!response.ok)throw new Error(result.error?.message || "Action failed. Try again.");
      if(result.data.confirmation){zone.querySelector(".confirmation")?.remove();zone.insertAdjacentHTML("beforeend",result.data.confirmation);focusTarget=zone.querySelector("[data-action='approve']");return;}
      await navigate(location.href,false,false);
      // Re-rendering drops focus to <body>: return to the same control if it survived, else the view.
      focusTarget=[...view.querySelectorAll("[data-action]")].find(el=>(el===button||!el.disabled)&&identity(el)===identity(button)) || view.querySelector("main");
      const toast=document.getElementById("toast");toast.replaceChildren(document.createTextNode(result.data.message));toast.hidden=false;
      const dismiss=document.createElement("button");dismiss.textContent="Dismiss";dismiss.setAttribute("aria-label","Dismiss notification");dismiss.addEventListener("click",()=>{toast.hidden=true;});toast.append(dismiss);
      if(result.data.undo){const undo=document.createElement("button");undo.textContent="Undo forget";undo.dataset.action="undo-forget";undo.dataset.ticket=result.data.undo;toast.append(undo);setTimeout(()=>{undo.remove();},30000);}
      announce(result.data.message);
    } catch(error) {
      const message=document.createElement("p");message.className="action-error error";message.setAttribute("role","alert");message.textContent=error.message;zone.append(message);
    } finally {button.disabled=false;focusTarget?.focus({preventScroll:true});}
  }
  let currentImage;
  const lightbox=document.getElementById("lightbox");
  function showImage(button) {
    currentImage=button;
    const img=document.getElementById("lightbox-image");img.src=button.dataset.image;img.alt=button.dataset.caption;
    document.getElementById("image-caption").textContent=button.dataset.caption;
    if(!lightbox.open)lightbox.showModal();
  }
  function moveImage(direction){const images=[...document.querySelectorAll("[data-image]")];const index=images.indexOf(currentImage);if(images.length)showImage(images[(index+direction+images.length)%images.length]);}
  document.getElementById("image-prev").addEventListener("click",()=>moveImage(-1));
  document.getElementById("image-next").addEventListener("click",()=>moveImage(1));
  lightbox.addEventListener("keydown",event=>{if(event.key==="ArrowLeft"){event.preventDefault();moveImage(-1);}if(event.key==="ArrowRight"){event.preventDefault();moveImage(1);}});
  document.getElementById("lightbox-image").addEventListener("error",()=>{document.getElementById("image-caption").textContent="Screenshot unavailable or changed. Close this view and refresh the check evidence.";});
  const connection=document.getElementById("connection");
  let events;
  async function refreshLive() {
    activity++;
    if (navigation || reading()) { pending(); return; }
    const focused=view.contains(document.activeElement)?document.activeElement:null;
    const key=focused?.dataset.action?identity(focused):null;
    const updated=await navigate(location.href,false,false,true);
    if(updated&&focused)((key&&[...view.querySelectorAll("[data-action]")].find(el=>!el.disabled&&identity(el)===key))||view.querySelector("main"))?.focus({preventScroll:true});
  }
  function listen() {
    events=new EventSource("events");
    // Every connection may follow missed changes, including the initial HTML-to-stream gap.
    events.addEventListener("ready",()=>{connection.textContent="Live";connection.classList.remove("offline");refreshLive();});
    events.onerror=()=>{
      connection.textContent="Reconnecting...";connection.classList.add("offline");
      // Browsers never retry a refused stream (for example 503 while other tabs hold every slot).
      if(events.readyState===EventSource.CLOSED)setTimeout(listen,3000);
    };
    events.addEventListener("change",refreshLive);
  }
  listen();
  window.addEventListener("pagehide",()=>events.close());
  window.addEventListener("pageshow",event=>{if(event.persisted)listen();});
})();
