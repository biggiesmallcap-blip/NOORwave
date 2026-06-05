import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('./GenreInterior.svelte', import.meta.url), 'utf8');

describe('GenreInterior load contract', () => {
	test('ignores stale genre track loads and clears previous interior content', () => {
		expect(source).toContain("import { onDestroy, onMount } from 'svelte';");
		expect(source).toContain('let loadSeq = 0;');
		expect(source).toContain('const targetNode = node;');
		expect(source).toContain('const seq = ++loadSeq;');
		expect(source).toContain('tracks = [];');
		expect(source).toContain('artistClusters = [];');
		expect(source).toContain('const response = await cachedApi.getGenreTracks(targetNode.id, true);');
		expect(source).toContain('if (seq !== loadSeq) return;');
		expect(source).toContain('if (seq === loadSeq) loading = false;');
		expect(source).toContain('loadSeq += 1;');
	});
});
