"use strict";
// Loaded on every page after console.js; uses window.yoke (act, refresh, navigate, notify).
// One generic flow serves every plans., prd. and docs. control: a button carries the action name and its
// parameters; inputs marked data-field in the same zone add to them. An action that needs the human's
// confirmation shows exactly what will happen, and only the Confirm button runs it.
(() => {
  const csrf = () => document.querySelector('meta[name="yoke-csrf"]').content;
  const zoneOf = button => button.closest(".action-zone") || button.parentElement;

  async function confirmTicket(ticket) {
    const response = await fetch("actions", {
      method: "POST",
      headers: {"X-Yoke-CSRF": csrf(), "Content-Type": "application/json"},
      body: JSON.stringify({action: "confirm", ticket}),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.message || "Action failed. Try again.");
    return result.data;
  }

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

  async function finish(data, button) {
    window.yoke.notify(data.message || "Done.");
    const next = button.dataset.after;
    if (next) await window.yoke.navigate(next);
    else await window.yoke.refresh();
  }

  function confirmation(zone, button, confirm) {
    const box = document.createElement("div");
    box.className = "confirmation";
    box.setAttribute("role", "group");
    const text = document.createElement("p");
    text.textContent = confirm.message;
    const yes = document.createElement("button");
    yes.type = "button";
    yes.className = "danger-button";
    yes.textContent = button.dataset.confirmLabel || "Confirm";
    const no = document.createElement("button");
    no.type = "button";
    no.className = "quiet";
    no.textContent = "Cancel";
    no.addEventListener("click", () => { box.remove(); button.focus(); });
    yes.addEventListener("click", async () => {
      yes.disabled = true;
      try { await finish(await confirmTicket(confirm.ticket), button); }
      catch (error) { box.remove(); showError(zone, error); }
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
      const params = JSON.parse(button.dataset.params || "{}");
      for (const field of zone.querySelectorAll("[data-field]")) params[field.dataset.field] = field.value;
      const data = await window.yoke.act(button.dataset.plansRun, params);
      if (data.confirm) confirmation(zone, button, data.confirm);
      else await finish(data, button);
    } catch (error) {
      showError(zone, error);
    } finally {
      button.disabled = false;
    }
  }

  document.addEventListener("click", event => {
    const button = event.target.closest("[data-plans-run]");
    if (button) run(button);
  });

  // A link into another documentation page carries a #doc- anchor; in-console navigation does not scroll to it.
  function scrollToDocAnchor() {
    if (!location.hash.startsWith("#doc-")) return;
    try { document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView(); } catch (_) { /* malformed hash */ }
  }
  document.addEventListener("yoke:view", scrollToDocAnchor);
  window.addEventListener("DOMContentLoaded", scrollToDocAnchor);
})();
