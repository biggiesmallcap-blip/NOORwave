// Full-site smoke test against a running NOORwave dev environment. Visits
// every route, clicks representative elements, and surfaces console errors,
// failed network requests, and unexpected layouts.
//
// Setup (one-time):
//   npm install --prefix scripts playwright
//   node scripts/click-test.mjs --setup   (installs Chromium once)
//
// Run (with both servers up: `cargo run -p noor-server` + `pnpm --dir frontend dev`):
//   node scripts/click-test.mjs
//
// Optional flags:
//   --artist 4001     Override the artist id (default: 4001 / Julio Iglesias)
//   --query "..."     Override the search query (default: "julio iglesias")
//   --viewport WxH    Browser viewport size (default: 1400x900). Examples:
//                     --viewport 800x600, --viewport 1920x1080, --viewport 2560x1440
//   --shots-suffix s  Append "-s" to every screenshot filename so a single run
//                     doesn't overwrite a prior set. Useful for capturing
//                     baselines at multiple viewports without losing earlier ones.
//   --headed          Show the browser window (default: headless)
//   --keep-open       Leave the browser open at the end for manual inspection
//
// The script exits 0 if everything looked fine, 1 if any step failed.

import { chromium } from 'playwright'
import { mkdirSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

const args = process.argv.slice(2)
const argVal = (flag, def) => {
	const i = args.indexOf(flag)
	return i >= 0 && args[i + 1] && !args[i + 1].startsWith('--') ? args[i + 1] : def
}
const argFlag = (flag) => args.includes(flag)

if (argFlag('--setup')) {
	const { execSync } = await import('node:child_process')
	execSync('npx playwright install chromium', { stdio: 'inherit' })
	process.exit(0)
}

const ARTIST_ID = Number(argVal('--artist', '4001'))
const QUERY = argVal('--query', 'julio iglesias')
const HEADLESS = !argFlag('--headed')
const KEEP_OPEN = argFlag('--keep-open')
const FRONTEND = process.env.NOOR_FRONTEND ?? 'http://localhost:17601'
const BACKEND  = process.env.NOOR_BACKEND  ?? 'http://localhost:17600'

const VIEWPORT_RAW = argVal('--viewport', '1400x900')
const vpMatch = /^(\d+)x(\d+)$/.exec(VIEWPORT_RAW)
if (!vpMatch) {
	console.error(`✗ --viewport must be WxH (e.g. 1280x800), got "${VIEWPORT_RAW}"`)
	process.exit(2)
}
const VIEWPORT = { width: Number(vpMatch[1]), height: Number(vpMatch[2]) }
const SHOTS_SUFFIX = argVal('--shots-suffix', '')

const __dirname = dirname(fileURLToPath(import.meta.url))
const SHOTS = join(__dirname, 'click-test-screenshots')
mkdirSync(SHOTS, { recursive: true })

const shotPath = (name) => {
	if (!SHOTS_SUFFIX) return join(SHOTS, name)
	const dot = name.lastIndexOf('.')
	return join(SHOTS, `${name.slice(0, dot)}-${SHOTS_SUFFIX}${name.slice(dot)}`)
}

const issues = []
const log  = (kind, msg) => console.log(`  ${kind} ${msg}`)
const fail = (msg)       => { issues.push(msg); console.log(`  ✗ ${msg}`) }
const pass = (msg)       => console.log(`  ✓ ${msg}`)
const warn = (msg)       => console.log(`  ~ ${msg}`)

// ── Auth token pre-seed ───────────────────────────────────────────────────────
// Fetched from the loopback-only /api/setup/token endpoint before browser
// launch so localStorage is populated before the first page fires API calls.
let preToken = null
try {
	const tr = await fetch(`${BACKEND}/api/setup/token`)
	if (tr.ok) preToken = (await tr.json()).token
} catch {}

const browser = await chromium.launch({ headless: HEADLESS })
const ctx = await browser.newContext({ viewport: VIEWPORT })
console.log(`viewport: ${VIEWPORT.width}x${VIEWPORT.height}${SHOTS_SUFFIX ? ` (suffix: -${SHOTS_SUFFIX})` : ''}`)
if (preToken) {
	await ctx.addInitScript((tok) => localStorage.setItem('noor_api_token', tok), preToken)
}
const page = await ctx.newPage()

// ── Error filters ─────────────────────────────────────────────────────────────
// Console errors and request failures that are expected in a headless dev run
// where TIDAL/Last.fm/Spotify tokens may not be active.
const BENIGN_CONSOLE = [
	/401.*Unauthorized/i,
	/TrendingShelf/i,
	/trending.*fetch failed/i,
	/curated chart/i,
	/charts\?source=lastfm/i,
	// Home page — Last.fm feeds and external news APIs
	/Failed to load releases/i,
	/Failed to load articles/i,
	/Failed to load news/i,
	/getHomeReleases/i,
	/getHomePicks/i,
	/503/i,
	// Videos — TIDAL token optional in dev
	/video.*stream/i,
	/getTidalVideoStream/i,
	// Discoverspace — no current track seed
	/no.*seed/i,
	/seed.*unavailable/i,
	// Genres — WebGL on some CI setups
	/WebGL/i,
	/INVALID_OPERATION/i,
	// MusicBrainz periodic background fetch
	/musicbrainz/i,
	/MusicBrainz/i,
	// RSS / news feed
	/Failed to fetch.*rss/i,
	/news.*failed/i,
	// Browser-generated "Failed to load resource: 404" — captured with URL
	// by the response handler; the console version has no URL and is redundant.
	/Failed to load resource.*404/i,
	/Failed to load resource.*Not Found/i,
]
const isBenignConsole = (text) => BENIGN_CONSOLE.some((re) => re.test(text))

const BENIGN_URLS = [
	'__data.json', 'hot-update',
	'charts?source=lastfm',
	'resources.tidal.com',
	'spotifycdn.com',
	'i.scdn.co',
	'api.last.fm', 'last.fm',
	'musicbrainz.org',
	'openweathermap.org',
	// Home page feeds — need Last.fm / RSS config that's absent in headless dev.
	'api/home/releases', 'api/home/news', 'api/home/articles', 'api/home/picks',
	// Single-track TIDAL artwork lookups fired by LazyTidalArt on scroll;
	// aborted whenever the page navigates before they complete.
	'/api/tidal/search?q=',
]
const isBenignUrl = (url) => BENIGN_URLS.some((s) => url.includes(s))

page.on('console', (m) => {
	if (m.type() === 'error' && !isBenignConsole(m.text()))
		fail(`console.error: ${m.text()}`)
})
page.on('pageerror', (e) => fail(`pageerror: ${e.message}`))
page.on('requestfailed', (req) => {
	const url = req.url()
	const err = req.failure()?.errorText ?? ''
	if (isBenignUrl(url)) return
	// ERR_ABORTED = the browser cancelled an in-flight request during navigation.
	// This is never a bug — SvelteKit triggers these on every page transition.
	if (err.includes('ERR_ABORTED')) return
	fail(`requestfailed: ${req.method()} ${url} — ${err}`)
})
page.on('response', (resp) => {
	const url = resp.url()
	const status = resp.status()
	if (!url.includes('/api/')) return
	// 404s from /api/ are real issues (wrong route, unregistered handler) — log
	// them with the URL so they're easy to diagnose.
	if (status === 404) {
		// Some endpoints legitimately 404 when content is absent (e.g. artist
		// with no Spotify data). Only flag routes that should always exist.
		const alwaysPresent = ['/api/artists', '/api/albums', '/api/tracks', '/api/playlists', '/api/search']
		if (alwaysPresent.some((r) => url.includes(r))) fail(`404: ${url}`)
		else warn(`404 (optional): ${url}`)
		return
	}
	if (status >= 500) {
		// Last.fm / external-feed 503s are infrastructure noise.
		if (url.includes('home/releases') || url.includes('home/picks') ||
		    url.includes('rss') || url.includes('news')) return
		fail(`5xx: ${status} ${url}`)
	}
})

// ── Step runner ───────────────────────────────────────────────────────────────
const step = async (name, fn) => {
	console.log(`\n▶ ${name}`)
	try { await fn() }
	catch (e) { fail(`${name} threw: ${e.message ?? e}`) }
}

// ══════════════════════════════════════════════════════════════════════════════
// SECTION A — Home & global shell
// ══════════════════════════════════════════════════════════════════════════════

await step('Open / (home page)', async () => {
	await page.goto(`${FRONTEND}/`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2000)
	await page.screenshot({ path: shotPath('01-home.png'), fullPage: true })
	const shell = await page.locator('.home-page').count()
	if (shell === 0) { fail('home-page shell not rendered'); return }
	pass('home-page shell rendered')
	const mixes   = await page.locator('.mix-card').count()
	const picks   = await page.locator('.genre-pill').count()
	const releases = await page.locator('[class*="release-card"], [class*="trending-card"]').count()
	log('·', `mix cards: ${mixes}, genre picks: ${picks}, release cards: ${releases}`)
	if (mixes + picks + releases === 0)
		warn('no content sections (expected if TIDAL+Last.fm unconfigured)')
	else
		pass(`home has content: ${mixes} mix cards, ${picks} genre picks, ${releases} release items`)
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION B — Library
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /library — tracks tab', async () => {
	await page.goto(`${FRONTEND}/library`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2500)
	await page.screenshot({ path: shotPath('02-library.png') })
	const rows = await page.locator('.home-track-row, .track-row').count()
	if (rows === 0) fail('no track rows in library (library may be empty)')
	else pass(`${rows} track rows visible`)
})

await step('Library — Albums tab', async () => {
	const albumsTab = page.locator('button, [role="tab"]').filter({ hasText: /^Albums$/i }).first()
	if (!(await albumsTab.count())) { warn('Albums tab not found — skipping'); return }
	await albumsTab.click()
	await page.waitForTimeout(1000)
	const cards = await page.locator('[class*="album-card"], [class*="album-tile"], .album-item').count()
	log('·', `album cards: ${cards}`)
	if (cards === 0) warn('no album cards (may be a filter state)')
	else pass(`${cards} album cards`)
})

await step('Library — Artists tab', async () => {
	const artistsTab = page.locator('button, [role="tab"]').filter({ hasText: /^Artists$/i }).first()
	if (!(await artistsTab.count())) { warn('Artists tab not found — skipping'); return }
	await artistsTab.click()
	await page.waitForTimeout(1000)
	const cards = await page.locator('[class*="artist-card"], [class*="artist-circle"], .artist-item').count()
	log('·', `artist cards: ${cards}`)
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION C — Search
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /search (empty)', async () => {
	await page.goto(`${FRONTEND}/search`, { waitUntil: 'domcontentloaded' })
	await page.screenshot({ path: shotPath('03-search-empty.png') })
	pass('navigated')
})

await step(`Search "${QUERY}" — Tracks tab not empty`, async () => {
	const input = page.locator('input').first()
	await input.fill(QUERY)
	await page.waitForTimeout(2000)
	await page.screenshot({ path: shotPath('04-search-results.png') })
	const tracks = await page.locator('[class*="track-row"], [class*="search-track-row"]').count()
	if (tracks === 0) fail(`no track rows for "${QUERY}"`)
	else pass(`${tracks} track rows`)
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION D — Artist page (library + TIDAL)
// ══════════════════════════════════════════════════════════════════════════════

await step(`Open /artists/${ARTIST_ID}`, async () => {
	await page.goto(`${FRONTEND}/artists/${ARTIST_ID}`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2500)
	await page.screenshot({ path: shotPath('05-artist-hero.png'), fullPage: true })
	const heroTitle = await page.locator('h1.hero-title').textContent().catch(() => null)
	if (!heroTitle?.trim()) fail('hero title empty')
	else pass(`hero title: "${heroTitle.trim()}"`)
	const portrait = await page.locator('.hero-portrait, .hero-portrait-glass, .hero-portrait-letter').count()
	if (portrait === 0) fail('no hero portrait/fallback rendered')
	else pass('hero portrait present')
})

await step('Top tracks — library + TIDAL merged', async () => {
	const total     = await page.locator('.popular-list > *').count()
	const tidalOnly = await page.locator('.tidal-popular-row').count()
	log('·', `library rows: ${total - tidalOnly}, TIDAL-only rows: ${tidalOnly}`)
	if (total === 0) fail('top tracks list empty')
	else pass(`${total} top-track rows`)
})

await step('Discography rails — presence + count badge', async () => {
	const rails = await page.locator('.media-rail').count()
	if (rails === 0) fail('no MediaRail instances rendered')
	else pass(`${rails} rails rendered`)
	const badge = await page.locator('.shelf-count').first().textContent().catch(() => '?')
	log('·', `first rail count badge: ${badge}`)
})

await step('Bio block (rendered or cleanly absent)', async () => {
	const bio = await page.locator('.hero-bio').count()
	log('·', bio > 0 ? 'bio rendered' : 'no bio (acceptable if artist has none)')
})

await step('Similar artists rail', async () => {
	const cards = await page.locator('.similar-card').count()
	log('·', cards > 0 ? `${cards} similar-artist cards` : 'no similar artists (acceptable)')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION E — Album page (via discography click)
// ══════════════════════════════════════════════════════════════════════════════

let albumHref = null
await step('Navigate to first discography album', async () => {
	const first = page.locator('.media-rail .grid-card').first()
	if (!(await first.count())) { fail('no album cards to click'); return }
	albumHref = await first.getAttribute('href')
	log('·', `target: ${albumHref}`)
	if (albumHref) {
		await page.goto(`${FRONTEND}${albumHref}`, { waitUntil: 'domcontentloaded' })
	} else { fail('grid-card has no href'); return }
	await page.waitForTimeout(1500)
	const url = page.url()
	if (!url.includes('/albums/') && !url.includes('/tidal/albums/'))
		fail(`unexpected URL: ${url}`)
	else pass(`landed on ${url}`)
	await page.screenshot({ path: shotPath('06-album-page.png'), fullPage: true })
})

await step('Album page — track list (library + TIDAL rows)', async () => {
	const lib = await page.locator('.track-list > [class*="track-row"]').count()
	const tid = await page.locator('.tidal-album-row').count()
	const tidal = await page.locator('.track-list-item').count()
	log('·', `library rows: ${lib}, TIDAL-only rows: ${tid}, other rows: ${tidal}`)
	if (lib + tid + tidal === 0) fail('album track list empty')
	else pass(`${lib + tid + tidal} total track rows`)
})

await step('Album page — "More by artist" rail', async () => {
	await page.waitForTimeout(1000)
	const rail = await page.locator('.more-section .media-rail').count()
	log('·', rail > 0 ? '"More by artist" rail present' : '"More by artist" rail absent (single-album artist)')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION F — TIDAL artist page (via similar-artist click)
// ══════════════════════════════════════════════════════════════════════════════

await step('Back to artist — click first similar artist', async () => {
	await page.goto(`${FRONTEND}/artists/${ARTIST_ID}`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2000)
	const similar = page.locator('.similar-card').first()
	if (!(await similar.count())) { warn('no similar artists rail — skipping'); return }
	const href = await similar.getAttribute('href')
	log('·', `similar → ${href}`)
	await similar.click()
	await page.waitForLoadState('domcontentloaded')
	await page.waitForTimeout(1500)
	const url = page.url()
	if (!url.includes('/artists/') && !url.includes('/tidal/artists/'))
		fail(`unexpected URL after similar click: ${url}`)
	else pass(`landed on ${url}`)
	await page.screenshot({ path: shotPath('07-tidal-artist.png'), fullPage: true })
	// Verify TIDAL artist page has a name heading
	const h1 = await page.locator('h1').first().textContent().catch(() => null)
	if (!h1?.trim()) warn('TIDAL artist page: no h1 found')
	else pass(`TIDAL artist name: "${h1.trim()}"`)
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION G — Playlists
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /playlists', async () => {
	await page.goto(`${FRONTEND}/playlists`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(1500)
	await page.screenshot({ path: shotPath('08-playlists.png') })
	const shell  = await page.locator('.playlists-page').count()
	const newBtn = await page.locator('button').filter({ hasText: /new smart playlist/i }).count()
	if (shell === 0) { fail('playlists page shell not rendered'); return }
	pass('playlists shell rendered')
	if (newBtn === 0) fail('"New smart playlist" button missing')
	else pass('"New smart playlist" button present')
	const cards = await page.locator('.playlist-card').count()
	log('·', `${cards} playlist cards`)
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION H — Analytics
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /analytics', async () => {
	await page.goto(`${FRONTEND}/analytics`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(3000)
	await page.screenshot({ path: shotPath('09-analytics.png'), fullPage: true })
	const tree = await page.locator('.analytics-tree').count()
	if (tree === 0) { fail('analytics-tree not rendered'); return }
	pass('analytics-tree rendered')
	const glass = await page.locator('.section.glass').count()
	log('·', `${glass} glass sections`)
})

await step('Analytics — switch time range to 7d', async () => {
	const pill7d = page.locator('button, [role="tab"]').filter({ hasText: /^7d$/i }).first()
	if (!(await pill7d.count())) { warn('7d pill not found — skipping'); return }
	await pill7d.click()
	await page.waitForTimeout(2000)
	pass('7d range switched without crash')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION I — Settings
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /settings', async () => {
	await page.goto(`${FRONTEND}/settings`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(1500)
	await page.screenshot({ path: shotPath('10-settings.png') })
	const shell = await page.locator('.settings-page').count()
	if (shell === 0) { fail('settings page not rendered'); return }
	pass('settings page rendered')
	const rail = await page.locator('.settings-rail-btn').count()
	log('·', `${rail} settings rail categories`)
	if (rail === 0) fail('no settings rail buttons')
	else pass(`${rail} rail categories`)
})

await step('Settings — click second category', async () => {
	const btns = page.locator('.settings-rail-btn')
	const count = await btns.count()
	if (count < 2) { warn('fewer than 2 setting categories — skipping'); return }
	await btns.nth(1).click()
	await page.waitForTimeout(500)
	pass('second category clicked without crash')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION J — Genres
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /genres (genre galaxy)', async () => {
	await page.goto(`${FRONTEND}/genres`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(3000)
	await page.screenshot({ path: shotPath('11-genres.png') })
	// Galaxy renders as a WebGL canvas; check the page shell at minimum
	const hasCanvas = await page.locator('canvas').count()
	const hasEmpty  = await page.locator('[class*="empty"]').count()
	const hasError  = await page.locator('[class*="error"]').count()
	log('·', `canvas elements: ${hasCanvas}, empty states: ${hasEmpty}, error states: ${hasError}`)
	if (hasError > 0) {
		const errText = await page.locator('[class*="error"]').first().textContent().catch(() => '')
		fail(`genres page error: ${errText}`)
	} else if (hasCanvas > 0) {
		pass(`galaxy canvas present (${hasCanvas})`)
	} else {
		warn('no canvas rendered (WebGL may be disabled in headless or library empty)')
	}
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION K — Automix
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /automix', async () => {
	await page.goto(`${FRONTEND}/automix`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(1500)
	await page.screenshot({ path: shotPath('12-automix.png') })
	const shell = await page.locator('.automix-page').count()
	if (shell === 0) { fail('automix page not rendered'); return }
	pass('automix page rendered')
	const hero = await page.locator('.automix-hero').count()
	log('·', hero > 0 ? 'automix hero section present' : 'automix hero absent')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION L — Duplicates
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /duplicates', async () => {
	await page.goto(`${FRONTEND}/duplicates`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(1500)
	await page.screenshot({ path: shotPath('13-duplicates.png') })
	const scanBtn = await page.locator('button').filter({ hasText: /scan library/i }).count()
	if (scanBtn === 0) fail('"Scan library" button not found')
	else pass('"Scan library" button present')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION M — Videos
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /videos', async () => {
	await page.goto(`${FRONTEND}/videos`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2000)
	await page.screenshot({ path: shotPath('14-videos.png') })
	// The page always has a search input regardless of TIDAL auth.
	const input = await page.locator('input[type="text"], input[type="search"], input').count()
	if (input === 0) fail('no search input on videos page')
	else pass('video search input present')
	const videoCards = await page.locator('[class*="video-card"], .video-item').count()
	log('·', videoCards > 0 ? `${videoCards} video cards` : 'no video cards (TIDAL auth required)')
})

// ══════════════════════════════════════════════════════════════════════════════
// SECTION N — Discoverspace
// ══════════════════════════════════════════════════════════════════════════════

await step('Open /discoverspace', async () => {
	await page.goto(`${FRONTEND}/discoverspace`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2500)
	await page.screenshot({ path: shotPath('15-discoverspace.png') })
	const shell  = await page.locator('.discoverspace-page').count()
	const canvas = await page.locator('canvas').count()
	if (shell === 0) { fail('discoverspace page shell not rendered'); return }
	pass('discoverspace shell rendered')
	log('·', canvas > 0 ? `${canvas} canvas elements` : 'no canvas (may need a track playing)')
})

// ══════════════════════════════════════════════════════════════════════════════
// FINAL REPORT
// ══════════════════════════════════════════════════════════════════════════════

console.log('\n══════════════════════════════════════════════════════════════════')
if (issues.length === 0) {
	console.log('✓ full-site pass — no console / network / structure issues')
} else {
	console.log(`✗ ${issues.length} issue(s):`)
	for (const i of issues) console.log(`  • ${i}`)
}
console.log(`screenshots → ${SHOTS}`)

if (!KEEP_OPEN) {
	await browser.close()
	process.exit(issues.length === 0 ? 0 : 1)
} else {
	console.log('\n--keep-open set: leaving browser up; ctrl-C to exit.')
}
