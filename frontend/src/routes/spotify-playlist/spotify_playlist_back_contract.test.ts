import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '[id]', '+page.svelte'), 'utf8');

describe('Spotify playlist back link contract', () => {
	test('caches playlist cover metadata after explicit playlist opens', () => {
		expect(source).toContain("import { putCachedSpotifyChartMeta } from '$lib/stores/spotify-chart-meta-cache';");
		expect(source).toContain('putCachedSpotifyChartMeta(id, res.playlist);');
	});

	test('guards playlist load, retry, and polling against stale route responses', () => {
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('function schedulePoll(seq: number, delayMs = POLL_INTERVAL_MS)');
		expect(source).toContain('setTimeout(() => void pollResolution(seq), delayMs)');
		expect(source).toContain('async function pollResolution(seq: number)');
		expect(source).toContain('if (seq !== loadSeq) {');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('async function attemptLoad(id: string, retryOn5xx: boolean, seq: number)');
		expect(source).toContain('await attemptLoad(id, /* retryOn5xx */ true, seq);');
		expect(source).toContain('if (seq === loadSeq && spotifyId.trim() === id)');
		expect(source).toContain('await attemptLoad(id, /* retryOn5xx */ false, seq);');
		expect(source).toContain('loadSeq += 1;');
		expect(source).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS)');
		expect(source).not.toContain('setTimeout(pollResolution, POLL_INTERVAL_MS * 2)');
	});

	test('returns to the originating mood when opened from moods', () => {
		expect(source).toContain('SPOTIFY_MOODS_BY_SLUG.has(mood)');
		expect(source).toContain("from === 'moods'");
		expect(source).toContain("href: `/moods/${encodeURIComponent(mood)}`");
		expect(source).toContain("label: 'Back to mood'");
	});

	test('returns to the originating search query when opened from search', () => {
		expect(source).toContain("const q = params.get('q')?.trim();");
		expect(source).toContain("from === 'search' && q");
		expect(source).toContain("href: `/search?q=${encodeURIComponent(q)}`");
		expect(source).toContain("label: 'Back to search'");
	});

	test('keeps search as the default direct-entry fallback', () => {
		expect(source).toContain("return { href: '/search', label: 'Back to search' };");
		expect(source).toContain('onclick={() => goBack(backLink.href)}');
	});
});
