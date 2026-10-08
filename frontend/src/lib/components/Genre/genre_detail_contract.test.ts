import { describe, expect, test } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';

const source = readFileSync(new URL('./GenreDetail.svelte', import.meta.url), 'utf8');

describe('GenreDetail load contract', () => {
	test('ignores stale genre track loads and clears previous content', () => {
		expect(source).toContain("import { onDestroy, untrack } from 'svelte';");
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('const targetNode = node;');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('tracks = [];');
		expect(source).toContain('topArtists = [];');
		expect(source).toContain('const response = await cachedApi.getGenreTracks(targetNode.id, true);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('loadSeq += 1;');
	});

	test('is the in-galaxy drawer body built on shared rows', () => {
		expect(existsSync(new URL('./GenreInterior.svelte', import.meta.url))).toBe(false);
		expect(source).toContain('onSelectGenre(child.id)');
		expect(source).not.toContain('href={`/genres/');
		expect(source).toContain("import TrackRow from '$lib/components/TrackRow.svelte';");
		expect(source).not.toContain('<canvas');
		// Once loaded, the list is the source of truth for the count.
		expect(source).toContain('let trackCount = $derived(loading ? node.trackCount : tracks.length);');
	});
});
