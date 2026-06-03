import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

function source(path: string): string {
	return readFileSync(join(here, path), 'utf8');
}

const albumCarousel = source('AlbumCarousel.svelte');
const albumDetailPopup = source('AlbumDetailPopup.svelte');
const artistCarousel = source('ArtistCarousel.svelte');

describe('album artwork contracts', () => {
	test('album carousel routes artwork through TIDAL fallback sizes', () => {
		expect(albumCarousel).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(albumCarousel).toContain('<ArtworkImage');
		expect(albumCarousel).toContain('className="album-carousel-art"');
		expect(albumCarousel).toContain('src={resolved}');
		expect(albumCarousel).toContain('size={320}');
		expect(albumCarousel).toContain('fallbackText={album.title.slice(0, 2).toUpperCase()}');
		expect(albumCarousel).toContain('decorative={true}');
		expect(albumCarousel).toContain(':global(.album-carousel-art)');
		expect(albumCarousel).not.toContain('tidalArtworkFallbackSizes');
		expect(albumCarousel).not.toContain('upscaleTidalArtwork');
		expect(albumCarousel).not.toContain('failedArtworkUrls');
		expect(albumCarousel).not.toContain('artworkCandidate');
		expect(albumCarousel).not.toContain('markArtworkFailed');
		expect(albumCarousel).not.toContain('<img');
		expect(albumCarousel).not.toContain('style="background-image: url');
	});

	test('artist carousel routes artist photos through ArtworkImage', () => {
		expect(artistCarousel).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(artistCarousel).toContain('function artistImageSources(');
		expect(artistCarousel).toContain('<ArtworkImage');
		expect(artistCarousel).toContain('className="artist-carousel-avatar"');
		expect(artistCarousel).toContain(
			'src={artistImageSources(artist.photo_url, lazyArt[artist.id], artist.fallback_art_url)}'
		);
		expect(artistCarousel).toContain('alt={artist.name}');
		expect(artistCarousel).toContain('size={320}');
		expect(artistCarousel).toContain('fallbackText={initials(artist.name)}');
		expect(artistCarousel).toContain(':global(.artist-carousel-avatar)');
		expect(artistCarousel).not.toContain("import { letterColor } from '$lib/utils/color';");
		expect(artistCarousel).not.toContain('failedImages');
		expect(artistCarousel).not.toContain('<img');
		expect(artistCarousel).not.toContain('onerror');
		expect(artistCarousel).not.toContain('style="background:');
	});

	test('album detail popup routes artwork through TIDAL fallback sizes', () => {
		expect(albumDetailPopup).toContain('tidalArtworkFallbackSizes');
		expect(albumDetailPopup).toContain('let popupArtwork = $derived(artworkCandidate(album.artwork_url, 640));');
		expect(albumDetailPopup).toContain('onerror={() => markArtworkFailed(popupArtwork)}');
		expect(albumDetailPopup).not.toContain('src={album.artwork_url}');
	});
});
