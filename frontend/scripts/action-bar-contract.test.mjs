import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const bar = read('../src/lib/components/ui/ActionBar.svelte');
const hero = read('../src/lib/components/ui/DetailHero.svelte');
const album = read('../src/routes/albums/[id]/+page.svelte');

// STYLING.md "ActionBar": the action row of every detail hero. Play is the
// labelled primary; Shuffle and Radio are labelled secondaries; Like and More
// are icons. Below 1100px of content width the labels become icons.
describe('ActionBar', () => {
	test('Play is the labelled primary and toggles to Pause', () => {
		expect(bar).toMatch(/class="ab-btn primary"/);
		expect(bar).toContain("{playing ? 'Pause' : playLabel}");
	});

	test('Shuffle and Radio are labelled secondaries; Like and More are icons', () => {
		expect(bar).toContain('<span class="label">Shuffle</span>');
		expect(bar).toContain('<span class="label">{radioLabel}</span>');
		expect(bar).toContain('aria-pressed={liked}');
		expect(bar).toContain('aria-label={moreLabel}');
		expect(bar).toContain('aria-haspopup="menu"');
	});

	test('labels collapse to icons below 1100px of content width but keep their names', () => {
		expect(hero).toMatch(/\.detail-hero\s*\{[^}]*container: detail-hero \/ inline-size;/);
		expect(bar).toMatch(/@container detail-hero \(max-width: 1100px\)/);
		// Visually hidden, not display: none, so the accessible name stays.
		expect(bar).toMatch(/@container detail-hero \(max-width: 1100px\)\s*\{\s*\.label\s*\{[^}]*clip-path: inset\(50%\);/);
	});

	test('buttons have no hover lift', () => {
		expect(bar).not.toMatch(/:hover[^{]*\{[^}]*transform/);
	});
});

// STYLING.md "Detail header": the hero is not a box.
describe('DetailHero is borderless', () => {
	test('no border, fill or radius on the hero', () => {
		const root = hero.match(/\.detail-hero\s*\{([^}]*)\}/)?.[1] ?? '';
		expect(root).not.toContain('border:');
		expect(root).not.toContain('background:');
		expect(root).not.toContain('border-radius:');
	});

	test('the backdrop fades out at the sides instead of ending on an edge', () => {
		expect(hero).toMatch(/\.backdrop\s*\{[^}]*mask-image:/);
	});
});

describe('the album page uses ActionBar', () => {
	test('the hero actions mount ActionBar; More opens the shared album menu', () => {
		expect(album).toContain('<ActionBar');
		expect(album).toContain('buildAlbumMenu(');
		expect(album).not.toContain('class="play-fab"');
		expect(album).not.toContain('class="ghost-btn"');
	});
});
