"use strict";
// System page. Loaded on every page after console.js.
// Server-rendered buttons carry data-system-act (a system.* action) and data-params (JSON). This
// script posts the action, shows the exact confirmation text the server returns, confirms only
// when the human clicks Confirm, then re-renders the view. No inline script or style.
(() => {
  if (window.yokeSystemInstalled) return;
  window.yokeSystemInstalled = true;
  const yoke = () => window.yoke;
  const el = (tag, props = {}, ...children) => {
    const node = document.createElement(tag);
    Object.assign(node, props);
    node.append(...children);
    return node;
  };

  function showError(source, message) {
    const zone = source.closest(".action-zone") || source.parentElement;
    zone.querySelectorAll(":scope > .action-error").forEach(n => n.remove());
    zone.append(el("p", { className: "action-error error", role: "alert", textContent: message }));
  }

  // Resolve with the confirmed result, or null when the human cancels.
  function askConfirm(source, confirm) {
    return new Promise((resolve, reject) => {
      document.querySelectorAll(".sys-confirm").forEach(n => n.remove());
      const yes = el("button", { type: "button", className: "danger-button", textContent: "Confirm" });
      const no = el("button", { type: "button", className: "quiet", textContent: "Cancel" });
      const box = el("div", { className: "confirmation sys-confirm", role: "group" },
        el("p", { textContent: confirm.message }), el("div", { className: "actions" }, yes, no));
      box.setAttribute("aria-label", "Confirm this action");
      (source.closest(".action-zone") || source.parentElement).append(box);
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

  async function run(button) {
    if (button.disabled) return;
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
    yoke().announce("Working");
    try {
      let data = await yoke().act(button.dataset.systemAct, JSON.parse(button.dataset.params || "{}"));
      if (data.confirm) {
        data = await askConfirm(button, data.confirm);
        if (!data) return;
      }
      await yoke().refresh();
      if (data.message) yoke().notify(data.message);
    } catch (error) {
      showError(button, error.message);
    } finally {
      button.disabled = false;
      button.removeAttribute("aria-busy");
    }
  }

  document.addEventListener("click", event => {
    const button = event.target.closest("[data-system-act]");
    if (button) run(button);
  });
})();
