import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const source = readFileSync(resolve(__dirname, '[id]/+page.svelte'), 'utf8');

describe('TIDAL album playback contract', () => {
	test('play all reuses the loaded album track list', () => {
		expect(source).toContain("import { playTidalTracksNow } from '$lib/stores/player';");
		expect(source).toContain('album_tidal_id: t.album_tidal_id ?? null');
		expect(source).toContain('async function playLoadedAlbum()');
		expect(source).toContain("playTidalTracksNow(tracks.map(trackAsPlayable), header()?.title ?? 'album')");
		expect(source).toContain('onclick={() => void playLoadedAlbum()}');
		expect(source).not.toContain('playTidalAlbum(tidalAlbumId)');
	});
});
