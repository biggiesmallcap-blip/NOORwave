import { describe, expect, test } from 'vitest';
import { existsSync, readFileSync } from 'node:fs';

const source = readFileSync(new URL('./GenreDetail.svelte', import.meta.url), 'utf8');
const route = readFileSync(new URL('../../../routes/genres/[id]/+page.svelte', import.meta.url), 'utf8');

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

	test('is a routed page built on shared rows, not an overlay', () => {
		expect(existsSync(new URL('./GenreInterior.svelte', import.meta.url))).toBe(false);
		expect(route).toContain('buildGenreSummary(snapshot, genreId)');
		expect(source).toContain("import TrackRow from '$lib/components/TrackRow.svelte';");
		expect(source).not.toContain('<canvas');
		expect(source).not.toContain('position: absolute;\n\t\tinset: 0;');
		// Once loaded, the list is the source of truth for the count.
		expect(source).toContain('let trackCount = $derived(loading ? node.trackCount : tracks.length);');
	});
});
