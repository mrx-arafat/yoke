"use strict";
// Sessions page, loaded on every page after console.js: the copy-ID button
// on a session's detail page. Everything else on the page is server-rendered links and forms.
(() => {
  if (window.yokeSessionsInstalled) return;
  window.yokeSessionsInstalled = true;

  // Select the visible ID so the human can copy it by hand when the clipboard is unavailable.
  function selectId(button) {
    const code = button.closest(".panel")?.querySelector("#session-id code");
    if (!code) return;
    const range = document.createRange();
    range.selectNodeContents(code);
    const selection = window.getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
  }

  document.addEventListener("click", async event => {
    const button = event.target.closest("[data-copy-session]");
    if (!button) return;
    const id = button.dataset.copySession;
    try {
      if (!navigator.clipboard) throw new Error("no clipboard");
      await navigator.clipboard.writeText(id);
      window.yoke.notify("Session ID copied.");
    } catch (_) {
      selectId(button);
      window.yoke.notify("Copying is blocked here; the ID is selected, so press Cmd or Ctrl C.");
    }
  });
})();
