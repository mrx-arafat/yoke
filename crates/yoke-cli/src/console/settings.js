"use strict";
// Workflow settings form, loaded on every page after console.js.
// Saving posts settings.save through window.yoke; the server answers with the exact file diff to
// confirm, writes the file only after the human confirms, and never trusts it. Unsaved edits are
// guarded against in-console navigation and page unload.
(() => {
  if (window.yokeSettingsInstalled) return;
  window.yokeSettingsInstalled = true;
  const yoke = () => window.yoke;
  const LEAVE = "You have unsaved settings edits. Leave this page and lose them?";
  const form = () => document.querySelector("main[data-page='settings'] form[data-settings-form]");
  const edited = f => [...f.querySelectorAll("[data-setting]")].filter(el =>
    el.tagName === "SELECT" ? !el.options[el.selectedIndex]?.defaultSelected : el.value !== el.defaultValue);
  const dirty = () => { const f = form(); return Boolean(f && edited(f).length); };
  // A navigation the human already agreed to leave for; cleared on every new view.
  let leaving = false;

  function show(f, text, alert) {
    f.querySelectorAll(".settings-message").forEach(n => n.remove());
    const p = document.createElement("p");
    p.className = alert ? "settings-message action-error error" : "settings-message muted";
    if (alert) p.setAttribute("role", "alert");
    p.textContent = text;
    f.querySelector(".actions").after(p);
  }

  // Show the exact diff; resolve with the confirmed result, or null when cancelled.
  function review(f, button, confirm) {
    return new Promise((resolve, reject) => {
      f.querySelectorAll(".settings-confirm").forEach(n => n.remove());
      const box = document.createElement("div");
      box.className = "confirmation settings-confirm";
      box.setAttribute("role", "group");
      box.setAttribute("aria-label", "Review the settings change");
      const diff = document.createElement("pre");
      diff.className = "settings-diff";
      diff.tabIndex = 0;
      diff.textContent = confirm.message;
      const yes = document.createElement("button");
      yes.type = "button"; yes.textContent = "Write the file";
      const no = document.createElement("button");
      no.type = "button"; no.className = "quiet"; no.textContent = "Cancel";
      const actions = document.createElement("div");
      actions.className = "actions";
      actions.append(yes, no);
      box.append(diff, actions);
      f.querySelector(".actions").after(box);
      diff.focus();
      const close = () => { box.remove(); button.focus({ preventScroll: true }); };
      no.addEventListener("click", () => { close(); resolve(null); });
      box.addEventListener("keydown", e => { if (e.key === "Escape") { close(); resolve(null); } });
      yes.addEventListener("click", async () => {
        yes.disabled = true;
        try { const data = await yoke().confirm(confirm.ticket); box.remove(); resolve(data); }
        catch (error) { box.remove(); reject(error); }
      });
    });
  }

  document.addEventListener("submit", async event => {
    const f = event.target.closest("form[data-settings-form]");
    if (!f) return;
    event.preventDefault();
    const button = f.querySelector("button[type=submit]");
    const changes = edited(f);
    if (!changes.length) { show(f, "Nothing to save: every value matches the settings in effect."); return; }
    const target = f.elements.target.value;
    const values = Object.fromEntries(changes.map(el => [el.name, el.value.trim()]));
    const base = target === "global" ? f.dataset.baseGlobal : f.dataset.baseProject;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    try {
      let data = await yoke().act("settings.save", { target, base, values });
      if (data.confirm) { data = await review(f, button, data.confirm); if (!data) return; }
      // Saved, or nothing to write: the form is clean again, so refreshing asks nothing.
      f.reset();
      yoke().notify(data.message);
      await yoke().refresh();
    } catch (error) {
      show(f, error.message, true);
    } finally {
      button.disabled = false;
      button.removeAttribute("aria-busy");
    }
  });

  document.addEventListener("reset", event => {
    const f = event.target.closest("form[data-settings-form]");
    if (f) f.querySelectorAll(".settings-message, .settings-confirm").forEach(n => n.remove());
  });

  // In-console links and palette entries: ask before dropping edits (capture runs before console.js).
  document.addEventListener("click", event => {
    if (!dirty() || event.metaKey || event.ctrlKey || event.shiftKey) return;
    if (!event.target.closest("a[data-nav], [data-destination], [data-refresh]")) return;
    if (window.confirm(LEAVE)) { leaving = true; return; }
    event.preventDefault();
    event.stopImmediatePropagation();
  }, true);

  // Keyboard shortcuts, history and live refreshes, once console.js fires this cancelable event.
  document.addEventListener("yoke:before-navigate", event => {
    if (!dirty() || leaving) return;
    if (event.detail?.live || !window.confirm(LEAVE)) event.preventDefault();
  });

  window.addEventListener("beforeunload", event => {
    if (!dirty()) return;
    event.preventDefault();
    event.returnValue = "";
  });

  document.addEventListener("yoke:view", () => { leaving = false; });
})();
