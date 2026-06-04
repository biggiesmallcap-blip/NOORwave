import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const trackPage = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');
const albumPage = readFileSync(join(here, '..', 'spotify-album', '[id]', '+page.svelte'), 'utf8');
const client = readFileSync(join(here, '..', '..', 'lib', 'api', 'client.ts'), 'utf8');

describe('Spotify save to library contract', () => {
	test('posts track and album saves through dedicated API client methods', () => {
		expect(client).toContain('export interface SpotifyLibrarySaveResponse');
		expect(client).toContain('saveSpotifyTrack(spotifyId: string)');
		expect(client).toContain('`/api/spotify-track/save`');
		expect(client).toContain('saveSpotifyAlbum(spotifyId: string)');
		expect(client).toContain('`/api/spotify-album/save`');
		expect(client).toContain('body: JSON.stringify({ spotify_id: spotifyId })');
		expect(client).toContain('timeoutMs: BULK_QUEUE_API_TIMEOUT_MS');
	});

	test('wires the Spotify track page to save resolved tracks', () => {
		expect(trackPage).toContain('let saving = $state(false);');
		expect(trackPage).toContain('const canSave = $derived');
		expect(trackPage).toContain('api.saveSpotifyTrack(id)');
		expect(trackPage).toContain('disabled={saving || !canSave}');
		expect(trackPage).toContain("{saving ? 'Saving...' : 'Save to library'}");
		expect(trackPage).toContain('Save failed: {saveErr}');
		expect(trackPage).not.toContain('$:');
	});

	test('wires the Spotify album page to save resolved album tracks', () => {
		expect(albumPage).toContain('let saving = $state(false);');
		expect(albumPage).toContain('api.saveSpotifyAlbum(id)');
		expect(albumPage).toContain('disabled={saving || resolvedCount === 0}');
		expect(albumPage).toContain('Saved ${res.imported} ${trackLabel(res.imported)}');
		expect(albumPage).toContain("{saving ? 'Saving...' : 'Save to library'}");
		expect(albumPage).toContain('Save failed: {saveErr}');
		expect(albumPage).not.toContain('$:');
	});
});
