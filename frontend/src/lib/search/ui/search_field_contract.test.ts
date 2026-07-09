import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const field = readFileSync(join(here, 'SearchField.svelte'), 'utf8');
const row = readFileSync(join(here, 'SearchResultRow.svelte'), 'utf8');

describe('SearchField contract', () => {
	test('reuses the shared parser + facet helpers, never a private copy', () => {
		expect(field).toContain("from '$lib/search/query_parser'");
		expect(field).toContain('parseQuery');
		expect(field).toContain('filtersToChips');
		expect(field).toContain('stripFilter');
		expect(field).toContain("from '$lib/search/facets'");
		expect(field).toContain('matchFacets');
		expect(field).toContain('inlineCompletionFor');
	});

	test('value and inputEl are two-way bindable so shells keep control', () => {
		expect(field).toContain('value = $bindable');
		expect(field).toContain('inputEl = $bindable');
	});

	test('standardized accent focus ring', () => {
		expect(field).toContain('box-shadow: 0 0 0 3px var(--accent-soft)');
		expect(field).toContain('border-color: var(--accent)');
	});

	test('Tab completes the trailing facet prefix and forwards other keys', () => {
		expect(field).toContain("event.key === 'Tab'");
		expect(field).toContain('completeTail(tabCompletion)');
		expect(field).toContain('onkeydown?.(event)');
	});

	test('never trips the motion footgun (no trailing ease after a motion token)', () => {
		expect(field).not.toMatch(/var\(--motion-(fast|base|slow)\)\s+ease/);
	});

	test('no em dash or box-drawing characters', () => {
		expect(field).not.toMatch(/—/);
		expect(field).not.toMatch(/[─-╿]/);
	});
});

describe('SearchResultRow contract', () => {
	test('routes artwork through the shared component, not a raw img', () => {
		expect(row).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(row).toContain('<ArtworkImage');
		expect(row).toContain('size={320}');
		expect(row).not.toMatch(/<img[\s\S]*(artwork_url|src=\{art\})/);
	});

	test('routes the more button and right-click through the app-owned menu subsystem', () => {
		expect(row).toContain("from '$lib/stores/context_menu'");
		expect(row).toContain('openContextMenu(');
		expect(row).toContain('openMenuAtElement(');
		expect(row).toContain('oncontextmenu={handleContext}');
	});

	test('no em dash or box-drawing characters', () => {
		expect(row).not.toMatch(/—/);
		expect(row).not.toMatch(/[─-╿]/);
	});
});
