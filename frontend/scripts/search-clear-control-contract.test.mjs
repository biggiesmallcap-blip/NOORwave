import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');

function read(relativePath) {
	return readFileSync(resolve(root, relativePath), 'utf8');
}

const SEARCH_FIELD = 'src/lib/search/ui/SearchField.svelte';

describe('search clear controls', () => {
	test('Library, global Search, and Videos all use the shared SearchField', () => {
		const pages = [
			'src/routes/library/+page.svelte',
			'src/routes/search/+page.svelte',
			'src/routes/videos/+page.svelte'
		];

		for (const page of pages) {
			const source = read(page);
			expect(source, page).toContain('<SearchField');
		}

		// SearchField renders a native type="search" input for its page variant,
		// which is what supplies the accent-themed cancel button below.
		const field = read(SEARCH_FIELD);
		expect(field).toContain("variant === 'modal' ? 'text' : 'search'");
	});

	test('native search cancel buttons are themed from the active palette accent', () => {
		const css = read('src/app.css');

		expect(css).toContain("input[type='search']::-webkit-search-cancel-button");
		expect(css).toMatch(/-webkit-appearance:\s*none/);
		expect(css).toMatch(/background:\s*var\(--accent\)/);
		expect(css).toMatch(/-webkit-mask:/);
		expect(css).toMatch(/mask:/);
	});

	test('the shared search field owns the standardized page-input geometry', () => {
		// The 720px pill recipe now lives once, in SearchField, instead of being
		// copy-pasted per route.
		const field = read(SEARCH_FIELD);
		const pageBlock = field.match(/\.sf--page \.sf-shell\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';

		expect(field.match(/\.sf--page\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '').toContain('max-width: 720px');
		expect(pageBlock).toContain('background: var(--panel-bg)');
		expect(pageBlock).toContain('border: 1px solid var(--border-subtle)');
		expect(pageBlock).toContain('padding: 14px 22px');

		// Every top-of-page search adopts it at the page variant.
		for (const path of [
			'src/routes/library/+page.svelte',
			'src/routes/search/+page.svelte',
			'src/routes/videos/+page.svelte'
		]) {
			expect(read(path), path).toContain('variant="page"');
		}
	});

	test('Search page does not add a unique top offset above the shared search pill', () => {
		const source = read('src/routes/search/+page.svelte');
		const pageBlock = source.match(/\.search-page\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';

		expect(pageBlock).toContain('padding: 0 4px 80px');
	});
});
