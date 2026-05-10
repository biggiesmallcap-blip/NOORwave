import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('QoL accessibility contracts', () => {
	it('search page does not steal focus on load', () => {
		const source = readFileSync('src/routes/search/+page.svelte', 'utf8');

		expect(source).not.toMatch(/\sautofocus\b/);
		expect(source).toContain('<kbd>/</kbd> focus');
	});

	it('album fallback rails do not emit dead hash links', () => {
		const artistPage = readFileSync('src/routes/artists/[id]/+page.svelte', 'utf8');
		const albumPage = readFileSync('src/routes/albums/[id]/+page.svelte', 'utf8');

		for (const source of [artistPage, albumPage]) {
			expect(source).not.toContain("href={album.id != null ? `/albums/${album.id}` : '#'}");
		}
	});
});
