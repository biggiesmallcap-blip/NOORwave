// API and socket fixtures exercise the real Settings route without changing audio output.
import assert from 'node:assert/strict';
import { chromium, type WebSocketRoute } from 'playwright';

const base = process.argv[2] ?? 'http://127.0.0.1:17601';
const browser = await chromium.launch({ headless: true, channel: process.env.NOOR_BROWSER_CHANNEL });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await context.newPage();
page.setDefaultTimeout(10000);
const errors: string[] = [];
page.on('pageerror', (error) => errors.push(error.message));
await context.addInitScript(() => {
	localStorage.setItem('noor_api_token', 'exclusive-warning-test');
	localStorage.setItem('noor.onboarding.complete.exclusive-warning-test', '1');
	localStorage.setItem('noor.welcome.v1-0.seen', '1');
});
let socket: WebSocketRoute;
let onConnected: () => void;
const connected = new Promise<void>((resolve) => { onConnected = resolve; });
await context.routeWebSocket((url) => url.pathname === '/ws', (route) => {
	socket = route;
	onConnected();
});
let settings = {
	quality: 'HI_RES_LOSSLESS', output_device: 'Realtek Digital Output', exclusive_mode: true,
	sample_rate_follow: true, exclusive_latency_mode: 'STABLE', exclusive_release_grace_secs: 30,
	exclusive_release_on_pause: false, video_quality_mode: 'AUTO',
};
let retries = 0;
const devices = [
	{ id: 'Wasapi:realtek', name: 'Realtek Digital Output', is_default: true },
	{ id: 'Wasapi:speakers-a', name: 'Speakers', is_default: false },
	{ id: 'Wasapi:speakers-b', name: 'Speakers', is_default: false },
];
await context.route((url) => url.pathname.startsWith('/api/'), async (route) => {
	const path = new URL(route.request().url()).pathname;
	let data: unknown = {};
	if (path === '/api/audio/settings') {
		if (route.request().method() === 'PUT') settings = route.request().postDataJSON();
		data = settings;
	} else if (path === '/api/audio/devices') data = { devices };
	else if (path === '/api/audio/exclusive/retry') { retries++; data = { ok: true }; }
	else if (path === '/api/remote/identity') data = { server_id: 'exclusive-test', name: 'NOORwave', protocol: 1, pairing_available: false };
	else if (path === '/api/setup/onboarding') data = { complete: true };
	else if (path === '/api/status') data = { version: '1.0.5' };
	else if (path === '/api/tracks') data = { tracks: [], total: 0 };
	else if (path === '/api/albums') data = { albums: [], total: 0 };
	else if (path === '/api/playlists') data = { playlists: [] };
	else if (path === '/api/genres') data = { genres: [] };
	else if (path.includes('/queue')) data = { queue: [] };
	else if (path === '/api/playback/state') data = { state: { current_track: null, is_playing: false, volume: 0.5, position_ms: 0, shuffle_mode: 'off', repeat_mode: 'off' }, queue: [] };
	await route.fulfill({ json: data });
});

try {
	await page.goto(`${base}/settings?category=playback`, { waitUntil: 'domcontentloaded' });
	await page.getByRole('combobox', { name: 'Output preset' }).waitFor();
	await Promise.race([
		connected,
		new Promise<never>((_, reject) => setTimeout(() => reject(new Error('Shell did not connect its WebSocket')), 5000)),
	]);
	const failure = { type: 'audio_exclusive_failed', device: settings.output_device, reason: 'Exclusive mode failed: format rejected (HRESULT 0x88890008)' };
	socket!.send(JSON.stringify(failure));
	socket!.send(JSON.stringify(failure));
	await page.locator('.exclusive-failed-banner').first().waitFor();
	await page.locator('.toast--error').first().waitFor();
	// Await processing of both socket messages before counting notifications.
	await page.waitForFunction(() => document.querySelectorAll('.toast--error').length > 0);
	await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
	assert.equal(await page.locator('.exclusive-failed-banner').count(), 1, 'One persistent failure banner');
	assert.equal(await page.locator('.toast--error').count(), 1, 'Repeated events must not stack identical notifications');
	console.log('PASS: repeated socket failures produce one banner and one toast');
	const retryResponse = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/audio/exclusive/retry');
	await page.getByRole('button', { name: 'Retry', exact: true }).click();
	await retryResponse;
	assert.equal(retries, 1);
	console.log('PASS: the remaining Retry control calls the real API path');
	socket!.send(JSON.stringify({ type: 'audio_exclusive_engaged', device: settings.output_device, transport_format: 'i24-in-32' }));
	await page.locator('.exclusive-failed-banner').waitFor({ state: 'detached' });
	console.log('PASS: successful exclusive engagement clears the failure banner');
	socket!.send(JSON.stringify(failure));
	await page.locator('.exclusive-failed-banner').waitFor();
	await page.getByRole('button', { name: 'Disable exclusive', exact: true }).click();
	await page.locator('.exclusive-failed-banner').waitFor({ state: 'detached' });
	assert.equal(settings.exclusive_mode, false);
	assert.deepEqual(errors, []);
	console.log('PASS: Disable exclusive saves the setting and clears the banner');
	const output = page.getByRole('combobox', { name: 'Output device', exact: true });
	assert.equal(await output.inputValue(), 'Wasapi:realtek', 'Unique legacy name resolves to its endpoint');
	const deviceSaved = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/audio/settings' && response.request().method() === 'PUT' && response.request().postDataJSON().output_device === 'Wasapi:speakers-b');
	await output.selectOption('Wasapi:speakers-b');
	await deviceSaved;
	assert.equal(settings.output_device, 'Wasapi:speakers-b', 'Duplicate names persist the selected endpoint ID');
	await page.reload({ waitUntil: 'domcontentloaded' });
	await output.locator('option[value="Wasapi:speakers-b"]').waitFor({ state: 'attached' });
	assert.equal(await output.inputValue(), 'Wasapi:speakers-b', 'Exact endpoint survives Settings reload');
	for (const saved of ['Disconnected DAC', 'Speakers']) {
		settings.output_device = saved;
		await page.evaluate(() => {
			for (const key of Object.keys(localStorage)) {
				if (key.startsWith('noor.query.')) localStorage.removeItem(key);
			}
		});
		await page.reload({ waitUntil: 'domcontentloaded' });
		await output.getByRole('option', { name: `Unavailable: ${saved}`, exact: true }).waitFor({ state: 'attached' });
		assert.equal(await output.inputValue(), saved, 'Missing or ambiguous preference must not display System default');
		assert.equal(await output.getByRole('option', { name: `Unavailable: ${saved}`, exact: true }).evaluate((option: HTMLOptionElement) => option.disabled), true);
	}
	assert.deepEqual(errors, []);
	console.log('PASS: stable IDs, unique legacy names and unavailable saved devices use the real Settings control');
} catch (error) {
	console.error('Route state:', (await page.locator('body').innerText()).slice(0, 1000), errors);
	throw error;
} finally {
	await browser.close();
}
