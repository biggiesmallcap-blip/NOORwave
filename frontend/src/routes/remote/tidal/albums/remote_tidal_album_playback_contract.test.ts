import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, test } from 'vitest';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('remote TIDAL album playback contract', () => {
	test('clears previous album tracks while loading a new route', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('async function load(id: number)');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('tracks = [];');
		expect(source).toContain('const res = await api.getTidalAlbumTracks(id);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('const id = tidalAlbumId;');
		expect(source).toContain('void load(id);');
	});

	test('play all reuses the loaded album track list', () => {
		expect(source).toContain("import { playTidalTracksNow, shuffleTidalTracksNow } from '$lib/stores/player';");
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('tracks.map((track) => tidalDiscographyTrackToPlayable(track))');
		expect(source).toContain("header?.title ?? 'album'");
		expect(source).not.toContain('function toPlayable(t: TidalDiscographyTrack)');
		expect(source).not.toContain('playTidalAlbum(tidalAlbumId)');
	});
});
