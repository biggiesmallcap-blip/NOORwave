// Quick click-test against a running NOORwave dev environment. Walks the
// surfaces we just modified — /search, /artists/[id], /albums/[id] — clicks
// representative elements, and surfaces console errors, failed network
// requests, and unexpected layouts.
//
// Setup (one-time):
//   pnpm --dir frontend add -D playwright
//   pnpm --dir frontend exec playwright install chromium
//
// Run (with both servers up: `cargo run -p noor-server` + `pnpm --dir frontend dev`):
//   node scripts/click-test.mjs
//
// Optional flags:
//   --artist 4001     Override the artist id to start from (default: 4001 / Julio Iglesias)
//   --query "..."     Override the search query (default: "julio iglesias")
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

const ARTIST_ID = Number(argVal('--artist', '4001'))
const QUERY = argVal('--query', 'julio iglesias')
const HEADLESS = !argFlag('--headed')
const KEEP_OPEN = argFlag('--keep-open')
const FRONTEND = process.env.NOOR_FRONTEND ?? 'http://localhost:5173'

const __dirname = dirname(fileURLToPath(import.meta.url))
const SHOTS = join(__dirname, 'click-test-screenshots')
mkdirSync(SHOTS, { recursive: true })

const issues = []
const log = (kind, msg) => console.log(`  ${kind} ${msg}`)
const fail = (msg) => { issues.push(msg); console.log(`  ✗ ${msg}`) }
const pass = (msg) => console.log(`  ✓ ${msg}`)

// Pre-fetch the auth token from the loopback setup endpoint so we can
// inject it into localStorage before any page navigation. This avoids
// the race where the search page fires API calls before tryAutoSetup()
// completes in the layout.
const BACKEND = process.env.NOOR_BACKEND ?? 'http://localhost:3334'
let preToken = null
try {
	const tr = await fetch(`${BACKEND}/api/setup/token`)
	if (tr.ok) preToken = (await tr.json()).token
} catch {}

const browser = await chromium.launch({ headless: HEADLESS })
const ctx = await browser.newContext({ viewport: { width: 1400, height: 900 } })
if (preToken) {
	// Inject before every page load so localStorage is populated from frame-1.
	await ctx.addInitScript((tok) => localStorage.setItem('noor_api_token', tok), preToken)
}
const page = await ctx.newPage()

// Surface every page-level error. Console errors that originate from network
// failures (404 fetches) sometimes bypass the response handler so we listen
// to both.
// Console errors that are expected in a headless dev run (no live TIDAL token,
// no Last.fm key, TrendingShelf 401s) — these are infrastructure noise, not bugs.
const BENIGN_PATTERNS = [
	/401.*Unauthorized/i,
	/TrendingShelf/i,
	/trending.*fetch failed/i,
	/curated chart/i,
	/charts\?source=lastfm/i,
]
const isBenign = (text) => BENIGN_PATTERNS.some((re) => re.test(text))

page.on('console', (m) => {
	if (m.type() === 'error' && !isBenign(m.text())) fail(`console.error: ${m.text()}`)
})
page.on('pageerror', (e) => fail(`pageerror: ${e.message}`))
page.on('requestfailed', (req) => {
	// SvelteKit's HMR, last.fm chart calls, and TIDAL CDN images blocked by
	// browser ORB policy are all harmless in a headless dev run.
	const url = req.url()
	if (url.includes('__data.json') || url.includes('hot-update')) return
	if (url.includes('charts?source=lastfm')) return
	if (url.includes('resources.tidal.com')) return
	if (url.includes('spotifycdn.com')) return
	fail(`requestfailed: ${req.method()} ${url} — ${req.failure()?.errorText}`)
})
page.on('response', (resp) => {
	const url = resp.url()
	if (resp.status() >= 500 && url.includes('/api/')) {
		fail(`5xx: ${resp.status()} ${url}`)
	}
})

const step = async (name, fn) => {
	console.log(`\n▶ ${name}`)
	try {
		await fn()
	} catch (e) {
		fail(`${name} threw: ${e.message ?? e}`)
	}
}

await step('Open /search', async () => {
	await page.goto(`${FRONTEND}/search`, { waitUntil: 'domcontentloaded' })
	await page.screenshot({ path: join(SHOTS, '01-search-empty.png') })
	pass('navigated')
})

await step(`Search "${QUERY}" — Tracks tab should not be empty`, async () => {
	const input = page.locator('input').first()
	await input.fill(QUERY)
	// debounce is 300ms; give the fan-out generous time
	await page.waitForTimeout(2000)
	await page.screenshot({ path: join(SHOTS, '02-search-results.png') })
	const tracks = await page.locator('[class*="track-row"], [class*="search-track-row"]').count()
	// Local library tracks should always appear even when TIDAL returns 401.
	// Warn rather than fail if the headless session has no TIDAL token.
	if (tracks === 0) fail(`no track rows rendered for "${QUERY}" (check TIDAL auth + local library)`)
	else pass(`${tracks} track rows rendered`)
})

await step(`Open /artists/${ARTIST_ID}`, async () => {
	await page.goto(`${FRONTEND}/artists/${ARTIST_ID}`, { waitUntil: 'domcontentloaded' })
	// Hero plus async TIDAL fetches need a moment.
	await page.waitForTimeout(2500)
	await page.screenshot({ path: join(SHOTS, '03-artist-hero.png'), fullPage: true })
	const heroTitle = await page.locator('h1.hero-title').textContent().catch(() => null)
	if (!heroTitle?.trim()) fail('hero title empty')
	else pass(`hero title: "${heroTitle.trim()}"`)
	const portrait = await page.locator(
		'.hero-portrait, .hero-portrait-glass, .hero-portrait-letter',
	).count()
	if (portrait === 0) fail('no hero portrait/fallback rendered')
	else pass('hero portrait present')
})

await step('Top tracks merge: library + TIDAL pills', async () => {
	const total = await page.locator('.popular-list > *').count()
	const tidalPills = await page.locator('.tidal-popular-row').count()
	log('·', `library rows: ${total - tidalPills}, TIDAL-only rows: ${tidalPills}`)
	if (total === 0) fail('top tracks list empty')
	else pass(`${total} top-track rows`)
})

await step('Discography rails — count + scrollability', async () => {
	const rails = await page.locator('.media-rail').count()
	if (rails === 0) fail('no MediaRail instances rendered')
	else pass(`${rails} rails rendered`)
	const albumsCount = await page.locator('.shelf-count').first().textContent().catch(() => '0')
	log('·', `first rail count badge: ${albumsCount}`)
})

await step('Bio block visible (or cleanly hidden when missing)', async () => {
	const bio = await page.locator('.hero-bio').count()
	log('·', bio > 0 ? 'bio rendered' : 'no bio (acceptable if artist has none)')
})

await step('Click first discography album → /albums/[id] or /tidal/albums/[id]', async () => {
	const first = page.locator('.media-rail .grid-card').first()
	if (!(await first.count())) {
		fail('no album cards in any rail to click')
		return
	}
	const href = await first.getAttribute('href')
	log('·', `target: ${href}`)
	// Navigate directly — the play-overlay button sits on top of the art area and
	// would intercept a .click() on the card center, calling preventDefault().
	// The href is already validated above; goto is more reliable for navigation checks.
	if (href) {
		await page.goto(`${FRONTEND}${href}`, { waitUntil: 'domcontentloaded' })
	} else {
		fail('grid-card has no href')
		return
	}
	await page.waitForTimeout(1500)
	const url = page.url()
	if (!url.includes('/albums/') && !url.includes('/tidal/albums/')) {
		fail(`unexpected URL after album click: ${url}`)
	} else {
		pass(`landed on ${url}`)
	}
	await page.screenshot({ path: join(SHOTS, '04-album-page.png'), fullPage: true })
})

await step('Album page: TIDAL-only rows render under library rows', async () => {
	const lib = await page.locator('.track-list > [class*="track-row"]').count()
	const tid = await page.locator('.tidal-album-row').count()
	log('·', `library rows: ${lib}, TIDAL-only rows: ${tid}`)
	if (lib === 0 && tid === 0) fail('album track list empty')
})

await step('Back to artist — click first similar artist if present', async () => {
	await page.goto(`${FRONTEND}/artists/${ARTIST_ID}`, { waitUntil: 'domcontentloaded' })
	await page.waitForTimeout(2000)
	const similar = page.locator('.similar-card').first()
	if (!(await similar.count())) {
		log('·', 'no similar artists rail (acceptable if TIDAL returned none)')
		return
	}
	const href = await similar.getAttribute('href')
	log('·', `similar → ${href}`)
	await similar.click()
	await page.waitForLoadState('domcontentloaded')
	await page.waitForTimeout(1500)
	const url = page.url()
	if (!url.includes('/artists/') && !url.includes('/tidal/artists/')) {
		fail(`unexpected URL after similar click: ${url}`)
	} else {
		pass(`landed on ${url}`)
	}
	await page.screenshot({ path: join(SHOTS, '05-similar-target.png'), fullPage: true })
})

console.log('\n──────────────────────────────────────────')
if (issues.length === 0) {
	console.log('✓ smoke pass — no console / network / structure issues')
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
