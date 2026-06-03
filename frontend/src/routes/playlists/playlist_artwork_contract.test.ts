import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const page = readFileSync(join(here, '+page.svelte'), 'utf8');
const cache = readFileSync(join(here, '../../lib/stores/playlist_artwork_cache.ts'), 'utf8');

describe('playlist artwork contracts', () => {
	test('playlist cover mosaics route track artwork through ArtworkImage', () => {
		expect(cache).toContain('pickArtworkUrls(tracks: Array<{ artwork_url: string | null }>)');
		expect(page).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(page).toContain('<ArtworkImage');
		expect(page).toContain('className="playlist-cover-art"');
		expect(page).toContain('src={url}');
		expect(page).toContain('src={mosaic[0]}');
		expect(page).toContain('size={320}');
		expect(page).toContain('fallbackText="PL"');
		expect(page).toContain('decorative={true}');
		expect(page).toContain(':global(.playlist-cover-art)');
		expect(page).not.toContain('<img src={url}');
		expect(page).not.toContain('<img class="cover-solo"');
	});
});
