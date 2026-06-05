import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8');

describe('videos route contract', () => {
	test('guards video search pagination against stale query and mix changes', () => {
		expect(source).toContain('let loadMoreSeq = 0;');
		expect(source).toContain('loadMoreSeq += 1;');
		expect(source).toContain('const seq = ++loadMoreSeq;');
		expect(source).toContain('const pageQuery = lastQuery;');
		expect(source).toContain('const pageOffset = offset;');
		expect(source).toContain('const isCurrentLoadMore = () =>');
		expect(source).toContain('seq === loadMoreSeq');
		expect(source).toContain('lastQuery === pageQuery');
		expect(source).toContain('offset === pageOffset');
		expect(source).toContain('const result = await api.searchTidalVideos(pageQuery, PAGE_SIZE, pageOffset);');
		expect(source).toContain('if (!isCurrentLoadMore()) return 0;');
		expect(source).toContain('if (seq === loadMoreSeq) loadingMore = false;');
	});

	test('keeps video selection, stream, and context actions wired', () => {
		expect(source).toContain('await fetchStream(video.tidal_id);');
		expect(source).toContain('void selectVideo(video);');
		expect(source).toContain('async function loadMix(mixId: string, autoPlayFirst = false)');
		expect(source).toContain('await loadMix(mixId, shouldPlayMix);');
		expect(source).toContain('buildArtistMenu({ tidal_id: selectedVideo.artist_id');
		expect(source).toContain('<VideoPlayer');
		expect(source).toContain('<VideoCard {video}');
		expect(source).not.toContain('$:');
	});
});
