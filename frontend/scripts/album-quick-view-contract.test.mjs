import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const popup = read('../src/lib/components/AlbumDetailPopup.svelte');
const albumPopup = read('../src/lib/components/album/AlbumPopup.svelte');

// STYLING.md: the quick view opens from browse surfaces; it always offers the
// full album page so the two never feel like different destinations.
describe('album quick view', () => {
	test('offers Open album page, derived for library albums and passed in otherwise', () => {
		expect(popup).toContain('albumHref?: string | null;');
		expect(popup).toContain('albumHref ?? (isLocal && album.id != null ? `/albums/${album.id}` : null)');
		expect(popup).toContain('>Open album page</button>');
	});

	test('closes before navigating, like the artist link', () => {
		expect(popup).toMatch(/function openAlbumPage\(\) \{[\s\S]*?requestClose\(\);[\s\S]*?goto\(href\)/);
	});

	test('TIDAL-only albums link to their TIDAL album page', () => {
		expect(albumPopup).toContain('`/tidal/albums/${detail.tidalAlbumId}`');
		expect(albumPopup).toContain('{albumHref}');
	});
});
