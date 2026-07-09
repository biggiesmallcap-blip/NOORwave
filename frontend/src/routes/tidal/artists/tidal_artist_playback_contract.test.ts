import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

// The non-library TIDAL artist route is a thin wrapper around the shared
// ArtistDetail component; the TIDAL-mode load + playback logic lives there.
const wrapper = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');
const source = readFileSync(resolve(__dirname, '../../artists/ArtistDetail.svelte'), 'utf8');

describe('TIDAL artist playback contract', () => {
	test('route wrapper renders the shared view in TIDAL mode', () => {
		expect(wrapper).toContain("import ArtistDetail from '../../../artists/ArtistDetail.svelte'");
		expect(wrapper).toContain("source={{ kind: 'tidal', tidalArtistId }}");
	});

	test('ignores stale TIDAL profile responses', () => {
		expect(source).toContain('let tidalLoadSeq = 0');
		expect(source).toContain('async function loadTidalProfile(tidalId: number)');
		expect(source).toContain('const seq = ++tidalLoadSeq');
		// Served through the cache layer: in-flight dedupe + instant re-visits.
		expect(source).toContain('const res = await cachedApi.getTidalArtistProfile(tidalId)');
		expect(source).toContain('if (seq !== tidalLoadSeq) return');
		expect(source).toContain('void loadTidalProfile(source.tidalArtistId)');
	});

	test('track rows preserve discography metadata with the active artist fallback', () => {
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track'");
		expect(source).toContain('function artistTrackPlayable(track: TidalDiscographyTrack)');
		expect(source).toContain('tidalDiscographyTrackToPlayable(track, { artistTidalId: activeTidalArtistId })');
		expect(source).toContain('{@const playable = artistTrackPlayable(track)}');
		expect(source).not.toContain('function trackAsPlayable(t: TidalDiscographyTrack)');
	});

	test('TIDAL mode plays, shuffles, and seeds radio without a local artist id', () => {
		expect(source).toContain('await playTidalTracksNow(playable, tidalProfileName ?? \'artist\')');
		expect(source).toContain('void shuffleTidalTracksNow(playable, tidalProfileName ?? \'artist\')');
		expect(source).toContain('await startTidalSongRadio(artistTrackPlayable(seed))');
	});
});
