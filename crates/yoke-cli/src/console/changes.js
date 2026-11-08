"use strict";
// Owner: parity-changes. Changes page and plan notes. Loaded on every page after console.js.
// Server-rendered buttons carry data-* attributes; this script only posts actions, shows the
// confirmation the server asks for, and remembers view preferences. No inline script or style.
(() => {
  if (window.yokeChangesInstalled) return;
  window.yokeChangesInstalled = true;
  const yoke = () => window.yoke;
  const prefs = {
    get(key, fallback) { try { return localStorage.getItem(key) || fallback; } catch (_) { return fallback; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch (_) { /* Storage may be blocked. */ } }
  };
  const el = (tag, props = {}, ...children) => {
    const node = document.createElement(tag);
    Object.assign(node, props);
    node.append(...children);
    return node;
  };

  function applyPreferences() {
    document.querySelectorAll(".changes-layout").forEach(root => {
      root.dataset.layout = prefs.get("yoke-changes-layout", "flat");
      root.dataset.diffView = prefs.get("yoke-changes-view", "unified");
      root.dataset.wrap = prefs.get("yoke-changes-wrap", "off");
      root.querySelectorAll("[data-changes-layout]").forEach(b => b.setAttribute("aria-pressed", String(b.dataset.changesLayout === root.dataset.layout)));
      root.querySelectorAll("[data-changes-view]").forEach(b => b.setAttribute("aria-pressed", String(b.dataset.changesView === root.dataset.diffView)));
      root.querySelectorAll("[data-changes-wrap]").forEach(b => b.setAttribute("aria-pressed", String(root.dataset.wrap === "on")));
    });
  }

  function showError(source, message) {
    const host = source.closest("form, .actions, .stash, .note-form") || source.parentElement;
    host.parentElement.querySelectorAll(":scope > .action-error").forEach(n => n.remove());
    host.after(el("p", { className: "action-error error", role: "alert", textContent: message }));
  }

  // The orchestrator's confirm step needs a top-level ticket, which yoke.act does not send.
  async function confirmTicket(ticket) {
    const csrf = document.querySelector('meta[name="yoke-csrf"]').content;
    const response = await fetch("actions", {
      method: "POST",
      headers: { "X-Yoke-CSRF": csrf, "Content-Type": "application/json" },
      body: JSON.stringify({ action: "confirm", ticket })
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.message || "Action failed. Try again.");
    return result.data;
  }

  // Show exactly what will happen; resolve with the confirmed result, or null when cancelled.
  function askConfirm(source, confirm) {
    return new Promise((resolve, reject) => {
      document.querySelectorAll(".changes-confirm").forEach(n => n.remove());
      const yes = el("button", { type: "button", textContent: "Confirm" });
      const no = el("button", { type: "button", className: "quiet", textContent: "Cancel" });
      const box = el("div", { className: "confirmation changes-confirm", role: "group" },
        el("p", { className: "confirm-text", textContent: confirm.message }), el("div", { className: "actions" }, yes, no));
      box.setAttribute("aria-label", "Confirm this action");
      (source.closest(".actions, .stash, form") || source).after(box);
      yes.focus();
      const close = () => { box.remove(); source.focus({ preventScroll: true }); };
      no.addEventListener("click", () => { close(); resolve(null); });
      box.addEventListener("keydown", e => { if (e.key === "Escape") { close(); resolve(null); } });
      yes.addEventListener("click", async () => {
        yes.disabled = true;
        try { const data = await confirmTicket(confirm.ticket); box.remove(); resolve(data); }
        catch (error) { box.remove(); reject(error); }
      });
    });
  }

  function save(file) {
    const url = URL.createObjectURL(new Blob([file.content], { type: "text/markdown;charset=utf-8" }));
    const link = el("a", { href: url, download: file.filename });
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 10000);
  }

  async function run(action, params, source, { refresh = true } = {}) {
    source.disabled = true;
    source.setAttribute("aria-busy", "true");
    yoke().announce("Working");
    try {
      let data = await yoke().act(action, params);
      if (data.confirm) { data = await askConfirm(source, data.confirm); if (!data) return null; }
      if (data.download) save(data.download);
      if (data.message) yoke().notify(data.message);
      if (refresh) await yoke().refresh();
      return data;
    } catch (error) {
      showError(source, error.message);
      return null;
    } finally {
      source.disabled = false;
      source.removeAttribute("aria-busy");
    }
  }

  function fromSelectors(button) {
    const params = JSON.parse(button.dataset.params || "{}");
    if (button.dataset.from) {
      for (const [name, selector] of Object.entries(JSON.parse(button.dataset.from))) params[name] = document.querySelector(selector)?.value ?? "";
    }
    return params;
  }

  async function draft(button) {
    const agent = button.dataset.changesDraft;
    const panel = button.closest(".changes-commit");
    const preview = panel.querySelector(".draft-preview");
    const data = await run("changes.draft-preview", { agent }, button, { refresh: false });
    if (!data?.preview) return;
    const p = data.preview;
    const send = el("button", { type: "button", textContent: `Send ${p.files.length} file(s) to ${agent}` });
    const cancel = el("button", { type: "button", className: "quiet", textContent: "Cancel" });
    preview.replaceChildren(
      el("p", { textContent: `The staged diff of these ${p.files.length} file(s), about ${Math.max(1, Math.round(p.bytes / 1024))} KiB with secret-looking values redacted, will be sent to the ${agent} command-line tool. Nothing has been sent yet.` }),
      el("ul", {}, ...p.files.map(f => el("li", {}, el("code", { textContent: f })))),
      ...(p.skipped.length ? [el("p", { className: "muted", textContent: `Left out because their names look like secrets: ${p.skipped.join(", ")}` })] : []),
      el("div", { className: "actions" }, send, cancel));
    preview.hidden = false;
    send.focus();
    cancel.addEventListener("click", () => { preview.hidden = true; button.focus(); });
    send.addEventListener("click", async () => {
      const result = await run("changes.draft-message", { agent: p.agent, digest: p.digest }, send, { refresh: false });
      if (!result) return;
      document.getElementById("commit-subject").value = result.subject;
      document.getElementById("commit-body").value = result.body;
      preview.hidden = true;
      document.getElementById("commit-subject").focus();
    });
  }

  function openNoteForm(button) {
    const slug = button.closest("[data-note-plan]")?.dataset.notePlan;
    if (!slug) { yoke().notify("There is no unfinished plan to save a note on."); return; }
    document.querySelectorAll(".note-form-host").forEach(n => n.remove());
    const text = el("textarea", { name: "text", rows: 3, maxLength: 4000, required: true });
    const save = el("button", { type: "submit", textContent: "Add note" });
    const cancel = el("button", { type: "button", className: "quiet", textContent: "Cancel" });
    const form = el("form", { className: "note-form" },
      el("label", {}, `Note on ${button.dataset.noteLabel}`, text),
      el("p", { className: "muted", textContent: `Saved on plan ${slug}. The agent reads it with yoke plan status.` }),
      el("div", { className: "actions" }, save, cancel));
    const row = button.closest("tr");
    let host;
    if (row) {
      host = el("tr", { className: "note-form-host" }, el("td", { colSpan: row.cells.length }, form));
      row.after(host);
    } else {
      host = el("div", { className: "note-form-host" }, form);
      (button.closest("[data-anchor]") || button.parentElement).after(host);
    }
    text.focus();
    cancel.addEventListener("click", () => { host.remove(); button.focus(); });
    text.addEventListener("keydown", e => {
      if (e.key === "Escape") { host.remove(); button.focus(); }
      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) { e.preventDefault(); form.requestSubmit(); }
    });
    form.addEventListener("submit", async e => {
      e.preventDefault();
      await run("note.add", { slug, anchor: button.dataset.noteAnchor, text: text.value }, save);
    });
  }

  document.addEventListener("click", event => {
    const target = event.target;
    const act = target.closest("[data-changes-act]");
    if (act) { run(act.dataset.changesAct, fromSelectors(act), act); return; }
    const layout = target.closest("[data-changes-layout]");
    if (layout) { prefs.set("yoke-changes-layout", layout.dataset.changesLayout); applyPreferences(); return; }
    const view = target.closest("[data-changes-view]");
    if (view) { prefs.set("yoke-changes-view", view.dataset.changesView); applyPreferences(); return; }
    if (target.closest("[data-changes-wrap]")) { prefs.set("yoke-changes-wrap", prefs.get("yoke-changes-wrap", "off") === "on" ? "off" : "on"); applyPreferences(); return; }
    const drafting = target.closest("[data-changes-draft]");
    if (drafting) { draft(drafting); return; }
    const add = target.closest(".note-add");
    if (add) { openNoteForm(add); return; }
    const exported = target.closest("[data-note-export]");
    if (exported) run("note.export", { slug: exported.dataset.noteExport }, exported, { refresh: false });
  });

  document.addEventListener("submit", event => {
    const form = event.target.closest("form[data-changes-form]");
    if (!form) return;
    event.preventDefault();
    const params = {};
    form.querySelectorAll("[name]").forEach(field => { params[field.name] = field.type === "checkbox" ? field.checked : field.value; });
    run(form.dataset.changesForm, params, form.querySelector("button[type=submit]") || form);
  });

  document.addEventListener("change", event => {
    const plan = event.target.closest("select[data-changes-plan]");
    if (!plan) return;
    const url = new URL(location.href);
    url.searchParams.set("plan", plan.value);
    yoke().navigate(url.href);
  });

  document.addEventListener("yoke:view", applyPreferences);
  applyPreferences();
})();
