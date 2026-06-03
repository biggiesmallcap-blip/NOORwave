import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, 'RemoteMiniSearch.svelte'), 'utf8');

describe('remote mini search contracts', () => {
	test('aborts stale remote search requests when the query or mode changes', () => {
		expect(source).toContain('const controller = new AbortController();');
		expect(source).toContain('api.searchTidal(normalized, 12, controller.signal)');
		expect(source).toContain('api.search(normalized, 12, controller.signal)');
		expect(source).toContain('if (controller.signal.aborted) return;');
		expect(source).toContain('controller.abort();');
	});

	test('guards playlist loading against stale tab changes', () => {
		expect(source).toContain('let playlistLoadSeq = 0;');
		expect(source).toContain('const token = ++playlistLoadSeq;');
		expect(source).toContain("if (token !== playlistLoadSeq || mode !== 'playlists') return;");
		expect(source).toContain("if (token === playlistLoadSeq && mode === 'playlists') busy = false;");
	});

	test('routes remote track artwork through ArtworkImage fallbacks', () => {
		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(source).toContain('className="remote-search-thumb"');
		expect(source).toContain('src={track.artwork_url}');
		expect(source).toContain('size={320}');
		expect(source).toContain('fallbackText="NOOR"');
		expect(source).toContain('decorative={true}');
		expect(source).toContain(':global(.remote-search-thumb)');
		expect(source).not.toContain("import { upscaleTidalArtwork } from '$lib/utils/artwork';");
		expect(source).not.toMatch(/<img[\s\S]*(artwork_url|upscaleTidalArtwork)/);
	});
});
