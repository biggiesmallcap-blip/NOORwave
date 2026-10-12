// Run against a dev server: node scripts/library-loading-smoke.ts [frontend URL]
// Requests are intercepted so this exercises the real route without touching a library.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';

const base = process.argv[2] ?? 'http://127.0.0.1:17601';
const browser = await chromium.launch({ headless: true, channel: process.env.NOOR_BROWSER_CHANNEL });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
const page = await context.newPage();
page.setDefaultTimeout(10000);
const errors: string[] = [];
page.on('pageerror', (error) => errors.push(error.stack ?? error.message));
// Keep the shell connected to an inert socket, just as the API uses fixtures.
await context.routeWebSocket('**/ws?*', () => {});
await context.addInitScript(() => {
 localStorage.setItem('noor_api_token', 'library-loading-test');
 localStorage.setItem('noor.onboarding.complete.library-loading-test', '1');
 localStorage.setItem('noor.welcome.v1-0.seen', '1');
});

const track = {
 id: 1, tidal_id: 1, title: 'Loading test song', artist_id: 1, artist_name: 'Loading test artist',
 album_id: 1, album_title: 'Loading test album', artwork_url: null, duration_ms: 180000,
 date_added: '2026-10-10T00:00:00Z', is_favorite: true, play_count: 5,
};
const album = { id: 1, title: track.album_title, artist_id: 1, artist_name: track.artist_name, artwork_url: null };
let releaseAlbums: () => void = () => {};
const albumsPending = new Promise<void>((resolve) => { releaseAlbums = resolve; });
let albumRequested = false;
let holdLanding = false;
let releaseLanding: () => void = () => {};
const landingPending = new Promise<void>((resolve) => { releaseLanding = resolve; });
let emptyLanding = false;
let failAlbums = false;
let landingRequests = 0;

await context.route((url) => url.pathname.startsWith('/api/'), async (route) => {
 const path = new URL(route.request().url()).pathname;
 let data: unknown = {};
 let status = 200;
 if (path === '/api/albums') {
  albumRequested = true;
  await albumsPending;
  status = failAlbums ? 503 : 200;
  data = { albums: [album], total: 1 };
 } else if (path === '/api/library/top-artists' || path.startsWith('/api/home/')) {
  landingRequests++;
  if (holdLanding) await landingPending;
  data = path === '/api/library/top-artists'
   ? { artists: emptyLanding ? [] : [{ id: 1, name: track.artist_name, photo_url: null, fallback_art_url: null, play_count: 5, track_count: 1, album_count: 1 }] }
   : { tracks: emptyLanding ? [] : [track], albums: emptyLanding ? [] : [album] };
 } else if (path === '/api/tracks') {
  if (holdLanding) await landingPending;
  data = { tracks: emptyLanding ? [] : [track], total: emptyLanding ? 0 : 1 };
 }
 else if (path === '/api/remote/identity') data = { server_id: 'loading-test', name: 'NOORwave', protocol: 1, pairing_available: false };
 else if (path === '/api/setup/onboarding') data = { complete: true };
 else if (path === '/api/status') data = { version: '1.0.5' };
 else if (path === '/api/history') data = { tracks: [], total: 0 };
 else if (path === '/api/library/catalogue/status') data = { tracks: [] };
 else if (path === '/api/playlists') data = { playlists: [] };
 else if (path === '/api/genres') data = { genres: [] };
 else if (path === '/api/albums/decades') data = { decades: [] };
 else if (path.includes('/queue')) data = { queue: [] };
 else if (path === '/api/playback/state') data = { state: { current_track: null, is_playing: false, volume: 0.5, position_ms: 0, shuffle_mode: 'off', repeat_mode: 'off' }, queue: [] };
 await route.fulfill({ status, json: data });
});

try {
 await page.goto(`${base}/library`, { waitUntil: 'domcontentloaded' });
 await page.locator('.library').waitFor();
 // Albums remain unresolved throughout: landing data must mount independently.
 await page.locator('.library-home .library-hero-card').waitFor({ timeout: 3000 });
 assert(albumRequested, 'The test must hold a real browse request in flight');
 await page.getByText('Recently Added', { exact: true }).waitFor();
 await page.getByRole('article', { name: 'Suggested tracks', exact: true }).waitFor();
 assert.equal(await page.getByRole('status', { name: 'Loading library', exact: true }).count(), 0);
 console.log('PASS: cold landing renders while Albums is unresolved');

 // A reload must paint persisted data even when every landing refresh is blocked.
 holdLanding = true;
 await page.evaluate(() => {
  for (const key of Object.keys(localStorage)) {
   if (!key.startsWith('noor.query.')) continue;
   const entry = JSON.parse(localStorage.getItem(key)!);
   entry.lastUpdated = Date.now() - 31 * 60 * 1000;
   localStorage.setItem(key, JSON.stringify(entry));
  }
 });
 await page.reload({ waitUntil: 'domcontentloaded' });
 await page.locator('.library-home .library-hero-card').waitFor({ timeout: 3000 });
 await page.getByText('Recently Added', { exact: true }).waitFor();
 await page.getByRole('article', { name: 'Suggested tracks', exact: true }).waitFor();
 console.log('PASS: persisted landing paints while browse and refresh requests are unresolved');

 // Navigate through the shell and back, not just reload the document.
 await page.locator('a[href="/playlists"]').first().click();
 await page.waitForURL('**/playlists');
 await page.locator('a[href="/library"]').first().click();
 await page.locator('.library-home .library-hero-card').waitFor({ timeout: 3000 });
 await page.getByRole('article', { name: 'Suggested tracks', exact: true }).waitFor();
 console.log('PASS: repeat navigation keeps the landing visible during pending requests');

 // Browse tabs still show loading and do not pretend that pending rows are ready.
 await page.getByRole('tab', { name: /^Albums/ }).click();
 await page.getByRole('status', { name: 'Loading library', exact: true }).waitFor();
 await page.getByRole('tab', { name: 'All', exact: true }).click();
 await page.locator('.library-home .library-hero-card').waitFor();
 failAlbums = true;
 const albumsFailed = page.waitForResponse((response) => new URL(response.url()).pathname === '/api/albums' && response.status() === 503);
 releaseAlbums();
 holdLanding = false;
 releaseLanding();
 await albumsFailed;
 await page.getByRole('article', { name: 'Suggested tracks', exact: true }).waitFor();
 assert.equal(await page.locator('.library-home').count(), 1);
 assert(landingRequests > 0);
 assert.deepEqual(errors, []);
 console.log('PASS: browse loading and failure do not remove the landing');

 // Empty data must settle without a global spinner or runtime exception.
 emptyLanding = true;
 await page.evaluate(() => {
  for (const key of Object.keys(localStorage)) if (key.startsWith('noor.query.')) localStorage.removeItem(key);
 });
 await page.reload({ waitUntil: 'domcontentloaded' });
 await page.locator('.library-home').waitFor();
 await page.waitForFunction(() => !document.querySelector('.home-loading'));
 assert.equal(await page.locator('.library-home .library-hero-card').count(), 0);
 assert.equal(await page.getByRole('status', { name: 'Loading library', exact: true }).count(), 0);
 assert.deepEqual(errors, []);
 console.log('PASS: empty landing settles without an unrelated loading screen');
} catch (error) {
 console.error('Route state:', page.url(), (await page.locator('body').innerText()).slice(0, 1500), errors);
 throw error;
} finally {
 releaseAlbums();
 releaseLanding();
 await browser.close();
}
