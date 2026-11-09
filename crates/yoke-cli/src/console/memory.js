"use strict";
// Loaded on every page after console.js; uses window.yoke (act, confirm, refresh, notify).
// A button with data-memory-run names a memory., session. or usage. action and carries its params in
// data-params. The server answers an action that sends data anywhere with a confirmation that says exactly
// what will be sent; only the Confirm button runs it.
(() => {
  const zoneOf = button => button.closest(".action-zone") || button.parentElement;

  function clear(zone) {
    zone.querySelectorAll(".confirmation,.action-error").forEach(node => node.remove());
  }

  function showError(zone, error) {
    const message = document.createElement("p");
    message.className = "action-error error";
    message.setAttribute("role", "alert");
    message.textContent = error.message;
    zone.append(message);
  }

  async function finish(data) {
    window.yoke.notify(data.message || "Done.");
    await window.yoke.refresh();
  }

  function confirmation(zone, button, confirm) {
    const box = document.createElement("div");
    box.className = "confirmation";
    box.setAttribute("role", "group");
    box.setAttribute("aria-label", "Confirm what will be sent");
    const text = document.createElement("p");
    text.textContent = confirm.message;
    const yes = document.createElement("button");
    yes.type = "button";
    yes.textContent = button.dataset.confirmLabel || "Confirm";
    const no = document.createElement("button");
    no.type = "button";
    no.className = "quiet";
    no.textContent = "Cancel";
    no.addEventListener("click", () => { box.remove(); button.focus(); });
    yes.addEventListener("click", async () => {
      yes.disabled = true;
      no.disabled = true;
      try { await finish(await window.yoke.confirm(confirm.ticket)); }
      catch (error) { box.remove(); showError(zone, error); button.focus(); }
    });
    box.append(text, yes, no);
    zone.append(box);
    yes.focus();
  }

  async function run(button) {
    const zone = zoneOf(button);
    clear(zone);
    button.disabled = true;
    try {
      const data = await window.yoke.act(button.dataset.memoryRun, JSON.parse(button.dataset.params || "{}"));
      if (data.confirm) confirmation(zone, button, data.confirm);
      else await finish(data);
    } catch (error) {
      showError(zone, error);
    } finally {
      button.disabled = false;
    }
  }

  document.addEventListener("click", event => {
    const button = event.target.closest("[data-memory-run]");
    if (button) run(button);
  });
})();
