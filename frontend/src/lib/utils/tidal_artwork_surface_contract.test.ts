import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../..');

function source(path: string): string {
	return readFileSync(join(srcRoot, path), 'utf8');
}

const playerBar = source('lib/shell/PlayerBar.svelte');
const appShell = source('routes/+layout.svelte');
// The artist view (hero artwork, rails) is shared by the library and TIDAL
// routes via ArtistDetail; both route files are thin wrappers.
const sharedArtistView = source('routes/artists/ArtistDetail.svelte');
const tidalArtistRoute = source('routes/tidal/artists/[id]/+page.svelte');
const tidalAlbumRoute = source('routes/tidal/albums/[id]/+page.svelte');
const duplicatesRoute = source('routes/duplicates/+page.svelte');
const genreInterior = source('lib/components/Genre/GenreInterior.svelte');

describe('TIDAL artwork surface contracts', () => {
	test('player and app shell artwork uses allowed sizes with error fallback', () => {
		expect(playerBar).toContain('tidalArtworkFallbackSizes');
		expect(playerBar).toContain('let nowPlayingArtwork = $derived(artworkCandidate(track?.artwork_url, 640));');
		expect(playerBar).toContain('onerror={() => markArtworkFailed(nowPlayingArtwork)}');
		expect(playerBar).not.toContain('src={track.artwork_url}');

		expect(appShell).toContain('tidalArtworkFallbackSizes');
		expect(appShell).toContain('let currentVideoArtwork = $derived(artworkCandidate($videoSession.current?.artwork_url, 320));');
		expect(appShell).toContain('let mobileNowPlayingArtwork = $derived(artworkCandidate($currentTrack?.artwork_url, 640));');
		expect(appShell).toContain('const queueArt = artworkCandidate(item.track.artwork_url, 320)');
		expect(appShell).not.toContain('src={$currentTrack.artwork_url}');
		expect(appShell).not.toContain('src={item.track.artwork_url}');
		expect(appShell).not.toContain('src={video.artwork_url}');
	});

	test('artist routes render TIDAL artwork through allowed sizes with fallbacks', () => {
		expect(sharedArtistView).toContain('tidalArtworkFallbackSizes');
		expect(sharedArtistView).toContain('let heroPortraitSrc = $derived(artworkCandidate(heroPortraitUrl, 640));');
		expect(sharedArtistView).toContain('let heroBackdropSrc = $derived(artworkCandidate(heroBackdropUrl, 1280));');
		expect(sharedArtistView).toContain('onerror={() => markArtworkFailed(heroBackdropSrc)}');
		expect(sharedArtistView).toContain('const similarArt = artworkCandidate(similar.artwork_url, 320)');
		expect(sharedArtistView).not.toContain('src={similar.artwork_url}');
		expect(sharedArtistView).not.toContain('background-image: url({heroBackdropUrl})');

		// The TIDAL artist route reuses the same artwork-safe view; the wrapper
		// only delegates to it, so there is no second hero/grid art surface to
		// keep in sync.
		expect(tidalArtistRoute).toContain("import ArtistDetail from '../../../artists/ArtistDetail.svelte'");
		expect(tidalArtistRoute).toContain("source={{ kind: 'tidal', tidalArtistId }}");
		expect(tidalArtistRoute).not.toContain('src={heroPortrait}');

		expect(tidalAlbumRoute).toContain('tidalArtworkFallbackSizes');
		expect(tidalAlbumRoute).toContain('let heroArtworkSrc = $derived(artworkCandidate(header()?.artwork_url, 640));');
		expect(tidalAlbumRoute).toContain('let heroBackdropSrc = $derived(artworkCandidate(header()?.artwork_url, 1280));');
		expect(tidalAlbumRoute).toContain('onerror={() => markArtworkFailed(heroArtworkSrc)}');
		expect(tidalAlbumRoute).not.toContain('style="background-image: url({h.artwork_url});"');
		expect(tidalAlbumRoute).not.toContain('src={h.artwork_url}');
	});

	test('duplicate and genre track thumbnails use ArtworkImage fallbacks', () => {
		expect(duplicatesRoute).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(duplicatesRoute).toContain('className="member-art"');
		expect(duplicatesRoute).toContain('src={member.track.artwork_url}');
		expect(duplicatesRoute).toContain('size={320}');
		expect(duplicatesRoute).not.toContain('<img class="member-art" src={member.track.artwork_url}');

		expect(genreInterior).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(genreInterior).toContain('className="track-art"');
		expect(genreInterior).toContain('src={track.artwork_url}');
		expect(genreInterior).toContain('size={320}');
		expect(genreInterior).not.toContain('<img class="track-art" src={track.artwork_url}');
	});
});
