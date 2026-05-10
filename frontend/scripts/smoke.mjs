import { mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

import { chromium } from 'playwright';

import { buildSmokeRoutes, parseSmokeOptions, redactSensitiveText } from './smoke-options.mjs';

const options = parseSmokeOptions(process.argv.slice(2));
const routes = buildSmokeRoutes(options);
const issues = [];

const scriptDir = dirname(fileURLToPath(import.meta.url));
const shotsDir = join(scriptDir, 'smoke-screenshots');
mkdirSync(shotsDir, { recursive: true });

const shotPath = (route, index) => {
	const safeRoute = route === '/' ? 'home' : route.replace(/^\/+/, '').replace(/[^a-z0-9]+/gi, '-').replace(/^-|-$/g, '');
	const suffix = options.shotsSuffix ? `-${options.shotsSuffix}` : '';
	return join(shotsDir, `${String(index + 1).padStart(2, '0')}-${safeRoute}${suffix}.png`);
};

const info = (message) => console.log(message);
const warn = (message) => console.log(`WARN ${redactSensitiveText(message)}`);
const fail = (message) => {
	const clean = redactSensitiveText(message);
	issues.push(clean);
	console.log(`FAIL ${clean}`);
};
const TRACK_ROW_SELECTOR = '.home-track-row, .track-row, .search-track-row, [class*="track-row"]';

let setupToken = null;
try {
	const response = await fetch(`${options.backend}/api/setup/token`);
	if (response.ok) {
		const body = await response.json();
		setupToken = body?.token ?? null;
	}
} catch {
	warn(`Could not fetch setup token from ${options.backend}. Continuing without auth preseed.`);
}

const browser = await chromium.launch({ headless: options.headless });
const context = await browser.newContext({ viewport: options.viewport });
if (setupToken) {
	await context.addInitScript((token) => localStorage.setItem('noor_api_token', token), setupToken);
}

const page = await context.newPage();

page.on('console', (message) => {
	if (message.type() !== 'error') return;
	const text = message.text();
	if (isBenignConsole(text)) return;
	fail(`console.error: ${text}`);
});

page.on('pageerror', (error) => fail(`pageerror: ${error.message}`));

page.on('requestfailed', (request) => {
	const url = request.url();
	const error = request.failure()?.errorText ?? '';
	if (isBenignUrl(url) || error.includes('ERR_ABORTED')) return;
	fail(`requestfailed: ${request.method()} ${url}: ${error}`);
});

page.on('response', (response) => {
	const url = response.url();
	const status = response.status();
	if (!url.includes('/api/')) return;
	if (status === 404 && isCoreApiUrl(url)) fail(`404: ${url}`);
	if (status >= 500 && !isBenignUrl(url)) fail(`5xx: ${status} ${url}`);
});

info(`NOOR smoke phase: ${options.phase}`);
info(`Viewport: ${options.viewport.width}x${options.viewport.height}`);
info(`Routes: ${routes.join(', ')}`);

for (const [index, route] of routes.entries()) {
	await visitRoute(route, index);
}

await runSearchProbe();
await runContextMenuProbe();

if (!options.keepOpen) {
	await browser.close();
}

if (issues.length > 0) {
	console.log('');
	console.log(`${issues.length} issue(s) found:`);
	for (const issue of issues) console.log(`- ${issue}`);
	process.exit(1);
}

console.log('');
console.log(`Smoke passed. Screenshots: ${shotsDir}`);

async function visitRoute(route, index) {
	const url = `${options.frontend}${route}`;
	info(`VISIT ${route}`);
	try {
		await page.goto(url, { waitUntil: 'domcontentloaded' });
		await waitForAppReady(route);
		await waitForRouteSettled(route);
		await page.waitForTimeout(300);
		await page.screenshot({ path: shotPath(route, index), fullPage: true });
		const titleCount = await page.locator(routeShellSelector(route)).count();
		if (titleCount === 0) fail(`No page shell detected for ${route}`);
	} catch (error) {
		fail(`${route} threw: ${error?.message ?? error}`);
	}
}

async function runSearchProbe() {
	if (!['full', 'links', 'menus', 'performance'].includes(options.phase)) return;
	info(`SEARCH ${options.query}`);
	try {
		await page.goto(`${options.frontend}/search`, { waitUntil: 'domcontentloaded' });
		await waitForAppReady('/search');
		await page.waitForSelector('input', { timeout: 10000 });
		const input = page.locator('input').first();
		await input.fill(options.query);
		await page.waitForTimeout(1200);
		await page.screenshot({ path: shotPath('/search-results', routes.length), fullPage: true });
	} catch (error) {
		fail(`Search probe threw: ${error?.message ?? error}`);
	}
}

async function runContextMenuProbe() {
	if (!['full', 'menus'].includes(options.phase)) return;
	info('CONTEXT MENU probe');
	try {
		await page.goto(`${options.frontend}/library`, { waitUntil: 'domcontentloaded' });
		await waitForAppReady('/library');
		await waitForTrackLikeRow();
		const target = page.locator(TRACK_ROW_SELECTOR).first();
		if ((await target.count()) === 0) {
			warn('No track-like row found for context menu probe');
			return;
		}
		const box = await target.boundingBox();
		const eventInit = box
			? { button: 2, clientX: Math.round(box.x + box.width / 2), clientY: Math.round(box.y + box.height / 2) }
			: { button: 2 };
		await target.dispatchEvent('contextmenu', eventInit);
		await page.waitForTimeout(300);
		const menuCount = await page.locator('.context-menu, [role="menu"]').count();
		if (menuCount === 0) fail('Right-click did not open a context menu on first track-like row');
	} catch (error) {
		fail(`Context menu probe threw: ${error?.message ?? error}`);
	}
}

async function waitForAppReady(route) {
	await page.waitForFunction(
		() => !document.body?.innerText?.includes('CHECKING SETUP'),
		{ timeout: 10000 },
	).catch(() => undefined);
	await page.waitForSelector(routeShellSelector(route), { timeout: 10000 });
}

async function waitForRouteSettled(route) {
	if (route === '/genres') {
		await page.waitForFunction(
			() => !document.body?.innerText?.includes('Loading genres'),
			{ timeout: 15000 },
		).catch(() => undefined);
		return;
	}
	if (route === '/discoverspace') {
		await page.waitForFunction(
			() => !document.body?.innerText?.includes('Mapping sound space'),
			{ timeout: 15000 },
		).catch(() => undefined);
	}
}

function routeShellSelector(route) {
	const routeShells = {
		'/': '.page-shell, .home-page, main, .app-shell, .connect-backdrop',
		'/library': '.page-shell.library, .library-home, .library-search-shell',
		'/search': '.search-page, .search-input',
		'/settings': '.settings-page, .settings-shell, .page-shell',
		'/playlists': '.page-shell, main, .playlist',
		'/videos': '.videos-page, .page-shell, main',
		'/automix': '.automix-page, .page-shell',
		'/duplicates': '.duplicates-page, .page-shell, main',
		'/genres': '.genres-route, .page-shell, main',
		'/analytics': '.analytics-page, .page-shell, main',
		'/discoverspace': '.discoverspace-page, .discover-space-page, main',
	};
	return routeShells[route] ?? 'h1, h2, main, .app-shell, .page-shell, .connect-backdrop';
}

async function waitForTrackLikeRow() {
	await page.waitForSelector(TRACK_ROW_SELECTOR, { timeout: 10000 });
}

function isCoreApiUrl(url) {
	return ['/api/artists', '/api/albums', '/api/tracks', '/api/playlists', '/api/search'].some((path) => url.includes(path));
}

function isBenignConsole(text) {
	return [
		/401.*Unauthorized/i,
		/Failed to load resource.*404/i,
		/Failed to load resource.*Not Found/i,
		/Failed to load tracks/i,
		/Failed to load batch metadata/i,
		/Failed to load releases/i,
		/Failed to load articles/i,
		/Failed to load news/i,
		/Failed to load picks/i,
		/\[trending\] fetch failed/i,
		/WebGL/i,
		/musicbrainz/i,
	].some((pattern) => pattern.test(text));
}

function isBenignUrl(url) {
	return [
		'resources.tidal.com',
		'spotifycdn.com',
		'i.scdn.co',
		'api.last.fm',
		'musicbrainz.org',
		'openweathermap.org',
		'/api/home/releases',
		'/api/home/news',
		'/api/home/articles',
		'/api/home/picks',
		'/api/tidal/search?q=',
	].some((part) => url.includes(part));
}
