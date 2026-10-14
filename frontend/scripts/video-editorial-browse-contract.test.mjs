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
		expect(source).not.toContain('<ScopeTabs');
	});

	test('the tab is the On Air dial: channels on a clock, tuned in from a pick', () => {
		// The shelf landing became On Air (docs/plans/2026-10-14-atlas-and-air.md);
		// its own contract lives in src/lib/video/onair/onair_contract.test.ts.
		expect(source).toContain('<ChannelCard {channel} {slot}');
		expect(source).toContain('<ChannelRow {channel} {slot}');
		expect(source).toContain('await tuneChannel(channel);');
		expect(source).not.toContain('<GuideRow');
		expect(source).not.toContain('VideoSetShelf');
		expect(playCollection).toContain('export async function playFromShelf');
		expect(playCollection).toContain('const ok = await playVideo(video, {');
	});

	test("TIDAL's editorial modules stay on their own tab", () => {
		expect(source).not.toContain("api.getTidalPage('videos')");
		expect(source).not.toContain('<TidalDiscoverShelves');
		expect(playCollection).toContain('export function playEditorialItem(item: TidalHomeItem, modules: TidalHomeModule[]): boolean');
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
		// The shared CommandHeader puts Back beside the field and the tabs
		// snippet under it (command-header-contract.test.mjs).
		const header = layout.slice(layout.indexOf('<CommandHeader'), layout.indexOf('</CommandHeader>'));
		expect(header).toContain('{#snippet field()}');
		expect(header.indexOf('<SearchField')).toBeLessThan(header.indexOf('{#snippet tabs()}'));
		expect(header).toContain('<ScopeTabs tabs={VIDEO_TABS} current={tab} label="Video pages" replaceState />');
		// Editorial lives under the Archive tab since the Atlas and Air tabs.
		expect(section).toContain("'/videos/editorial': 'archive',");
	});

	test('Back is unconditional and returns to where the listener came from', () => {
		const open = layout.slice(layout.indexOf('<CommandHeader'), layout.indexOf('{#snippet field()}'));
		expect(open).toContain("onback={() => goBack(onWatchPage ? '/videos' : '/')}");
		expect(layout.slice(0, layout.indexOf('<CommandHeader')).split('<div class="video-section">')[1]).not.toContain('{#if');
	});

	test('the field filters in place on Liked, Archive and Atlas and searches TIDAL everywhere else', () => {
		expect(layout).toContain('let filtersInPlace = $derived(onLiked || onArchive || onAtlas);');
		expect(layout).toContain('let searchingTidal = $derived(query.length > 0 && !filtersInPlace);');
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
