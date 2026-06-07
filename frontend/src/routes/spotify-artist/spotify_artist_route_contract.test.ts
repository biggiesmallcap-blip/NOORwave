import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

describe('Spotify artist route contract', () => {
	test('guards artist load and polling against stale route responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS)');
		expect(source).toContain('setTimeout(() => void pollResolution(seq), delayMs)');
		expect(source).toContain('async function pollResolution(seq: number)');
		expect(source).toContain('if (seq !== loadSeq) { clearPoll(); return; }');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('const nextDetail = await api.getSpotifyArtist(id, controller.signal);');
		expect(source).toContain('const rel = await api.getSpotifyArtistRelated(id, controller.signal).catch(() => null);');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('loadSeq += 1;');
		expect(source).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS)');
		expect(source).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS * 2)');
	});

	test('keeps TIDAL row actions wired through the Spotify artist page', () => {
		expect(source).toContain('function buildRowMenu(t: SpotifyPlaylistTrack): MenuItem[]');
		expect(source).toContain('onclick={() => { const tr = asTidalPlayable(t); if (tr) void playTidalTrackNow(tr); }}');
		expect(source).toContain('function handleRowContextMenu(e: MouseEvent, t: SpotifyPlaylistTrack)');
		expect(source).toContain("oncontextmenu={(e) => handleRowContextMenu(e, t)}");
		expect(source).toContain('e.preventDefault();');
		expect(source).toContain('e.stopPropagation();');
		expect(source).toContain('use:lazyTidalArt');
	});
});
