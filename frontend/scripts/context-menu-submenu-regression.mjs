// Browser regression: expanding a submenu near the bottom of the window must
// not flip the menu away from the pointer (pointerleave closes the menu).
// Run against a dev server: CONTEXT_MENU_TEST_URL=http://127.0.0.1:17703
import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const baseUrl = process.env.CONTEXT_MENU_TEST_URL ?? 'http://127.0.0.1:17703';
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const page = await browser.newPage({ viewport: { width: 1280, height: 1400 } });
const pageErrors = [];
page.on('pageerror', error => pageErrors.push(error.message));

try {
	await page.route(new URL('/context-menu-test', baseUrl).href, route => route.fulfill({
		contentType: 'text/html', body: '<html><body style="margin:0"><div id="root"></div></body></html>'
	}));
	await page.goto(new URL('/context-menu-test', baseUrl).href);
	await page.evaluate(async () => {
		const harness = await import('/src/lib/components/fixtures/ContextMenuHarness.svelte');
		harness.mount(harness.ContextMenu, { target: document.querySelector('#root') });
		const item = label => ({ label, onSelect: () => {} });
		const items = [
			item('Move next'), { label: '', separator: true }, item('Song radio'), item('Play album'),
			item('Shuffle album'), item('Artist radio'), { label: '', separator: true },
			item('Go to artist'), item('Go to album'), { label: '', separator: true },
			item('Remove from favourites'),
			{ label: 'Download', submenu: ['FLAC', 'Hi-Res FLAC', 'AAC 320', 'AAC 96'].map(item) },
			item('Not for me'), item('Remove from queue')
		];
		harness.openContextMenu({ clientX: 400, clientY: 900 }, items, 'Even Flow');
		await harness.tick();
	});

	const menu = page.locator('.context-menu');
	await menu.waitFor();
	await page.waitForTimeout(300);
	const before = await menu.boundingBox();
	assert.ok(before.y >= 880, `menu should open below the click, got top=${before.y}`);

	const download = menu.getByRole('menuitem', { name: 'Download' });
	await download.click();
	await page.waitForTimeout(400);

	assert.equal(await menu.count(), 1, 'menu closed after expanding Download');
	const after = await menu.boundingBox();
	assert.ok(after.height > before.height, 'Download submenu did not expand');
	assert.ok(after.y + after.height <= 1400 - 8 + 0.5, `menu runs off screen: bottom=${after.y + after.height}`);
	assert.ok(after.y >= before.y + before.height - after.height - 0.5, `menu flipped away: top ${before.y} -> ${after.y}`);

	await menu.getByRole('menuitem', { name: 'AAC 320' }).click();
	await page.waitForTimeout(300);
	assert.equal(await menu.count(), 0, 'selecting a format should close the menu');
	assert.deepEqual(pageErrors, []);
	console.log(JSON.stringify({ before, after, formatReachable: true, pageErrors }));
} finally {
	await browser.close();
}
