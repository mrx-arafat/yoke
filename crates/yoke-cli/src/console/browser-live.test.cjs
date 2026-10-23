// Focused browser regressions for the shipped script; no human console or real state needed.
// NODE_PATH=<playwright installation>/node_modules node --test <this file>
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { chromium } = require('playwright');

const fragment = label => `<main tabindex="-1" data-page="decisions"><h1>${label}</h1>
  <form data-filter action="decisions"><input name="tool" value=""><select name="outcome"><option value="">All</option><option value="deny">Denied</option></select><button>Apply filters</button></form>
  <details><summary data-row>A decision</summary>Evidence</details></main>`;
const shell = `<html><head><meta name="yoke-csrf" content="fixture"></head><body>
  <nav aria-label="Workspace"><a href="decisions" data-nav>Decisions</a></nav>
  <span id="connection">Connecting...</span><select id="theme"><option value="system">System</option></select>
  <button id="palette-open">Commands</button><button id="help-open">Help</button>
  <div id="notice" hidden>New activity <button data-refresh>Refresh view</button></div>
  <div id="view">${fragment('Initial')}</div><div id="announcement"></div><div id="toast" hidden></div>
  <dialog id="palette"><input id="command-search"></dialog><dialog id="help"></dialog>
  <dialog id="lightbox"><img id="lightbox-image"><p id="image-caption"></p><button id="image-prev"></button><button id="image-next"></button></dialog>
  <script src="assets/console.js"></script></body></html>`;

test('live updates preserve drafts, catch up on ready, and defer while reading', async () => {
  const browser = await chromium.launch({ executablePath: process.env.YOKE_CHROMIUM });
  try {
    const page = await browser.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.EventSource = class extends EventTarget {
        static CLOSED = 2;
        constructor() { super(); window.stream = this; }
        close() {}
      };
    });
    let requests = 0;
    let hold;
    let release;
    await page.route('http://console.test/**', async route => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith('/console.js')) {
        await route.fulfill({ contentType: 'text/javascript', body: readFileSync(`${__dirname}/console.js`, 'utf8') });
      } else if (route.request().headers()['x-yoke-fragment']) {
        requests++;
        if (hold) await hold;
        await route.fulfill({ contentType: 'text/html', body: fragment(`Update ${requests}`) });
      } else {
        await route.fulfill({ contentType: 'text/html', body: shell });
      }
    });
    await page.goto('http://console.test/decisions');
    const emit = name => page.evaluate(name => window.stream.dispatchEvent(new Event(name)), name);
    await emit('ready');
    await page.waitForFunction(() => document.querySelector('h1').textContent === 'Update 1');
    assert.equal(await page.locator('#connection').textContent(), 'Live');

    await page.locator('input[name=tool]').fill('Bash');
    await page.locator('h1').click();
    await emit('change');
    assert.equal(await page.locator('#notice').isVisible(), true);
    assert.equal(await page.locator('input[name=tool]').inputValue(), 'Bash');
    assert.equal(requests, 1);
    // Reconnects use the same guard, including an edited select that has lost focus.
    await page.locator('input[name=tool]').fill('');
    await page.locator('select[name=outcome]').selectOption('deny');
    await page.locator('h1').click();
    await emit('ready');
    assert.equal(requests, 1);
    await page.getByRole('button', { name: 'Apply filters' }).click();
    await page.waitForFunction(() => document.querySelector('h1').textContent === 'Update 2');
    assert.match(page.url(), /outcome=deny/);

    // Nothing need change after reconnection to trigger catch-up.
    await page.locator('h1').click();
    await emit('ready');
    await page.waitForFunction(() => document.querySelector('h1').textContent === 'Update 3');
    await page.locator('summary').click();
    await emit('change');
    assert.equal(requests, 3);
    assert.equal(await page.locator('#notice').isVisible(), true);
    await page.getByRole('button', { name: 'Refresh view' }).click();
    await page.waitForFunction(() => document.querySelector('h1').textContent === 'Update 4');

    // A draft made while a background request is in flight must also survive.
    hold = new Promise(resolve => { release = resolve; });
    await page.locator('h1').click();
    const requested = page.waitForRequest(request => request.headers()['x-yoke-fragment'] === '1');
    await emit('change');
    await requested;
    await page.locator('input[name=tool]').fill('Edit');
    await page.locator('h1').click();
    release();
    await page.waitForFunction(() => !document.querySelector('#view').hasAttribute('aria-busy'));
    assert.equal(await page.locator('input[name=tool]').inputValue(), 'Edit');
    assert.equal(await page.locator('h1').textContent(), 'Update 4');
    assert.equal(await page.locator('#notice').isVisible(), true);

    // A stream event must not abort a deliberate navigation or hide pending activity.
    hold = new Promise(resolve => { release = resolve; });
    const applied = page.waitForRequest(request => request.headers()['x-yoke-fragment'] === '1');
    await page.getByRole('button', { name: 'Apply filters' }).click();
    await applied;
    await emit('change');
    assert.equal(requests, 6);
    release();
    await page.waitForFunction(() => document.querySelector('h1').textContent === 'Update 6');
    assert.equal(new URL(page.url()).searchParams.get('tool'), 'Edit');
    assert.equal(await page.locator('#notice').isVisible(), true);
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
  }
});
