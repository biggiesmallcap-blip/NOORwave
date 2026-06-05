import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('TIDAL album playback contract', () => {
	test('play all reuses the loaded album track list', () => {
		expect(source).toContain("import { playTidalTracksNow } from '$lib/stores/player';");
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('async function playLoadedAlbum()');
		expect(source).toContain('tracks.map((track) => tidalDiscographyTrackToPlayable(track))');
		expect(source).toContain("header()?.title ?? 'album'");
		expect(source).toContain('track={tidalDiscographyTrackToPlayable(track)}');
		expect(source).toContain('onclick={() => void playLoadedAlbum()}');
		expect(source).not.toContain('function trackAsPlayable(t: TidalDiscographyTrack)');
		expect(source).not.toContain('playTidalAlbum(tidalAlbumId)');
	});
});
