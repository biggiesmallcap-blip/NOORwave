import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('artwork usage contracts', () => {
	test('artist portrait surfaces lazy-resolve missing artist images', () => {
		const source = readFileSync('src/lib/components/ArtistCarousel.svelte', 'utf8');
		const library = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).toContain('lazyTidalArt');
		expect(source).toContain('lazyArt');
		expect(library).toContain('use:lazyTidalArt');
		expect(library).toContain('artistLazyArt');
	});

	test('library artist cards keep photo_url separate from track artwork', () => {
		const source = readFileSync('src/routes/library/+page.svelte', 'utf8');

		expect(source).not.toContain('info.photo_url = track.artwork_url');
		expect(source).not.toContain('photo_url: storeArtist?.photo_url ?? track.artwork_url ?? null');
		expect(source).toContain('fallback_art_url');
		expect(source).toContain('fallback_art_url: track.artwork_url');
	});

	test('tidal artist page separates portrait art from backdrop fallback art', () => {
		const source = readFileSync('src/routes/tidal/artists/[id]/+page.svelte', 'utf8');

		expect(source).toContain('heroPortrait');
		expect(source).toContain('heroBackdrop');
		expect(source).not.toContain('{#if heroArt}');
		expect(source).not.toContain('src={heroArt}');
	});

	test('grouped local album cards fill artwork from later tracks', () => {
		const artistPage = readFileSync('src/routes/artists/[id]/+page.svelte', 'utf8');
		const albumPage = readFileSync('src/routes/albums/[id]/+page.svelte', 'utf8');

		for (const source of [artistPage, albumPage]) {
			expect(source).toContain('existing.artwork_url = t.artwork_url');
		}
	});
});
