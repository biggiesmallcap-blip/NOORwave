import assert from 'node:assert/strict';
import { chromium } from 'playwright';

// All account, library, and image responses are isolated fixtures.
const browser = await chromium.launch({ headless: true,
 ...(process.env.NOOR_SMOKE_BROWSER ? { executablePath: process.env.NOOR_SMOKE_BROWSER } : {}) });
const page = await browser.newPage();
const errors = [];
const portraitRequests = [];
const artwork = 'https://resources.tidal.com/images/hero-cover/640x640.jpg';
const portrait = 'https://resources.tidal.com/images/hero-stale/640x640.jpg';
const track = { id: 1, tidal_id: 42, title: 'Hero track', artist_id: 1,
 artist_name: 'Simon & Garfunkel', album_id: 1, album_title: 'Hero album',
 artwork_url: artwork, play_count: 3, duration_ms: 120000, source: 'tidal' };
page.on('pageerror', error => errors.push(error.message));
await page.addInitScript(() => localStorage.setItem('noor_api_token', 'test-token'));
await page.routeWebSocket('**/ws?*', () => {});
async function respondArtwork(route) {
 if (route.request().url().includes('/hero-stale/')) {
  portraitRequests.push(route.request().url());
  return route.fulfill({ status: 403, body: 'AccessDenied' });
 }
 return route.fulfill({ headers: { 'cache-control': 'no-store' }, contentType: 'image/svg+xml',
  body: '<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="blue"/></svg>' });
}
await page.route('https://resources.tidal.com/**', respondArtwork);
await page.route('**/artwork/tidal/**', respondArtwork);
await page.route('**/api/**', route => {
 const path = new URL(route.request().url()).pathname;
 if (!path.startsWith('/api/')) return route.continue();
 let body = { tracks: [], artists: [], albums: [], playlists: [], genres: [], modules: [],
  articles: [], news: [], stations: [], mixes: [], devices: [], decades: [], total: 0 };
 if (path === '/api/setup/onboarding') body = { complete: true };
 else if (path === '/api/library/top-artists') body = { artists: [{ id: 1, name: 'Simon & Garfunkel',
  photo_url: portrait, fallback_art_url: artwork, play_count: 3, track_count: 3, album_count: 3 }] };
 else if (path === '/api/tracks') body = { tracks: [track], total: 1 };
 else if (path === '/api/albums') body = { albums: [], total: 0 };
 else if (path === '/api/playback/state') body = { state: { current_track: null, current_queue_item_id: null,
  is_playing: false, volume: 1, shuffle_mode: 'off', repeat_mode: 'off', position_ms: 0,
  automix_enabled: false, crossfade_ms: 0, automix_discover_new: false,
  automix_use_learning: true, automix_allow_external: false }, queue: [], queue_revision: 0 };
 else if (path.endsWith('/status')) body = { connected: false };
 return route.fulfill({ json: body });
});
try {
 const origin = process.env.NOOR_SMOKE_FRONTEND || 'http://127.0.0.1:17601';
 await page.goto(origin);
 await page.locator('a[href="/library"]').first().click();
 const hero = page.getByRole('region', { name: 'Top artists', exact: true });
 await hero.getByRole('button', { name: 'Simon & Garfunkel', exact: true }).waitFor();
 await page.waitForFunction(() => {
  const image = document.querySelector('.library-hero-card .mural-panel-art');
  return image instanceof HTMLImageElement && image.src.includes('/hero-cover/') && image.complete && image.naturalWidth > 0;
 });
 assert.equal(portraitRequests.length, 1, 'A failed portrait must try the available cover before retrying other portrait sizes');
 assert.equal(await hero.locator('img').getAttribute('loading'), 'eager');
 assert.equal(await hero.locator('img').getAttribute('fetchpriority'), 'high');
 assert.deepEqual(errors, []);
 console.log('Library hero artwork passed: eager request, featured priority, and prompt cover fallback after a failed portrait.');
} finally { await browser.close(); }
