import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, 'commands.ts'), 'utf8');

describe('slash command contracts', () => {
	test('normalizes TIDAL search results before playback, queue, and radio actions', () => {
		expect(source).toContain("import { tidalSearchTrackToPlayable, trackToTidalPlayable } from '$lib/utils/track';");
		expect(source).toContain('await playTidalTrackNow(tidalSearchTrackToPlayable(first));');
		expect(source).toContain('await addTidalTrackToQueue(tidalSearchTrackToPlayable(first));');
		expect(source).toContain('await startTidalSongRadio(tidalSearchTrackToPlayable(first));');
		expect(source).toContain('const playable = trackToTidalPlayable(track);');
		expect(source).toContain('if (playable) await startTidalSongRadio(playable);');
		expect(source).not.toContain('{ ...first, artist_tidal_id: first.artist_id ?? null }');
	});
});
