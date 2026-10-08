/**
 * Capture the release tour stills: one 3200x1800 frame per surface and theme.
 *
 * Feeds scripts/build-release-tour.py, which turns them into the release-page
 * MP4 and the README stills. Point it at a noor-server that serves the build
 * being released (NOOR_WWW_DIR), not the installed app, or the shots show the
 * previous release.
 *
 * Usage (from repo root):
 *     node scripts/capture-release-tour.mjs --base http://127.0.0.1:17610 \
 *         --artist 182 --album 1981 --out docs/assets/raw
 */

import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';
import path from 'node:path';

const args = process.argv.slice(2);
const flag = (name, fallback) => {
	const i = args.indexOf(`--${name}`);
	return i === -1 ? fallback : args[i + 1];
};

const BASE = flag('base', 'http://127.0.0.1:17600');
const OUT = flag('out', 'docs/assets/raw');
const ARTIST = flag('artist', '1');
const ALBUM = flag('album', '1');
const ONLY = flag('only', null);

// theme: surface mode; palette: accent palette id from wallpaper/palettes.ts.
const THEMES = {
	dark: { theme: 'dark', palette: 'clay' },
	warm: { theme: 'dark', palette: 'ember' },
	light: { theme: 'light', palette: 'clay' }
};

// settle is per-surface: the galaxy runs a force layout, analytics draws ridges.
// type: optional text typed into the focused search field after load.
const SHOTS = [
	{ name: 'home-dark', path: '/', settle: 3500 },
	{ name: 'home-warm', path: '/', settle: 3500 },
	{ name: 'home-light', path: '/', settle: 3500 },
	{ name: 'search-dark', path: '/search', settle: 2500, type: 'moby' },
	{ name: 'artist-dark', path: `/artists/${ARTIST}`, settle: 3500 },
	{ name: 'album-light', path: `/albums/${ALBUM}`, settle: 3500 },
	{ name: 'library-dark', path: '/library', settle: 3500 },
	{ name: 'library-light', path: '/library', settle: 3500 },
	{ name: 'mix-dark', path: '/mix', settle: 3000 },
	{ name: 'stations-dark', path: '/videos/stations', settle: 4000 },
	{ name: 'videos-warm', path: '/videos/liked', settle: 4000 },
	{ name: 'galaxy-dark', path: '/genres', settle: 9000 },
	{ name: 'space-dark', path: '/discoverspace', settle: 7000 },
	{ name: 'analytics-warm', path: '/analytics', settle: 5000 },
	{ name: 'analytics-light', path: '/analytics', settle: 5000 },
	{ name: 'settings-dark', path: '/settings', settle: 2500 }
];

let browser;
try {
	browser = await chromium.launch({ channel: 'chrome' });
} catch {
	browser = await chromium.launch();
}
await mkdir(OUT, { recursive: true });

// Optional: start the featured artist and pause at once, so Now Playing shows
// their artwork instead of whatever the library last played.
if (args.includes('--set-now-playing')) {
	const context = await browser.newContext({ viewport: { width: 1600, height: 900 } });
	await context.addInitScript(() => localStorage.setItem('noor.onboarding.complete', '1'));
	const page = await context.newPage();
	await page.goto(`${BASE}/artists/${ARTIST}`, { waitUntil: 'networkidle', timeout: 60_000 });
	await page.getByRole('button', { name: /^play/i }).first().click();
	await page.waitForTimeout(1500);
	await page.getByRole('button', { name: /^pause/i }).first().click();
	await page.waitForTimeout(800);
	await context.close();
}

for (const shot of SHOTS) {
	if (ONLY && !ONLY.split(',').includes(shot.name)) continue;
	const look = THEMES[shot.name.split('-').pop()];
	const context = await browser.newContext({
		// 1600x900 CSS px at 2x: the UI fills more of the frame than at 1920 and
		// every frame is 3200x1800, so the 1080p/4K encodes are downscales, never blurry.
		viewport: { width: 1600, height: 900 },
		deviceScaleFactor: 2,
		colorScheme: look.theme === 'light' ? 'light' : 'dark'
	});
	await context.addInitScript((l) => {
		localStorage.setItem('noor-theme', l.theme);
		localStorage.setItem('noor-palette', l.palette);
		localStorage.setItem('noor.onboarding.complete', '1');
	}, look);
	const page = await context.newPage();
	await page.goto(`${BASE}${shot.path}`, { waitUntil: 'networkidle', timeout: 60_000 });
	if (shot.type) {
		await page.keyboard.type(shot.type, { delay: 60 });
	}
	await page.waitForTimeout(shot.settle);
	await page.mouse.move(1599, 450);
	// The sidebar shows the server's version; a capture server running the
	// previous binary would otherwise stamp the old number on the new release.
	const label = flag('version-label', null);
	if (label) {
		await page.evaluate((v) => {
			const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
			for (let n = walker.nextNode(); n; n = walker.nextNode()) {
				if (/^\s*v\d+\.\d+\.\d+\s*$/.test(n.textContent ?? '')) n.textContent = v;
			}
		}, label);
	}
	const file = path.join(OUT, `${shot.name}.png`);
	await page.screenshot({ path: file });
	console.log(`${shot.name.padEnd(16)} -> ${file}`);
	await context.close();
}

await browser.close();
