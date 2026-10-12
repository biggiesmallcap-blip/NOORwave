import assert from 'node:assert/strict';
import { chromium } from 'playwright';

// Run against pnpm dev on an isolated port. Requests are intercepted so the
// interaction check never edits the user's library, TIDAL account, or queue.
const browser = await chromium.launch({ headless: true,
	...(process.env.NOOR_SMOKE_BROWSER ? { executablePath: process.env.NOOR_SMOKE_BROWSER } : {}) });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await context.newPage();
const errors = [];
const reads = [];
const writes = [];
let failLoad = false;
let failSave = false;
let refreshed = false;
let savedPlaylist = null;
let emptyMix = false;
let holdFirstLoad = true;
let releaseFirstLoad;
const firstLoad = new Promise((resolve) => { releaseFirstLoad = resolve; });
const tracks = [42, 43, 42].map((tidal_id, index) => ({ tidal_id, title: `Smoke Song ${index + 1}`,
	artist_name: 'Smoke Artist', artist_tidal_id: 17, album_tidal_id: 19, album_title: 'Smoke Album', duration_ms: 120000, artwork_url: null }));
const playlist = { id: 1, name: 'Existing playlist', tidal_uuid: 'existing', is_smart: false, track_count: 2 };
const playbackState = { current_track: null, current_queue_item_id: null, is_playing: false,
	volume: 1, shuffle_mode: 'off', repeat_mode: 'off', position_ms: 0, automix_enabled: false,
	crossfade_ms: 0, automix_discover_new: false, automix_use_learning: true, automix_allow_external: false };
page.on('pageerror', (error) => errors.push(error.stack));
await context.addInitScript(() => localStorage.setItem('noor_api_token', 'test-token'));
await page.routeWebSocket('**/ws?*', () => {});
await page.route('**/api/**', async (route) => {
	const request = route.request();
	const path = new URL(request.url()).pathname;
	if (!path.startsWith('/api/')) return route.continue();
	reads.push(path);
	let body = { modules: [], articles: [], news: [], stations: [], tracks: [], devices: [], genres: [], listens: [] };
	if (request.method() !== 'GET') writes.push({ path, body: request.postDataJSON() });
	if (path === '/api/setup/onboarding') body = { complete: true };
	else if (path === '/api/tidal/mixes') body = { mixes: [{ id: 'smoke-mix', title: 'Smoke Mix', sub_title: 'Daily mix', is_video_mix: false }], source: 'tidal' };
	else if (path === '/api/tidal/page/new-releases') body = { modules: [{ id: 'playlist-shelf', title: 'Playlists', kind: 'PLAYLIST_LIST', items: [{ id: 'smoke-playlist', kind: 'playlist', title: 'Smoke Playlist', creator_name: 'Smoke Owner' }] }] };
	else if (path === '/api/tidal/playlists/smoke-playlist/tracks') body = { tracks: new URL(request.url()).searchParams.get('refresh') === 'true' ? tracks : tracks.slice(0, 1) };
	else if (path === '/api/tidal/mixes/smoke-mix/tracks') {
		if (holdFirstLoad) await firstLoad;
		if (failLoad) return route.fulfill({ status: 502, json: { message: 'TIDAL unavailable' } });
		body = { tracks: emptyMix ? [] : tracks };
	} else if (path === '/api/tidal/status') body = { connected: true, user_id: 'test', country_code: 'AU', auth_flow: 'pkce' };
	else if (path === '/api/playlists' && request.method() === 'POST') {
		if (failSave) return route.fulfill({ status: 502, json: { message: 'TIDAL unavailable' } });
		savedPlaylist = { ...playlist, id: 2, name: request.postDataJSON().name, tidal_uuid: 'saved', track_count: 3 };
		body = { playlist: savedPlaylist };
	} else if (path === '/api/playlists') body = { playlists: savedPlaylist ? [playlist, savedPlaylist] : [playlist] };
	else if (path === '/api/tidal/tracks/import') body = { local_id: request.postDataJSON().tidal_id, tidal_id: request.postDataJSON().tidal_id };
	else if (path === '/api/library/batch/add-to-playlist') body = { added: 1, playlist_id: 1 };
	else if (path === '/api/playlists/1/refresh') { refreshed = true; body = { tracks: 3 }; }
	else if (path === '/api/playlists/1/tracks' || path === '/api/playlists/2/tracks') body = { tracks: (refreshed || path.includes('/2/') ? tracks : tracks.slice(0, 2)).map((track) => ({ ...track, id: track.tidal_id, artist_id: 17, album_id: 19, source: 'tidal' })) };
	else if (path === '/api/playback/state') body = { state: playbackState, queue: [], queue_revision: 0 };
	else if (path.includes('/status')) body = { connected: false };
	else if (path.includes('/queue')) body = { queue: [], queue_revision: 1, state: playbackState };
	else if (path.includes('recommendations')) body = { shelves: [] };
	await route.fulfill({ json: body });
});

try {
	const origin = process.env.NOOR_SMOKE_FRONTEND || 'http://127.0.0.1:17609';
	await page.goto(origin);
	const card = page.getByRole('button', { name: 'Open mix Smoke Mix', exact: true });
	await card.click();
	const popup = page.getByRole('dialog', { name: 'Smoke Mix' });
	await popup.getByText('Loading tracks…', { exact: true }).waitFor();
	assert(await popup.getByRole('button', { name: '▶ Play', exact: true }).isDisabled());
	holdFirstLoad = false;
	releaseFirstLoad();
	await popup.getByText('Smoke Song 1', { exact: true }).waitFor();
	assert.equal(writes.filter((write) => write.path.includes('/queue')).length, 0, 'Opening a mix must not play it');
	assert.equal(await popup.locator('ol > li').count(), 3, 'Repeated tracks must remain visible');
	failLoad = true;
	await popup.getByRole('button', { name: 'Refresh from TIDAL', exact: true }).click();
	await popup.getByRole('alert').waitFor();
	assert(await popup.getByText('Smoke Song 1', { exact: true }).isVisible(), 'Failed refresh must keep loaded tracks visible');
	failLoad = false;
	await popup.getByRole('button', { name: 'Retry', exact: true }).click();
	await popup.getByRole('alert').waitFor({ state: 'detached' });
	await Promise.all([
		page.waitForResponse((response) => new URL(response.url()).pathname === '/api/playback/queue' && response.request().method() === 'POST'),
		popup.getByRole('button', { name: '▶ Play', exact: true }).click(),
	]);
	assert(writes.some((write) => write.path.includes('/queue')), 'Play must reach the queue API');
	await Promise.all([
		page.waitForResponse((response) => new URL(response.url()).pathname === '/api/playback/queue' && response.request().method() === 'POST'),
		popup.getByRole('button', { name: 'Shuffle', exact: true }).click(),
	]);
	assert(writes.filter((write) => write.path.includes('/queue')).length >= 2, 'Shuffle must reach the queue API');
	const secondRow = popup.locator('ol > li').nth(1);
	for (const gesture of [
		() => secondRow.locator('.title').click(),
		() => secondRow.getByRole('button', { name: 'Play Smoke Song 2', exact: true }).click(),
		() => secondRow.locator('.title').dblclick(),
	]) {
		await Promise.all([
			page.waitForResponse((response) => new URL(response.url()).pathname === '/api/playback/queue' && response.request().method() === 'POST'),
			gesture(),
		]);
		await page.waitForTimeout(100);
		assert.deepEqual(writes.filter((write) => write.path === '/api/playback/queue').at(-1).body.items.map((item) => item.tidal_id), [43, 42], 'Every row play gesture must preserve the remaining collection');
	}
	await popup.getByText('Smoke Song 1', { exact: true }).click({ button: 'right' });
	await page.getByRole('menuitem', { name: 'Add to playlist', exact: true }).click();
	const picker = page.getByRole('dialog', { name: 'Add to playlist' });
	await picker.getByRole('button', { name: 'Existing playlist TIDAL' }).click();
	await picker.waitFor({ state: 'detached' });
	assert(writes.some((write) => write.path === '/api/library/batch/add-to-playlist' && write.body.playlist_id === 1));
	failSave = true;
	await popup.getByRole('button', { name: 'Save playlist', exact: true }).click();
	await popup.getByRole('alert').waitFor();
	assert.equal(await popup.locator('ol > li').count(), 3, 'A failed save must keep the track list');
	failSave = false;
	await popup.getByLabel('Playlist name').fill('Saved Smoke Mix');
	await popup.getByRole('button', { name: 'Save playlist', exact: true }).click();
	await popup.getByRole('link', { name: 'Open saved playlist' }).waitFor();
	const save = writes.filter((write) => write.path === '/api/playlists').at(-1);
	assert.deepEqual(save.body, { name: 'Saved Smoke Mix', track_ids: [42, 43, 42], tidal_track_ids: [42, 43, 42], sync_to_tidal: true });
	await popup.getByRole('link', { name: 'Open saved playlist' }).click();
	await page.getByRole('heading', { name: 'Saved Smoke Mix', exact: true }).waitFor();
	assert.equal(await page.locator('.detail-track').count(), 3, 'Saved snapshot must render repeated local track ids');
	const repeatedRow = page.locator('.detail-track').nth(2).locator('[tabindex="0"]').first();
	await repeatedRow.focus();
	const moveResponse = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/playlists/2/tracks/move', { timeout: 5000 });
	await repeatedRow.press('Alt+ArrowUp');
	await moveResponse;
	assert.deepEqual(writes.find((write) => write.path === '/api/playlists/2/tracks/move').body, { from: 2, to: 1 });
	await page.goto(origin);
	failLoad = true;
	await card.click();
	await popup.getByRole('alert').waitFor();
	failLoad = false;
	await popup.getByRole('button', { name: 'Retry' }).click();
	await popup.getByText('Smoke Song 1', { exact: true }).waitFor();
	await page.setViewportSize({ width: 390, height: 844 });
	assert(await popup.evaluate((node) => node.getBoundingClientRect().width <= window.innerWidth));
	await popup.getByRole('button', { name: 'Close track list' }).click();
	await page.setViewportSize({ width: 1440, height: 1000 });
	emptyMix = true;
	await card.click();
	await popup.getByText('No tracks in this mix.', { exact: true }).waitFor();
	assert(await popup.getByRole('button', { name: '▶ Play', exact: true }).isDisabled());
	assert.equal(await popup.getByRole('button', { name: 'Save playlist', exact: true }).count(), 0);
	await popup.getByRole('button', { name: 'Close track list' }).click();
	emptyMix = false;
	await page.goto(`${origin}/playlists/1`);
	await page.getByRole('button', { name: 'Refresh from TIDAL', exact: true }).click();
	await page.getByText('Smoke Song 3', { exact: true }).waitFor();
	assert(refreshed);
	await page.goto(origin);
	await page.getByRole('button', { name: 'Open playlist Smoke Playlist', exact: true }).click();
	const playlistPopup = page.getByRole('dialog', { name: 'Smoke Playlist' });
	await playlistPopup.getByText('Smoke Song 3', { exact: true }).waitFor();
	assert.equal(await playlistPopup.locator('ol > li').count(), 3, 'Opening an editorial playlist must bypass stale cached tracks');
	assert.deepEqual(errors, []);
	console.log('Playlist surface passed: Home popup, repeated tracks, right-click add, linked save, failure/retry, mobile width, and refresh.');
} catch (error) {
	console.error({ url: page.url(), errors, reads, body: (await page.locator('body').innerText()).slice(0, 2000) });
	throw error;
} finally { await browser.close(); }
