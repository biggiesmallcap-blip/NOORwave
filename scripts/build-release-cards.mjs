/**
 * Build milestone social cards (SVG + PNG) from the animated NOOR wordmark.
 *
 * The wordmark, LED pattern and EQ bars are lifted verbatim from
 * frontend/static/social/og-card.svg, so the bars keep their SMIL loop in the
 * SVGs; the PNGs are a single frame rendered by Chrome at 2x.
 *
 * Usage (from repo root):
 *     node scripts/build-release-cards.mjs --version 1.0 --line "The hundredth release"
 */

import { chromium } from 'playwright';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';

const args = process.argv.slice(2);
const flag = (name, fallback) => {
	const i = args.indexOf(`--${name}`);
	return i === -1 ? fallback : args[i + 1];
};

const VERSION = flag('version', '1.0');
const LINE = flag('line', 'The hundredth release');
const SUB = flag('sub', 'TIDAL desktop music player');
const DIR = 'frontend/static/social';
const SLUG = `release-${VERSION.replace(/\./g, '-')}`;

const TEAL = '#2DD4D4';
const INK = '#0A0A0A';
const FONT = "'Inter', 'Helvetica Neue', 'Arial Black', sans-serif";

// Inner wordmark: everything inside the og-card's outer transform group.
const og = await readFile(path.join(DIR, 'og-card.svg'), 'utf8');
const inner = og.slice(og.indexOf('>', og.indexOf('<g transform')) + 1, og.lastIndexOf('</g>'));
// Visible wordmark bounds in its own coordinates (glyph tops to the EQ tips).
const MARK = { x: 228, y: 228, w: 1150, h: 500 };

// Each card: canvas, wordmark box and the version block. Wide cards put the
// wordmark left of the number; the square stacks them.
const CARDS = [
	{ name: 'og', w: 1200, h: 630, mark: [64, 150, 560], num: [700, 360, 250], text: [706, 432], cap: [22, 19] },
	{ name: 'github', w: 1280, h: 640, mark: [72, 155, 600], num: [748, 365, 260], text: [754, 440], cap: [23, 20] },
	{ name: 'x-banner', w: 1500, h: 500, mark: [150, 95, 600], num: [860, 300, 230], text: [866, 368], cap: [26, 22] },
	{ name: 'video', w: 1920, h: 1080, mark: [150, 300, 860], num: [1170, 600, 400], text: [1180, 700], cap: [34, 30] },
	{ name: 'square', w: 1080, h: 1080, mark: [165, 150, 750], num: [540, 760, 300], text: [540, 845], center: true }
];

function card(c) {
	const [mx, my, mw] = c.mark;
	const s = mw / MARK.w;
	const tx = mx - MARK.x * s;
	const ty = my - MARK.y * s;
	const [nx, ny, nsize] = c.num;
	const [lx, ly] = c.text;
	const anchor = c.center ? 'middle' : 'start';
	// Caption sizes: wide cards size them to the column right of the divider.
	const [lineSize, subSize] = c.cap ?? [Math.round(nsize * 0.115), Math.round(nsize * 0.1)];
	const [major, minor] = VERSION.split('.');
	const glowX = c.center ? nx : nx + nsize * 0.8;
	const glowY = ny - nsize * 0.35;
	const divider = c.center
		? ''
		: `<line x1="${nx - 52}" y1="${ny - nsize * 0.78}" x2="${nx - 52}" y2="${ly + 56}" stroke="#FFFFFF" stroke-opacity="0.14" stroke-width="2"/>`;
	return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${c.w} ${c.h}" width="${c.w}" height="${c.h}" role="img" aria-label="NOORwave ${VERSION}">
<title>NOORwave ${VERSION}</title>
<defs>
<radialGradient id="glow" cx="${glowX}" cy="${glowY}" r="${nsize * 1.6}" gradientUnits="userSpaceOnUse">
<stop offset="0" stop-color="${TEAL}" stop-opacity="0.22"/>
<stop offset="1" stop-color="${TEAL}" stop-opacity="0"/>
</radialGradient>
</defs>
<rect width="${c.w}" height="${c.h}" fill="${INK}"/>
<rect width="${c.w}" height="${c.h}" fill="url(#glow)"/>
<g transform="translate(${tx.toFixed(1)} ${ty.toFixed(1)}) scale(${s.toFixed(4)})">${inner}</g>
${divider}
<text x="${nx}" y="${ny}" text-anchor="${anchor}" font-family="${FONT}" font-weight="900" font-size="${nsize}" letter-spacing="${-nsize * 0.04}" fill="#FFFFFF">${major}<tspan fill="${TEAL}">.</tspan>${minor}</text>
<text x="${lx}" y="${ly}" text-anchor="${anchor}" font-family="${FONT}" font-weight="800" font-size="${lineSize}" letter-spacing="${(lineSize * 0.26).toFixed(1)}" fill="${TEAL}">${LINE.toUpperCase()}</text>
<text x="${lx}" y="${ly + Math.round(lineSize * 1.6)}" text-anchor="${anchor}" font-family="${FONT}" font-weight="500" font-size="${subSize}" fill="#FFFFFF" fill-opacity="0.62">NOORwave &#183; ${SUB}</text>
</svg>
`;
}

let browser;
try {
	browser = await chromium.launch({ channel: 'chrome' });
} catch {
	browser = await chromium.launch();
}
for (const c of CARDS) {
	const svg = card(c);
	const base = path.join(DIR, `${SLUG}-${c.name}`);
	await writeFile(`${base}.svg`, svg);
	const page = await browser.newPage({ viewport: { width: c.w, height: c.h }, deviceScaleFactor: 2 });
	await page.setContent(`<body style="margin:0;background:${INK}">${svg}</body>`);
	// One frame into the EQ loop so the bars are mid-pulse, not at rest.
	await page.waitForTimeout(1300);
	await page.locator('svg').first().screenshot({ path: `${base}.png` });
	await page.close();
	console.log(`${c.name.padEnd(9)} -> ${base}.svg / .png`);
}
await browser.close();
