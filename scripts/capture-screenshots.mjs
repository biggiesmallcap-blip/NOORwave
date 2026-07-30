/**
 * Capture README screenshots from a running noor-server.
 *
 * Playwright rather than `chrome --screenshot` because the Genre Galaxy canvas
 * needs real wall-clock time to run its layout simulation: under Chrome's
 * virtual time budget the page reports its data loaded and then screenshots a
 * blank canvas.
 *
 * Loopback only, so the UI fetches the access PIN itself and no token is needed.
 *
 * Usage (from repo root, with noor-server running):
 *     node scripts/capture-screenshots.mjs
 *     node scripts/capture-screenshots.mjs --out docs/assets --base http://127.0.0.1:17600
 *
 * Then round the corners:
 *     python scripts/polish-screenshots.py
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
const OUT = flag('out', 'docs/assets');

// settle is per-surface: the galaxy runs a force layout, analytics draws ridges.
const SHOTS = [
	{ name: 'home', path: '/', settle: 3500 },
	{ name: 'library', path: '/library', settle: 3500 },
	{ name: 'genregalaxy', path: '/genres', settle: 9000 },
	{ name: 'videolikes', path: '/videos/liked', settle: 4000 },
	{ name: 'analytics', path: '/analytics', settle: 5000 }
];

// Reuse the system Chrome rather than making every clone download a bundled
// browser for five screenshots. Falls back to the bundled one if it is present.
let browser;
try {
	browser = await chromium.launch({ channel: 'chrome' });
} catch {
	browser = await chromium.launch();
}
const page = await browser.newPage({
	viewport: { width: 1920, height: 1080 },
	deviceScaleFactor: 1,
	colorScheme: 'dark'
});

await mkdir(OUT, { recursive: true });

for (const shot of SHOTS) {
	await page.goto(`${BASE}${shot.path}`, { waitUntil: 'networkidle', timeout: 60_000 });
	await page.waitForTimeout(shot.settle);
	const file = path.join(OUT, `screenshot-${shot.name}.png`);
	await page.screenshot({ path: file });
	console.log(`${shot.name.padEnd(14)} -> ${file}`);
}

await browser.close();
