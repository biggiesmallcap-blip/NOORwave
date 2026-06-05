import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('TIDAL album playback contract', () => {
	test('ignores stale route-load responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('async function load(id: number)');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('failedArtworkUrls = {};');
		expect(source).toContain('const res = await api.getTidalAlbumTracks(id);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('const id = tidalAlbumId;');
		expect(source).toContain('void load(id);');
		expect(source).not.toContain('void load();');
	});

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
