import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const source = read('../src/routes/videos/+page.svelte');
const layout = read('../src/routes/videos/+layout.svelte');
const watch = read('../src/routes/videos/watch/+page.svelte');
const dock = read('../src/lib/components/video/VideoDock.svelte');
const store = read('../src/lib/stores/video_session.ts');
const section = read('../src/lib/video/section.ts');
const playCollection = read('../src/lib/video/play_collection.ts');
const shelves = read('../src/lib/components/search/TidalDiscoverShelves.svelte');
const appCss = read('../src/app.css');

function functionBody(text, signature) {
	const start = text.indexOf(signature);
	return text.slice(start, text.indexOf('\n}\n', start));
}

describe('Videos tab browse state', () => {
	test('the tab only browses: no player, hero, or search of its own', () => {
		expect(source).not.toContain('<VideoPlayer');
		expect(source).not.toContain('videoStageAnchor');
		expect(source).not.toContain('<SearchField');
		expect(source).not.toContain('<VideoNavigation');
	});

	test('daily picks lead as the featured row and play through the shared video queue', () => {
		expect(source).toContain("discoverSets.find((s) => s.slug === 'daily-picks')");
		expect(source).toContain('<GuideFeature label={`Daily picks: ${dailySet.title}`}');
		expect(source).not.toContain('<ChartMural');
		expect(playCollection).toContain('export async function playFromShelf');
		expect(playCollection).toContain('const ok = await playVideo(video, {');
	});

	test('every other built set is a guide row; a frame plays the set from that video', () => {
		expect(source).toContain("discoverSets.filter((s) => s.slug !== 'daily-picks'");
		expect(source).toContain('{#each shelfSets as set, index (set.slug)}');
		expect(source).toContain('<GuideRow');
		expect(source).toContain('onplay={(startWith) => playSetFrom(set, startWith)}');
		expect(source).not.toContain('VideoSetShelf');
	});

	test('shelf playback uses the full row while browse radio uses the library mix', () => {
		const shelfPlay = source.slice(source.indexOf('function playFromSet'), source.indexOf('function playBrowseMix'));
		expect(shelfPlay).toContain('playFromShelf(video, set.items, set.title, { autoplay: true })');
		const browsePlay = source.slice(source.indexOf('function playBrowseMix'), source.indexOf('async function handleDeepLink'));
		expect(browsePlay).toContain("playFromShelf(first, browseMix, 'Video radio', { autoplay: true, continuous: true, radioScope: 'library' })");
		expect(source).toContain('onclick={() => dailySet && playFromSet(dailySet, 0)}>Play all</button>');
	});

	test("TIDAL's editorial modules stay on their own tab", () => {
		expect(source).not.toContain("api.getTidalPage('videos')");
		expect(source).not.toContain('<TidalDiscoverShelves');
		expect(playCollection).toContain('export function playEditorialItem(item: TidalHomeItem, modules: TidalHomeModule[]): boolean');
	});

	test('landing chips only appear when there is nothing to browse', () => {
		expect(source).toContain('{#if !hasBrowseContent && !loadingBrowse}');
		expect(source).toContain('onclick={() => videoSectionQuery.set(item)}');
	});

	test('old deep links move to the watch page or play their collection', () => {
		expect(source).toContain('await goto(watchUrl(videoId, {');
		expect(source).toContain("await goto('/videos', { replaceState: true, keepFocus: true });");
		expect(source).toContain("void playVideoCollection('mix', mixId)");
		expect(source).toContain("void playVideoCollection('playlist', playlistId)");
	});

	test('shelf posters fade in instead of popping as each decodes', () => {
		const card = read('../src/lib/components/video/VideoCard.svelte');
		expect(card).toContain('fadeIn={true}');
	});
});

describe('Video modules never fall through to the audio detail page', () => {
	test('View all is hidden for video modules unless the host handles it', () => {
		// /search/discover/[id] plays every item via playTidalTrackNow, so
		// following it from a video module plays the song, not the video.
		expect(shelves).toContain("mediaKind !== 'video' || Boolean(onViewAll)");
		expect(shelves).toContain('let showViewAll = $derived(');
		expect(shelves).toContain('{#if canViewAll(mod)}');
		expect(shelves).toContain('return showViewAll && Boolean(mod.more_path);');
	});

	test('a video item outside the section opens the watch page', () => {
		expect(shelves).toContain('void goto(watchUrl(item.id));');
		expect(shelves).not.toContain('/videos?videoId=');
	});
});

describe('Video section flow', () => {
	test('one header for every tab: Back and search on the first row, pills under it', () => {
		const header = layout.slice(layout.indexOf('<header class="video-header">'), layout.indexOf('</header>'));
		expect(header).toContain('class="back-link"');
		expect(header.indexOf('class="back-link"')).toBeLessThan(header.indexOf('<SearchField'));
		expect(header.indexOf('<SearchField')).toBeLessThan(header.indexOf('<VideoNavigation'));
		expect(header).toContain('<VideoNavigation current={tab} />');
		expect(section).toContain("{ id: 'editorial', href: '/videos/editorial', label: 'TIDAL editorial' }");
	});

	test('Back is unconditional and returns to where the listener came from', () => {
		expect(layout).toContain("onclick={() => goBack(onWatchPage ? '/videos' : '/')}");
		const row = layout.slice(layout.indexOf('<div class="search-row">'), layout.indexOf('<div class="search-slot">'));
		expect(row).not.toContain('{#if');
	});

	test('the field filters likes on Liked and searches TIDAL everywhere else', () => {
		expect(layout).toContain("let searchingTidal = $derived(query.length > 0 && tab !== 'liked');");
		expect(layout).toContain('<VideoSearchResults />');
		expect(layout).toContain('<div class="section-body" hidden={searchingTidal}>');
		expect(layout).toContain('Search all of TIDAL for');
	});

	test('every pick opens the watch page; queue steps never move the listener', () => {
		expect(layout).toContain('const nonce = $videoStageReveal;');
		expect(layout).toContain('if (window.location.pathname !== WATCH_PATH) void goto(WATCH_PATH);');
		expect(store).toContain('if (!opts.step) revealVideoStage();');
		for (const fn of ['export async function advanceVideo', 'export async function previousVideo', 'export async function nextVideo']) {
			expect(functionBody(store, fn), fn).toContain('step: true');
		}
	});

	test('only the watch page publishes a stage; everywhere else is the corner player', () => {
		expect(watch).toContain('videoStageAnchor.set(stageAnchor)');
		expect(source).not.toContain('videoStageAnchor');
		expect(dock).toContain("stageUsable ? 'full' : panelUsable ? 'panel' : 'mini'");
		expect(dock).toContain('stageRect.width > 0 && stageRect.height > 0');
		expect(dock).toContain('void goto(WATCH_PATH);');
		expect(store).not.toContain('videoBrowseMode');
	});
});
