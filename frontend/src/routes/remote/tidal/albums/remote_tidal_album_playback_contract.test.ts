import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, test } from 'vitest';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('remote TIDAL album playback contract', () => {
	test('play all reuses the loaded album track list', () => {
		expect(source).toContain("import { playTidalTracksNow, shuffleTidalTracksNow } from '$lib/stores/player';");
		expect(source).toContain('function toPlayable(t: TidalDiscographyTrack): TidalPlayable');
		expect(source).toContain('album_tidal_id: t.album_tidal_id ?? null');
		expect(source).toContain("onPlay={() => playTidalTracksNow(tracks.map(toPlayable), header?.title ?? 'album')}");
		expect(source).toContain("onShuffle={() => shuffleTidalTracksNow(tracks.map(toPlayable), header?.title ?? 'album')}");
		expect(source).not.toContain('playTidalAlbum(tidalAlbumId)');
	});
});
