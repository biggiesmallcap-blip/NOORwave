import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'CommandPalette.svelte'), 'utf8');

describe('command palette search contracts', () => {
	test('searches local library and merges it with TIDAL results', () => {
		expect(source).toContain("import { mergeLocalIntoTidal } from '$lib/search/merge_local';");
		expect(source).toMatch(/api\.search\(\s*searchQuery\s*,\s*6\s*,\s*signal\s*\)/);
		expect(source).toContain('mergeLocalIntoTidal(localResults, tidalResults)');
	});

	test('updates local results before waiting for external providers', () => {
		expect(source).toContain('let searchGeneration = $state(0);');
		expect(source).toContain("import { PRIMARY_SEARCH_DEBOUNCE_MS, SECONDARY_PROVIDER_DELAY_MS } from '$lib/search/search_timing';");
		expect(source).toContain('}, PRIMARY_SEARCH_DEBOUNCE_MS);');
		expect(source).toContain('void runPaletteSearch(searchQuery, generation, request.token, request.signal);');
		expect(source).toContain('const localPromise = api.search(searchQuery, 6, signal);');
		expect(source).toMatch(/localPromise[\s\S]*\.then\(\(localResults\)/);
		expect(source).toMatch(/api\.searchTidal\(\s*searchQuery\s*,\s*6\s*,\s*signal\s*\)[\s\S]*\.then\(\(tidalResults\)/);
		expect(source).toMatch(/api\.searchSpotifyPlaylists\(\s*searchQuery\s*,\s*6\s*,\s*signal\s*\)[\s\S]*\.then\(\(playlists\)/);
		expect(source).toContain('if (!isCurrentPaletteSearch(searchQuery, generation, requestToken)) return;');
	});

	test('defers playlist-provider work until the primary search has had a head start', () => {
		expect(source).toMatch(/setTimeout\(\(\) => \{[\s\S]*api\.searchSpotifyPlaylists\(searchQuery, 6, signal\)[\s\S]*\}, SECONDARY_PROVIDER_DELAY_MS\);/);
	});

	test('invalidates pending searches when the palette closes or unmounts', () => {
		expect(source).toContain("import { onDestroy } from 'svelte';");
		expect(source).toContain('clearTimeout(debounceTimer);');
		expect(source).toContain('searchGeneration += 1;');
		expect(source).toContain('loading = false;');
		expect(source).toContain('requestGate.isCurrent(requestToken)');
		expect(source).toContain('searchGeneration === generation');
		expect(source).toContain('onDestroy(() => {');
	});

	test('aborts every provider request when the query changes', () => {
		expect(source).toContain("import { createLatestRequestGate } from '$lib/search/latest_request';");
		expect(source).toContain('requestGate.invalidate();');
		expect(source).toContain('api.search(searchQuery, 6, signal)');
		expect(source).toContain('api.searchTidal(searchQuery, 6, signal)');
		expect(source).toContain('api.searchSpotifyPlaylists(searchQuery, 6, signal)');
	});

	test('normalizes TIDAL search rows before palette play and menu actions', () => {
		expect(source).toContain("import { tidalSearchTrackToPlayable } from '$lib/utils/track';");
		expect(source).toContain('await playTidalTrackNow(tidalSearchTrackToPlayable(track));');
		expect(source).toContain('const playable = tidalSearchTrackToPlayable(track);');
		expect(source).toContain("onSelect: () => void playTidalTrackNow(playable)");
		expect(source).toContain("onSelect: () => void addTidalTrackToQueue(playable)");
		expect(source).not.toContain('{ ...track, artist_tidal_id: track.artist_id ?? null }');
	});
});
