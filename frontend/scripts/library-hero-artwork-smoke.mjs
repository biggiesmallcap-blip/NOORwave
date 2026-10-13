import assert from 'node:assert/strict';
import { chromium } from 'playwright';

// All account, library, and image responses are isolated fixtures.
const browser = await chromium.launch({ headless: true,
 ...(process.env.NOOR_SMOKE_BROWSER ? { executablePath: process.env.NOOR_SMOKE_BROWSER } : {}) });
// No service worker: a built frontend registers one, and route() cannot see
// requests it serves.
const page = await browser.newPage({ serviceWorkers: 'block' });
const errors = [];
const portraitRequests = [];
// The server stores artist pictures at 320 or 480. TIDAL serves them at
// 160/320/480/750 only (640, 1080 and 1280 are 403), and covers at
// 80/160/320/640/750/1080/1280. `hero-stale` is a picture TIDAL no longer has.
const ARTIST_PICTURE_SIZES = new Set(['160', '320', '480', '750']);
const portrait = 'https://resources.tidal.com/images/hero-portrait/480x480.jpg';
const stalePortrait = 'https://resources.tidal.com/images/hero-stale/480x480.jpg';
const artwork = 'https://resources.tidal.com/images/hero-cover/640x640.jpg';
const staleArtwork = 'https://resources.tidal.com/images/hero-cover-two/640x640.jpg';
const track = { id: 1, tidal_id: 42, title: 'Hero track', artist_id: 1,
 artist_name: 'Simon & Garfunkel', album_id: 1, album_title: 'Hero album',
 artwork_url: artwork, play_count: 3, duration_ms: 120000, source: 'tidal' };
page.on('pageerror', error => errors.push(error.message));
await page.addInitScript(() => {
 localStorage.setItem('noor_api_token', 'test-token');
 localStorage.setItem('noor.welcome.v1-0.seen', '1');
});
await page.routeWebSocket('**/ws?*', () => {});
async function respondArtwork(route) {
 const url = route.request().url();
 const size = url.match(/\/(\d+)x\d+\.jpg/)?.[1];
 const isPortrait = url.includes('/hero-portrait/') || url.includes('/hero-stale/');
 if (isPortrait) portraitRequests.push(url);
 if (url.includes('/hero-stale/') || (isPortrait && !ARTIST_PICTURE_SIZES.has(size))) {
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
 else if (path === '/api/library/top-artists') body = { artists: [
  { id: 1, name: 'Simon & Garfunkel', photo_url: portrait, fallback_art_url: artwork,
   play_count: 3, track_count: 3, album_count: 3 },
  { id: 2, name: 'Stale Portrait', photo_url: stalePortrait, fallback_art_url: staleArtwork,
   play_count: 2, track_count: 2, album_count: 1 }] };
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
 const tileImage = (name, part) => page.waitForFunction(([label, needle]) => {
  const image = document.querySelector(`.library-hero-card button[aria-label="Select ${label}"] .mural-panel-art`);
  return image instanceof HTMLImageElement && image.src.includes(needle) && image.complete && image.naturalWidth > 0;
 }, [name, part]);
 await tileImage('Simon & Garfunkel', '/hero-portrait/750x750.jpg');
 await tileImage('Stale Portrait', '/hero-cover-two/');
 const invalidPortraitSizes = portraitRequests.filter(url => !ARTIST_PICTURE_SIZES.has(url.match(/\/(\d+)x\d+\.jpg/)?.[1]));
 assert.deepEqual(invalidPortraitSizes, [], 'Artist pictures must be requested at a size TIDAL serves them at');
 assert.equal(portraitRequests.filter(url => url.includes('/hero-portrait/')).length, 1, 'A valid portrait loads on its first request');
 assert.equal(portraitRequests.filter(url => url.includes('/hero-stale/')).length, 1, 'A failed portrait must try the available cover before retrying other portrait sizes');
 const featured = hero.locator('button[aria-label="Select Simon & Garfunkel"] img');
 assert.equal(await featured.getAttribute('loading'), 'eager');
 assert.equal(await featured.getAttribute('fetchpriority'), 'high');
 assert.deepEqual(errors, []);
 console.log('Library hero artwork passed: portraits at a TIDAL artist-picture size, featured priority, and prompt cover fallback after a missing portrait.');
} finally { await browser.close(); }
