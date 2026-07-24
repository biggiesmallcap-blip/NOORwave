import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const read = (rel) => readFileSync(resolve(import.meta.dirname, rel), 'utf8');
const source = read('../src/routes/videos/+page.svelte');
const dock = read('../src/lib/components/video/VideoDock.svelte');
const store = read('../src/lib/stores/video_session.ts');
const shelves = read('../src/lib/components/search/TidalDiscoverShelves.svelte');

describe('Videos editorial browse state', () => {
	test('hero slot arbitration: the layer yields only while the player owns the stage', () => {
		expect(source).toContain('let videoSessionActive = $derived');
		expect(source).toContain('let playerOwnsStage = $derived(videoSessionActive && !browseMode)');
		expect(source).toContain('let showEditorialLayer = $derived(!playerOwnsStage');
		expect(source).toContain('{#if showEditorialLayer}');
		expect(source).toContain('transition:fade');
	});

	test('search focus recedes the layer without unmounting it', () => {
		expect(source).toContain('let searchFocused = $derived(query.trim().length > 0)');
		expect(source).toContain('class:receded={searchFocused}');
		expect(source).toContain('.editorial-layer.receded');
		expect(source).toContain('grid-template-rows: 0fr');
		expect(source).toContain('pointer-events: none');
	});

	test('daily picks feed the mural and play through the shared video queue', () => {
		expect(source).toContain("discoverSets.find((s) => s.slug === 'daily-picks')");
		expect(source).toContain('<ChartMural');
		expect(source).toContain('await playVideo(video, {');
		// Every editorial surface plays through one path, never an inline player.
		expect(source).toContain('async function playFromQueue');
		expect(source).not.toContain('<VideoPlayer');
	});

	test('every other built set renders as its own shelf', () => {
		expect(source).toContain("discoverSets.filter((s) => s.slug !== 'daily-picks'");
		expect(source).toContain('{#each shelfSets as set (set.slug)}');
		expect(source).toContain('<VideoSetShelf');
		expect(source).toContain('onPlayAll={() => playFromSet(set, 0)}');
	});

	test('mural rotation pauses under focus, playback, and hover', () => {
		expect(source).toContain('MURAL_ROTATE_MS');
		expect(source).toContain('if (!muralPaused && !searchFocused && !playerOwnsStage) jumpMural(1);');
		expect(source).toContain('onPauseChange={(paused) => (muralPaused = paused)}');
	});

	test('TIDAL editorial modules render through the shared shelves with claimed clicks', () => {
		expect(source).toContain("api.getTidalPage('videos')");
		expect(source).toContain("From TIDAL's desk");
		expect(source).toContain('<TidalDiscoverShelves');
		expect(source).toContain('onItemSelect={handleEditorialSelect}');
		// Same-route goto is a no-op, so the route must claim these clicks.
		expect(source).toContain('function handleEditorialSelect(item: TidalHomeItem): boolean');
		expect(source).toContain('href="/tidal/videos">More from TIDAL</a>');
	});

	test('legacy landing chips only appear when there is no editorial content', () => {
		expect(source).toContain('!hasBrowseContent && !loadingBrowse}');
	});
});

describe('Queue rows for shelf and editorial picks', () => {
	test('the jump lookup searches the live session queue, not just search results', () => {
		// Shelf and editorial picks only ever live in the session queue, so a
		// lookup limited to the route's local arrays left every queue row dead
		// for those sources.
		expect(source).toContain('function findVideoInCurrentContext');
		const body = source.slice(
			source.indexOf('function findVideoInCurrentContext'),
			source.indexOf('function toggleVideoAutoplay')
		);
		expect(body).toContain('...$videoSession.queue');
	});

	test('jumping inside a shelf keeps that shelf as the queue', () => {
		// Otherwise the fallthrough swaps the shelf for the (usually empty)
		// search results and autoplay dies.
		const body = source.slice(
			source.indexOf('function buildPlayContext'),
			source.indexOf('async function selectVideo')
		);
		expect(body).toContain('session.queue.some((item) => item.tidal_id === video.tidal_id)');
		expect(body).toContain('queue: session.queue');
		expect(body).toContain('sourceLabel: session.sourceLabel');
	});
});

describe('Video modules never fall through to the audio detail page', () => {
	test('View all is hidden for video modules unless the host handles it', () => {
		// /search/discover/[id] plays every item via playTidalTrackNow, so
		// following it from a video module plays the song, not the video.
		expect(shelves).toContain(
			"let showViewAll = $derived(mediaKind !== 'video' || Boolean(onViewAll))"
		);
		expect(shelves).toContain('{#if showViewAll}');
	});
});

describe('Browse while playing', () => {
	test('the route offers a way back to the picks without stopping playback', () => {
		expect(source).toContain('function backToPicks()');
		expect(source).toContain('setVideoBrowseMode(true)');
		expect(source).toContain('>Back to picks</button>');
		// Browse mode must withdraw the stage anchor: its absence is the signal
		// the dock reads to fall back to the mini player.
		expect(source).toContain('let showVideoHero = $derived(\n\t\t!browseMode &&');
	});

	test('and a way back to the player that does not restart it', () => {
		expect(source).toContain('function backToPlayer()');
		expect(source).toContain('setVideoBrowseMode(false)');
		expect(source).toContain('Back to the player');
	});

	test('the dock docks to the corner in browse mode and stays mounted', () => {
		expect(dock).toContain("let mode = $derived(onVideosRoute && !$videoBrowseMode ? 'full' : 'mini')");
		expect(dock).toContain('setVideoBrowseMode(false)');
		expect(dock).toContain("page.url.pathname.startsWith('/videos')");
		expect(dock).toContain('getBoundingClientRect()');
	});

	test('browse mode resets when a new video starts or the session ends', () => {
		expect(store).toContain('export const videoBrowseMode = writable(false)');
		expect(store).toContain('export function setVideoBrowseMode');
		const playVideoBody = store.slice(store.indexOf('export async function playVideo'));
		expect(playVideoBody).toContain('videoBrowseMode.set(false)');
		const clearBody = store.slice(store.indexOf('export function clearVideoSession'));
		expect(clearBody).toContain('videoBrowseMode.set(false)');
	});
});
