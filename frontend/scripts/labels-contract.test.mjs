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
