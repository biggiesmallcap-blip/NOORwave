// Run against an isolated Windows desktop build with WebView2 remote debugging.
// Usage: node scripts/tauri-zoom-smoke.mjs http://127.0.0.1:18766 http://127.0.0.1:17611 [Portable|Installed]
import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const [debugUrl, appUrl, installMode = 'Portable'] = process.argv.slice(2);
assert.ok(['Portable', 'Installed'].includes(installMode), 'Expected Portable or Installed mode.');
if (!debugUrl || !appUrl) throw new Error('Supply the isolated WebView2 debug URL and app URL.');
for (const value of [debugUrl, appUrl]) {
  const url = new URL(value);
  assert.equal(url.protocol, 'http:');
  assert.equal(url.hostname, '127.0.0.1');
}
const browser = await chromium.connectOverCDP(debugUrl);
try {
  const pages = browser.contexts().flatMap(context => context.pages());
  assert.equal(pages.length, 1, 'Expected exactly one isolated app WebView.');
  const page = pages[0];
  const zoomErrors = [];
  page.on('console', message => {
    if (message.text().includes('set_webview_zoom failed')) zoomErrors.push(message.text());
  });
  await page.goto(appUrl);
  await page.waitForFunction(() => window.__TAURI_INTERNALS__?.invoke && document.querySelector('main'));
  if (new URL(page.url()).pathname === '/onboarding') {
    await page.getByRole('button', { name: 'Skip for now', exact: false }).click();
    await page.waitForURL(`${appUrl}/`);
  }
  const cdp = await page.context().newCDPSession(page);
  // Server-rendered markup appears before Svelte mounts its input handlers.
  for (let attempt = 0; ; attempt++) {
    const result = await cdp.send('Runtime.evaluate', {
      expression: 'Boolean(getEventListeners(window).wheel?.length && getEventListeners(window).keydown?.length)',
      includeCommandLineAPI: true, returnByValue: true,
    });
    if (result.result.value) break;
    assert.ok(attempt < 100, 'Frontend input handlers did not mount.');
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  const invoke = command => page.evaluate(command => window.__TAURI_INTERNALS__.invoke(command), command);
  assert.equal(await invoke('get_install_mode'), installMode);
  assert.equal(typeof await invoke('get_minimize_to_tray'), 'boolean');
  assert.equal(typeof await invoke('get_remote_host_state'), 'object');
  assert.equal(typeof await invoke('get_startup_state'), 'object');
  await page.keyboard.press('Control+0');
  await page.waitForFunction(() => localStorage.getItem('noor-ui-zoom') === '1');
  const baseline = await page.evaluate(() => devicePixelRatio);

  async function expectZoom(value) {
    await page.waitForFunction(({value, baseline}) =>
      Number(localStorage.getItem('noor-ui-zoom')) === value &&
      Math.abs(devicePixelRatio / baseline - value) < 0.01,
    { value, baseline });
  }
  async function wheel(deltaY, ctrl = true) {
    await cdp.send('Input.dispatchMouseEvent', {
      type: 'mouseWheel', x: 100, y: 100, deltaX: 0, deltaY,
      modifiers: ctrl ? 2 : 0,
    });
  }
  await wheel(-120);
  await expectZoom(1.05);
  await wheel(120);
  await expectZoom(1);
  await page.keyboard.press('Control+=');
  await expectZoom(1.1);
  await page.reload();
  await expectZoom(1.1);
  await page.keyboard.press('Control+-');
  await expectZoom(1);

  // Ordinary wheel events must continue to scroll and leave zoom unchanged.
  await page.evaluate(() => {
    const pane = document.createElement('div');
    pane.id = 'noor-scroll-smoke';
    pane.style.cssText = 'position:fixed;left:0;top:0;width:300px;height:300px;overflow:auto;z-index:2147483647';
    pane.innerHTML = '<div style="height:3000px">Scroll smoke fixture</div>';
    document.body.append(pane);
  });
  await wheel(120, false);
  await page.waitForFunction(() => document.getElementById('noor-scroll-smoke').scrollTop > 0);
  await expectZoom(1);
  await page.evaluate(() => document.getElementById('noor-scroll-smoke').remove());

  for (let i = 1; i <= 21; i++) {
    await wheel(-120);
    await expectZoom(Math.min(2, Math.round((1 + i * 0.05) * 100) / 100));
  }
  for (let i = 1; i <= 31; i++) {
    await wheel(120);
    await expectZoom(Math.max(0.5, Math.round((2 - i * 0.05) * 100) / 100));
  }
  await page.keyboard.press('Control+0');
  await expectZoom(1);
  assert.deepEqual(zoomErrors, []);
  console.log(JSON.stringify({ appUrl, installMode, baseline, passed: [
    'desktop commands', 'Ctrl+wheel in/out', 'Ctrl+plus/minus/reset',
    'persisted zoom after reload', 'ordinary wheel scrolling', '50–200% bounds',
  ] }));
} finally {
  await browser.close();
}
