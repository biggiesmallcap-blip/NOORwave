import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const page = readFileSync(resolve(import.meta.dirname, '../src/routes/library/+page.svelte'), 'utf8');
const hero = readFileSync(resolve(import.meta.dirname, '../src/lib/components/LibraryHero.svelte'), 'utf8');
const murals = readFileSync(resolve(import.meta.dirname, '../src/lib/components/library/LibraryMurals.svelte'), 'utf8');
const appCss = readFileSync(resolve(import.meta.dirname, '../src/app.css'), 'utf8');

describe('Library route motion', () => {
	test('Library does not apply page-level translate animation on entry', () => {
		expect(page).toContain('class="page-shell library"');
		expect(page).not.toContain('class="page-shell library animate-in"');
	});

	test('every block on the library landing eases in, not just the murals', () => {
		// The suggestion murals were the only thing that animated, so the hero and
		// the three carousel sections snapped in around them. They now share one
		// cascade, counting up in document order.
		expect(hero).toContain('class="library-hero-card rise-in-shelf"');
		expect(hero).toContain('riseIndex');
		// Landing order: hero, recently added, murals, recent tracks, artists.
		expect(page).toContain('riseIndex={0}');
		expect(page).toContain('class="home-section rise-in-shelf" style="--rise-index: 1"');
		expect(page).toContain('riseIndex={2}');
		expect(page).toContain('class="home-section home-section--tracks rise-in-shelf" style="--rise-index: 3"');
		expect(page).toContain('class="home-section rise-in-shelf" style="--rise-index: 4"');
		expect(page.indexOf('Recently Added')).toBeLessThan(page.indexOf('<LibraryMurals'));
		expect(page.indexOf('<LibraryMurals')).toBeLessThan(page.indexOf('Recent Tracks'));
		expect(page.indexOf('Recent Tracks')).toBeLessThan(page.indexOf('Recently Played Artists'));
		// Panels still stagger inside their grid, but through the shared card
		// variant rather than a private copy of the keyframes.
		expect(murals).toContain('class="murals rise-in-shelf" style={`--rise-index: ${riseIndex}`}');
		expect(murals).toContain('class="mural rise-in-card"');
		expect(murals).not.toContain('@keyframes');
		expect(murals).not.toContain('var(--mural-index');
		// The shared variants are the only definition, and both stand down for
		// reduced motion.
		expect(appCss).toContain('.rise-in-shelf {');
		expect(appCss).toContain('.rise-in-card {');
		expect(appCss).toContain('prefers-reduced-motion: reduce');
	});
});
