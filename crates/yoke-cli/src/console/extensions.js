"use strict";
// Owner: parity-extensions. Extensions page. Loaded on every page after console.js.
// The server renders every control with data-* attributes; this script posts actions through window.yoke,
// shows the confirmation the server asks for (its text names the exact paths), and refreshes the view.
(() => {
  if (window.yokeExtensionsInstalled) return;
  window.yokeExtensionsInstalled = true;
  const yoke = () => window.yoke;
  const el = (tag, props = {}, ...children) => {
    const node = document.createElement(tag);
    Object.assign(node, props);
    node.append(...children);
    return node;
  };
  const HOSTS = ".ext-actions, .ext-remote-form, form, .actions";

  function showError(source, message) {
    const host = source.closest(HOSTS) || source.parentElement;
    host.parentElement.querySelectorAll(":scope > .action-error").forEach(n => n.remove());
    host.after(el("p", { className: "action-error error", role: "alert", textContent: message }));
  }

  // Show exactly what will happen; resolve with the confirmed result, or null when cancelled.
  function askConfirm(source, confirm) {
    return new Promise((resolve, reject) => {
      document.querySelectorAll(".ext-confirm").forEach(n => n.remove());
      const yes = el("button", { type: "button", textContent: "Confirm" });
      const no = el("button", { type: "button", className: "quiet", textContent: "Cancel" });
      const box = el("div", { className: "confirmation ext-confirm", role: "group" },
        el("p", { className: "confirm-text", textContent: confirm.message }), el("div", { className: "actions" }, yes, no));
      box.setAttribute("aria-label", "Confirm this action");
      (source.closest(HOSTS) || source).after(box);
      yes.focus();
      const close = () => { box.remove(); source.focus({ preventScroll: true }); };
      no.addEventListener("click", () => { close(); resolve(null); });
      box.addEventListener("keydown", e => { if (e.key === "Escape") { close(); resolve(null); } });
      yes.addEventListener("click", async () => {
        yes.disabled = true;
        try { const data = await yoke().confirm(confirm.ticket); box.remove(); resolve(data); }
        catch (error) { box.remove(); reject(error); }
      });
    });
  }

  async function run(source, action, params) {
    source.disabled = true;
    source.setAttribute("aria-busy", "true");
    yoke().announce("Working");
    try {
      let data = await yoke().act(action, params);
      if (data.confirm) { data = await askConfirm(source, data.confirm); if (!data) return null; }
      if (data.message) yoke().notify(data.message);
      if (data.navigate) await yoke().navigate(data.navigate); else await yoke().refresh();
      return data;
    } catch (error) {
      showError(source, error.message);
      return null;
    } finally {
      source.disabled = false;
      source.removeAttribute("aria-busy");
    }
  }

  const origin = node => node.closest("[data-ext-origin]")?.dataset.extOrigin;

  document.addEventListener("submit", event => {
    const form = event.target.closest("form[data-ext-rename], form[data-ext-remote-connect], form[data-ext-share-form]");
    if (!form) return;
    event.preventDefault();
    const button = form.querySelector("button[type=submit]");
    if (form.matches("[data-ext-rename]")) {
      run(button, "ext.rename", { origin: origin(form), name: form.elements.name.value.trim() });
    } else if (form.matches("[data-ext-remote-connect]")) {
      run(button, "ext.remote-connect", { url: form.elements.url.value.trim(), branch: form.elements.branch.value.trim() });
    } else {
      const origins = [...form.querySelectorAll("input[name=share]:checked")].map(box => box.value);
      if (!origins.length) { showError(button, "Tick at least one extension to share."); return; }
      run(button, "ext.remote-push", { origins });
    }
  });

  document.addEventListener("click", event => {
    const move = event.target.closest("[data-ext-move]");
    if (move) { run(move, "ext.move", { origin: origin(move), scope: move.dataset.extMove }); return; }
    const del = event.target.closest("[data-ext-delete]");
    if (del) { run(del, "ext.delete", { origin: origin(del) }); return; }
    const pull = event.target.closest("[data-ext-remote-pull]");
    if (pull) { run(pull, "ext.remote-pull", {}); return; }
    const leave = event.target.closest("[data-ext-remote-disconnect]");
    if (leave) { run(leave, "ext.remote-disconnect", {}); return; }
    const install = event.target.closest("[data-ext-install]");
    if (install) run(install, "ext.remote-install", { id: install.dataset.extInstall, scope: install.dataset.extScope });
  });

  window.yokeExtensions = Object.freeze({ run, askConfirm, el });
})();
