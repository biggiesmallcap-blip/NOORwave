import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'CommandPalette.svelte'), 'utf8');

describe('command palette search contracts', () => {
	test('searches local library and merges it with TIDAL results', () => {
		expect(source).toContain("import { mergeLocalIntoTidal } from '$lib/search/merge_local';");
		expect(source).toMatch(/api\.search\(\s*searchQuery\s*,\s*6\s*\)/);
		expect(source).toContain('mergeLocalIntoTidal(localResults, tidalResults)');
	});

	test('updates local results before waiting for external providers', () => {
		expect(source).toContain('let searchGeneration = $state(0);');
		expect(source).toContain('}, 120);');
		expect(source).toContain('void runPaletteSearch(searchQuery, generation);');
		expect(source).toContain('const localPromise = api.search(searchQuery, 6);');
		expect(source).toMatch(/localPromise[\s\S]*\.then\(\(localResults\)/);
		expect(source).toMatch(/api\.searchTidal\(\s*searchQuery\s*,\s*6\s*\)[\s\S]*\.then\(\(tidalResults\)/);
		expect(source).toMatch(/api\.searchSpotifyPlaylists\(\s*searchQuery\s*,\s*6\s*\)[\s\S]*\.then\(\(playlists\)/);
		expect(source).toContain('if (!isCurrentPaletteSearch(searchQuery, generation)) return;');
	});
});
