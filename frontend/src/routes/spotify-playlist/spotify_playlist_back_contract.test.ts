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

	test('returns to the originating mood when opened from moods', () => {
		expect(source).toContain('SPOTIFY_MOODS_BY_SLUG.has(mood)');
		expect(source).toContain("from === 'moods'");
		expect(source).toContain("href: `/moods/${encodeURIComponent(mood)}`");
		expect(source).toContain("label: 'Back to mood'");
	});

	test('keeps search as the default direct-entry fallback', () => {
		expect(source).toContain("return { href: '/search', label: 'Back to search' };");
		expect(source).toContain('<a class="back-link" href={backLink.href}>&lt; {backLink.label}</a>');
	});
});
