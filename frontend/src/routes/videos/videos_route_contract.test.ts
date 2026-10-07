import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const watch = readFileSync(join(here, 'watch/+page.svelte'), 'utf8');
const sectionLayout = readFileSync(join(here, '+layout.svelte'), 'utf8');
const search = readFileSync(join(here, '../../lib/components/video/VideoSearchResults.svelte'), 'utf8');
const layoutSource = readFileSync(join(here, '../+layout.svelte'), 'utf8');

describe('video search results contract', () => {
	test('guards video search pagination against stale queries', () => {
		expect(search).toContain('let loadMoreSeq = 0;');
		expect(search).toContain('loadMoreSeq += 1;');
		expect(search).toContain('const seq = ++loadMoreSeq;');
		expect(search).toContain('const pageQuery = lastQuery;');
		expect(search).toContain('const pageOffset = offset;');
		expect(search).toContain('const isCurrentLoadMore = () =>');
		expect(search).toContain('seq === loadMoreSeq');
		expect(search).toContain('lastQuery === pageQuery');
		expect(search).toContain('offset === pageOffset');
		expect(search).toContain('const result = await api.searchTidalVideos(pageQuery, PAGE_SIZE, pageOffset);');
		expect(search).toContain('if (!isCurrentLoadMore()) return 0;');
		expect(search).toContain('if (seq === loadMoreSeq) loadingMore = false;');
	});

	test('a result plays with the whole result list queued behind it', () => {
		expect(search).toContain('queue: videos,');
		expect(search).toContain("source: 'search',");
		expect(search).toContain('<VideoCard {video}');
	});
});

describe('watch page contract', () => {
	test('hands its stage to the persistent video dock', () => {
		// The live <video> lives in VideoDock so audio survives navigation; the
		// page only exposes an anchor the dock positions its player over.
		expect(watch).toContain('bind:this={stageAnchor}');
		expect(watch).toContain('videoStageAnchor.set(stageAnchor)');
		expect(watch).not.toContain('<VideoPlayer');
	});

	test('keeps artist context actions, saving, and closing wired', () => {
		expect(watch).toContain('event.preventDefault();');
		expect(watch).toContain('event.stopPropagation();');
		expect(watch).toContain('buildArtistMenu({ tidal_id: current.artist_id');
		expect(watch).toContain('await api.setVideoSaved(item, saved)');
		expect(watch).toContain('clearVideoSession()');
		expect(watch).not.toContain('$:');
	});

	test('one queue: up next is the app queue panel, the page keeps exploring', () => {
		// A second up-next list beside the player duplicated the queue panel
		// and, taller than the player column, opened a gap under the video.
		expect(watch).not.toContain('videoSessionUpcoming');
		expect(watch).not.toContain('class="up-next"');
		expect(watch).toContain('await api.getRelatedVideos({');
		expect(watch).toContain('if (seq !== relatedRequest) return;');
		expect(watch).toContain('Keep exploring');
	});

	test('the address follows the playing video, so reload and copied links reopen it', () => {
		expect(watch).toContain('replaceState(watchUrl(id, {');
		expect(watch).toContain('artistName: current?.artist_name,');
		expect(watch).toContain('function openFromUrl()');
		expect(watch).toContain("if ($videoSession.current?.tidal_id === videoId) return;");
	});

	test('reserves the measured bottom player inset for video content and queue', () => {
		expect(layoutSource).toContain('bind:this={appShellElement}');
		expect(layoutSource).toContain('bind:this={bottomPlayerElement}');
		expect(layoutSource).toContain("shell.style.setProperty('--bottom-player-height'");
		expect(layoutSource).toContain('new ResizeObserver(updateBottomPlayerHeight)');
		expect(sectionLayout).toContain('max(var(--bottom-player-height, 0px), 44px, var(--safe-bottom))');
		expect(layoutSource).toContain('bottom: calc(var(--bottom-player-height) + var(--space-2));');
		expect(layoutSource).toMatch(
			/\.app-shell\[data-player-layout='bottom'\] \.video-queue-panel\.queue-drawer-open \{[^}]*z-index: var\(--z-overlay\);/
		);
	});

	test('keeps the bottom video player compact and discovery copy contained', () => {
		expect(layoutSource).toContain("grid-template-areas: 'art copy source actions queue heading';");
		expect(layoutSource).toContain(".app-shell[data-player-layout='bottom'] .video-panel-source");
		expect(layoutSource).toContain('text-overflow: ellipsis;');
		expect(layoutSource).toContain(".app-shell[data-player-layout='bottom'] .video-radio-hits { display: none; }");
	});
});
