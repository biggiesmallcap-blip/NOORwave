import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const srcRoot = resolve(here, '../../..');

function source(path: string): string {
	return readFileSync(join(srcRoot, path), 'utf8');
}

const remoteTransport = source('lib/components/remote/RemoteTransport.svelte');
const remoteTrackRow = source('lib/components/remote/RemoteTrackRow.svelte');
const remoteLayout = source('routes/remote/+layout.svelte');
const remoteAlbumTile = source('lib/components/remote/RemoteAlbumTile.svelte');

describe('remote artwork contracts', () => {
	test('remote now-playing art uses shared TIDAL fallback handling', () => {
		expect(remoteTransport).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(remoteTransport).toContain('className="remote-art-image"');
		expect(remoteTransport).toContain('src={track?.artwork_url ?? null}');
		expect(remoteTransport).toContain('size={640}');
		expect(remoteTransport).toContain('fallbackText="NOOR"');
		expect(remoteTransport).toContain(':global(.remote-art-image.fallback)');
		expect(remoteTransport).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(remoteTransport).not.toMatch(/<img[\s\S]*(track\?\.artwork_url|upscaleTidalArtwork|onerror)/);
	});

	test('remote track rows use shared TIDAL fallback handling', () => {
		expect(remoteTrackRow).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(remoteTrackRow).toContain('className="remote-track-thumb"');
		expect(remoteTrackRow).toContain('src={track.artwork_url ?? null}');
		expect(remoteTrackRow).toContain('size={320}');
		expect(remoteTrackRow).toContain('fallbackText="NOOR"');
		expect(remoteTrackRow).toContain(':global(.remote-track-thumb.fallback)');
		expect(remoteTrackRow).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(remoteTrackRow).not.toMatch(/<img[\s\S]*(artwork_url|upscaleTidalArtwork|onerror)/);
	});

	test('remote blurred backdrop uses shared lockscreen-sized artwork fallback handling', () => {
		expect(remoteLayout).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(remoteLayout).toContain('className="remote-layout-backdrop-art"');
		expect(remoteLayout).toContain('src={$currentTrack?.artwork_url ?? null}');
		expect(remoteLayout).toContain('size={1280}');
		expect(remoteLayout).toContain(':global(.remote-layout-backdrop-art.fallback)');
		expect(remoteLayout).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(remoteLayout).not.toMatch(/<img[\s\S]*(backdropArt|\$currentTrack\?\.artwork_url|upscaleTidalArtwork|onerror)/);
	});

	test('remote album tiles use shared TIDAL fallback handling', () => {
		expect(remoteAlbumTile).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(remoteAlbumTile).toContain('<ArtworkImage');
		expect(remoteAlbumTile).toContain('className="remote-album-tile-artwork"');
		expect(remoteAlbumTile).toContain('src={artworkUrl}');
		expect(remoteAlbumTile).toContain('size={320}');
		expect(remoteAlbumTile).toContain('fallbackText="NOOR"');
		expect(remoteAlbumTile).toContain('decorative={true}');
		expect(remoteAlbumTile).toContain(':global(.remote-album-tile-artwork)');
		expect(remoteAlbumTile).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(remoteAlbumTile).not.toMatch(/<img[\s\S]*(artworkUrl|upscaleTidalArtwork|onerror)/);
	});
});
