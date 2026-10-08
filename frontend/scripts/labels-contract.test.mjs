import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');

// STYLING.md "Typography": a section gets a title or a label, never both.
// The audit found eyebrows ("TIDAL", "CHARTS", "CONNECTED PROFILES") stacked
// over titles that already said the same thing, on almost every route.
describe('no eyebrow over a title', () => {
	test('the shared headers have no eyebrow slot', () => {
		for (const file of ['../src/lib/components/ui/SectionHeader.svelte', '../src/lib/components/ui/PageHeader.svelte']) {
			expect(read(file), file).not.toMatch(/eyebrow\??:/);
		}
	});

	test('chart murals do not repeat the section source on every slide', () => {
		expect(read('../src/lib/components/charts/ChartMural.svelte')).not.toContain('kindLabel');
	});

	test('Sound Space is the title, its tagline sits under it', () => {
		const page = read('../src/routes/discoverspace/+page.svelte');
		expect(page).toContain('<h1 class="t-page-title">{PAGE_TITLE}</h1>');
		expect(page).not.toContain('<span class="eyebrow">{PAGE_TITLE}</span>');
	});
});

// STYLING.md "Boundaries": Settings is a form on the ground. No page panel,
// no group panels; groups are a label over hairline rows.
describe('Settings is a form, not a panel', () => {
	const page = read('../src/routes/settings/+page.svelte');
	const css = read('../src/lib/components/settings/settings.css');
	test('the page is not wrapped in glass', () => {
		expect(page).toContain('<div class="page-shell settings-page settings-scope">');
	});
	test('groups and sections drop their box', () => {
		expect(css).toMatch(/\.settings-scope \.section-panel \{ background: none;[^}]*border: 0;[^}]*padding: 0;/);
	});
});

// Audit "Search: answer first": the top result and five songs share the
// first screen; library marks are a ring, not a red dot.
describe('Search answers first', () => {
	const search = read('../src/routes/search/+page.svelte');
	test('five songs sit beside the top result in the All view', () => {
		expect(search).toContain('<div class="answer-split" class:with-songs={answerTracks.length > 0}>');
		expect(search).toContain('visibleTracks.slice(0, ANSWER_SONGS)');
		expect(search).toContain('{@render songRow(track, idx)}');
	});
	test('the keyboard cursor stays in view', () => {
		expect(search).toContain("scrollIntoView({ block: 'nearest' })");
	});
});

describe('Search Enter never leaves the results', () => {
	const search = read('../src/routes/search/+page.svelte');
	test('an artist top result plays in place instead of navigating', () => {
		expect(search).toContain('else playTopArtistInPlace(topResult.entry, mode)');
		const fn = search.slice(search.indexOf('function playTopArtistInPlace'), search.indexOf('function topResultPlay'));
		expect(fn).not.toContain('goto(');
	});
});

describe('Space presses a focused control', () => {
	const layout = read('../src/routes/+layout.svelte');
	test('the play/pause shortcut yields to focused controls outside the transport', () => {
		expect(layout).toContain('if (spaceActivatesFocusedControl(target)) return;');
		expect(layout).toContain("return control.closest('[data-transport]') == null;");
	});
});
