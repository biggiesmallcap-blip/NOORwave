import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('QoL accessibility contracts', () => {
	it('search page does not steal focus on load', () => {
		const source = readFileSync('src/routes/search/+page.svelte', 'utf8');

		expect(source).not.toMatch(/\sautofocus\b/);
	});

	it('search and list shortcuts stay documented in the shortcut help panel', () => {
		// The inline kbd hint strip under the search fields is gone; the same
		// shortcuts have to remain discoverable from the "?" help panel.
		const help = readFileSync('src/lib/components/ShortcutHelp.svelte', 'utf8');
		const library = readFileSync('src/routes/library/+page.svelte', 'utf8');
		const search = readFileSync('src/routes/search/+page.svelte', 'utf8');

		expect(help).toContain("title: 'Search and lists'");
		expect(help).toContain("keys: ['/'], action: 'Focus the search field'");
		expect(help).toContain("keys: ['Shift', 'Enter'], action: 'Queue the highlighted result'");
		expect(help).toContain("keys: ['Ctrl', 'Enter'], action: 'Play the highlighted result next'");

		for (const source of [library, search]) {
			expect(source).not.toContain('kbd-hint');
		}
	});

	it('album fallback rails do not emit dead hash links', () => {
		const artistPage = readFileSync('src/routes/artists/[id]/+page.svelte', 'utf8');
		const albumPage = readFileSync('src/routes/albums/[id]/+page.svelte', 'utf8');

		for (const source of [artistPage, albumPage]) {
			expect(source).not.toContain("href={album.id != null ? `/albums/${album.id}` : '#'}");
		}
	});
});
