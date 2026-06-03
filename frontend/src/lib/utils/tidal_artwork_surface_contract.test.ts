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
const localArtistRoute = source('routes/artists/[id]/+page.svelte');
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
		expect(localArtistRoute).toContain('tidalArtworkFallbackSizes');
		expect(localArtistRoute).toContain('let heroPortraitSrc = $derived(artworkCandidate(heroPortraitUrl, 640));');
		expect(localArtistRoute).toContain('let heroBackdropSrc = $derived(artworkCandidate(heroBackdropUrl, 1280));');
		expect(localArtistRoute).toContain('onerror={() => markArtworkFailed(heroBackdropSrc)}');
		expect(localArtistRoute).toContain('const similarArt = artworkCandidate(similar.artwork_url, 320)');
		expect(localArtistRoute).not.toContain('src={similar.artwork_url}');
		expect(localArtistRoute).not.toContain('background-image: url({heroBackdropUrl})');

		expect(tidalArtistRoute).toContain('tidalArtworkFallbackSizes');
		expect(tidalArtistRoute).toContain('const heroPortraitSrc = $derived(artworkCandidate(heroPortrait, 640))');
		expect(tidalArtistRoute).toContain('const heroBackdropSrc = $derived(artworkCandidate(heroBackdrop, 1280))');
		expect(tidalArtistRoute).toContain('class="grid-art-image"');
		expect(tidalArtistRoute).not.toContain("style={album.artwork_url ? `background-image");
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
