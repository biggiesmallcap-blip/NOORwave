import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('TIDAL artist playback contract', () => {
	test('track rows preserve discography metadata with the route artist fallback', () => {
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track'");
		expect(source).toContain('function artistTrackPlayable(track: TidalDiscographyTrack)');
		expect(source).toContain('tidalDiscographyTrackToPlayable(track, { artistTidalId: tidalArtistId })');
		expect(source).toContain('track={artistTrackPlayable(track)}');
		expect(source).not.toContain('function trackAsPlayable(t: TidalDiscographyTrack)');
	});
});
