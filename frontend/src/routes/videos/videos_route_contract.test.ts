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

	test('guards video mix loads against stale route responses', () => {
		expect(source).toContain('let mixLoadSeq = 0;');
		expect(source).toContain('const seq = ++mixLoadSeq;');
		expect(source).toContain('const isCurrentMixLoad = () => seq === mixLoadSeq && activeMixId === mixId;');
		expect(source).toContain('if (!isCurrentMixLoad()) return;');
		expect(source).toContain('if (autoPlayFirst && isCurrentMixLoad() && mixItems.length > 0)');
		expect(source).toContain('if (isCurrentMixLoad()) showToast(mixError, \'error\', 3200);');
		expect(source).toContain('if (seq === mixLoadSeq) loadingMix = false;');
		expect(source).toContain('mixLoadSeq += 1;');
	});

	test('keeps video selection, stream, and context actions wired', () => {
		// Playback is delegated to the persistent dock via the controller; the
		// route picks a video and hands it to playVideo() with a play context.
		expect(source).toContain('const ok = await playVideo(video, buildPlayContext(video));');
		expect(source).toContain('void selectVideo(video);');
		expect(source).toContain('async function loadMix(mixId: string, autoPlayFirst = false)');
		expect(source).toContain('await loadMix(mixId, shouldPlayMix);');
		expect(source).toContain('event.preventDefault();');
		expect(source).toContain('event.stopPropagation();');
		expect(source).toContain('buildArtistMenu({ tidal_id: selectedVideo.artist_id');
		expect(source).toContain('<VideoCard {video}');
		expect(source).not.toContain('$:');
	});

	test('hands the hero placeholder to the persistent video dock', () => {
		// The live <video> lives in VideoDock so audio survives navigation; the
		// route only exposes an anchor the dock positions its player over.
		expect(source).toContain('bind:this={stageAnchor}');
		expect(source).toContain('videoStageAnchor.set(stageAnchor)');
		expect(source).not.toContain('<VideoPlayer');
	});
});
