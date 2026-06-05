import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('remote TIDAL artist playback contract', () => {
	test('bulk and radio actions preserve discography track metadata', () => {
		expect(source).toContain("import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('playTidalTracksNow(p.top_tracks.map(tidalDiscographyTrackToPlayable)');
		expect(source).toContain('shuffleTidalTracksNow(p.top_tracks.map(tidalDiscographyTrackToPlayable)');
		expect(source).toContain('startTidalSongRadio(tidalDiscographyTrackToPlayable(p.top_tracks[0]))');
		expect(source).not.toContain('function toPlayable(t: TidalDiscographyTrack)');
	});
});
