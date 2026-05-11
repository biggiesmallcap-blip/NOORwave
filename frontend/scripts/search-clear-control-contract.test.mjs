import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');

function read(relativePath) {
	return readFileSync(resolve(root, relativePath), 'utf8');
}

describe('search clear controls', () => {
	test('Library, global Search, and Videos all use native search inputs', () => {
		const pages = [
			'src/routes/library/+page.svelte',
			'src/routes/search/+page.svelte',
			'src/routes/videos/+page.svelte'
		];

		for (const page of pages) {
			const source = read(page);
			expect(source, page).toMatch(/type="search"/);
		}
	});

	test('native search cancel buttons are themed from the active palette accent', () => {
		const css = read('src/app.css');

		expect(css).toContain("input[type='search']::-webkit-search-cancel-button");
		expect(css).toMatch(/-webkit-appearance:\s*none/);
		expect(css).toMatch(/background:\s*var\(--accent\)/);
		expect(css).toMatch(/-webkit-mask:/);
		expect(css).toMatch(/mask:/);
	});

	test('top search pills share the same route-level geometry and palette styling', () => {
		const pages = [
			{
				path: 'src/routes/library/+page.svelte',
				inputSelector: '.library-search-input',
				headerSelector: '.library-search-shell'
			},
			{
				path: 'src/routes/search/+page.svelte',
				inputSelector: '.search-input',
				headerSelector: '.search-header'
			},
			{
				path: 'src/routes/videos/+page.svelte',
				inputSelector: '.search-input',
				headerSelector: '.search-header'
			}
		];

		for (const page of pages) {
			const source = read(page.path);
			const inputBlock = source.match(new RegExp(`${page.inputSelector.replace('.', '\\.')}\\s*\\{([\\s\\S]*?)\\n\\s*\\}`))?.[1] ?? '';
			const headerBlock = source.match(new RegExp(`${page.headerSelector.replace('.', '\\.')}\\s*\\{([\\s\\S]*?)\\n\\s*\\}`))?.[1] ?? '';

			expect(inputBlock, page.path).toContain('max-width: 720px');
			expect(inputBlock, page.path).toContain('background: var(--panel-bg)');
			expect(inputBlock, page.path).toContain('border: 1px solid var(--border-subtle)');
			expect(inputBlock, page.path).toContain('padding: 14px 22px');
			expect(headerBlock, page.path).toContain('width: 100%');
			expect(headerBlock, page.path).toContain('margin: 0 auto var(--space-5)');
		}
	});

	test('Search page does not add a unique top offset above the shared search pill', () => {
		const source = read('src/routes/search/+page.svelte');
		const pageBlock = source.match(/\.search-page\s*\{([\s\S]*?)\n\s*\}/)?.[1] ?? '';

		expect(pageBlock).toContain('padding: 0 4px 80px');
	});
});
