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

	test('guards the Spotify track page against stale route loads and polling', () => {
		expect(trackPage).toContain('let loadSeq = 0;');
		expect(trackPage).toContain('function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS)');
		expect(trackPage).toContain('setTimeout(() => void pollResolution(seq), delayMs)');
		expect(trackPage).toContain('async function pollResolution(seq: number)');
		expect(trackPage).toContain('if (seq !== loadSeq) {');
		expect(trackPage).toContain('if (seq !== loadSeq) return;');
		expect(trackPage).toContain('const seq = ++loadSeq;');
		expect(trackPage).toContain('const nextDetail = await api.getSpotifyTrack(id, controller.signal);');
		expect(trackPage).toContain('const rel = await api.getSpotifyTrackRelated(id, controller.signal).catch(() => null);');
		expect(trackPage).toContain('if (seq === loadSeq) loading = false;');
		expect(trackPage).toContain('loadSeq += 1;');
		expect(trackPage).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS)');
		expect(trackPage).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS * 2)');
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

	test('keeps Spotify album row context menus app-owned', () => {
		expect(albumPage).toContain('function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack)');
		expect(albumPage).toContain('e.preventDefault();');
		expect(albumPage).toContain('e.stopPropagation();');
		expect(albumPage).toContain("oncontextmenu={(e) => handleRowContextMenu(e, t)}");
		expect(albumPage).toContain('openContextMenu(e, buildRowMenu(t), t.title ??');
	});

	test('keeps Spotify track header and row context menus app-owned', () => {
		expect(trackPage).toContain('function handleHeaderContextMenu(e: MouseEvent)');
		expect(trackPage).toContain('function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack)');
		expect(trackPage).toContain('e.preventDefault();');
		expect(trackPage).toContain('e.stopPropagation();');
		expect(trackPage).toContain('oncontextmenu={handleHeaderContextMenu}');
		expect(trackPage).toContain("oncontextmenu={(e) => handleRowContextMenu(e, t)}");
	});

	test('guards the Spotify album page against stale route loads and polling', () => {
		expect(albumPage).toContain('let loadSeq = 0;');
		expect(albumPage).toContain('function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS)');
		expect(albumPage).toContain('setTimeout(() => void pollResolution(seq), delayMs)');
		expect(albumPage).toContain('async function pollResolution(seq: number)');
		expect(albumPage).toContain('if (seq !== loadSeq) { clearPoll(); return; }');
		expect(albumPage).toContain('if (seq !== loadSeq) return;');
		expect(albumPage).toContain('const seq = ++loadSeq;');
		expect(albumPage).toContain('const res = await api.getSpotifyAlbum(id, controller.signal);');
		expect(albumPage).toContain('const rel = await api.getSpotifyAlbumRelated(id, controller.signal).catch(() => null);');
		expect(albumPage).toContain('if (seq === loadSeq) loading = false;');
		expect(albumPage).toContain('loadSeq += 1;');
		expect(albumPage).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS)');
		expect(albumPage).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS * 2)');
	});
});
