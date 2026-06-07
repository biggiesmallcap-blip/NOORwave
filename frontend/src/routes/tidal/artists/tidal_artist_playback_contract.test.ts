import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('TIDAL artist playback contract', () => {
	test('ignores stale route-load responses', () => {
		expect(source).toContain('let loadSeq = 0');
		expect(source).toContain('async function load(id: number)');
		expect(source).toContain('const seq = ++loadSeq');
		expect(source).toContain('failedArtworkUrls = {}');
		expect(source).toContain('const nextProfile = await api.getTidalArtistProfile(id)');
		expect(source).toContain('if (seq !== loadSeq) return');
		expect(source).toContain('if (seq === loadSeq) loading = false');
		expect(source).toContain('const id = tidalArtistId');
		expect(source).toContain('void load(id)');
		expect(source).not.toContain('let cancelled = false');
	});

	test('track rows preserve discography metadata with the route artist fallback', () => {
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track'");
		expect(source).toContain('function artistTrackPlayable(track: TidalDiscographyTrack)');
		expect(source).toContain('tidalDiscographyTrackToPlayable(track, { artistTidalId: tidalArtistId })');
		expect(source).toContain('track={artistTrackPlayable(track)}');
		expect(source).not.toContain('function trackAsPlayable(t: TidalDiscographyTrack)');
	});
});
