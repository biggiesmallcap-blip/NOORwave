import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const lib = join(here, '../../../lib/video/archive');
const page = readFileSync(join(here, '+page.svelte'), 'utf8');
const tile = readFileSync(join(lib, 'ArchiveTile.svelte'), 'utf8');
const lensBar = readFileSync(join(lib, 'LensBar.svelte'), 'utf8');
const river = readFileSync(join(lib, 'EraRiver.svelte'), 'utf8');
const drawer = readFileSync(join(lib, 'VersionsDrawer.svelte'), 'utf8');
const setlist = readFileSync(join(lib, 'ConcertSetlist.svelte'), 'utf8');

describe('archive route contract', () => {
	test('the URL is the lens state, and lens changes replace it', () => {
		expect(page).toContain('let lenses = $derived(parseLenses(page.url.searchParams));');
		expect(page).toContain('feed.request(lenses);');
		expect(page).toContain('goto(`/videos/archive${lensesToSearch(next)}`, { replaceState: true, keepFocus: true, noScroll: true })');
	});

	test('requests go through the debounced, abortable feed', () => {
		expect(page).toContain('fetch: (query, signal) => api.getVideoArchive(query, signal),');
		expect(page).toContain('debounceMs: 220,');
		expect(page).toContain('feed.dispose();');
	});

	test('a tile plays its best cut with the visible results queued', () => {
		expect(page).toContain('const queue = items.map((item) => item.best);');
		expect(page).toContain('playFromList(song.best, queue, lensSummary(lenses, sceneName))');
		expect(page).toContain('onversions={(picked) => (openSong = picked)}');
	});

	test('first load, refresh, building, error and filtered-empty are distinct states', () => {
		expect(page).toContain('class="tile-skeleton"');
		expect(page).toContain('class:dimmed={refreshing}');
		expect(page).toContain('The Archive is still being catalogued');
		expect(page).toContain('<ErrorState title="Could not open the Archive"');
		expect(page).toContain('Nothing under these lenses');
		expect(page).toContain('setLenses(clearLenses(lenses))');
	});

	test('infinite scroll pages through the feed', () => {
		expect(page).toContain('bind:this={sentinel}');
		expect(page).toContain('feed.loadMore()');
	});

	test('TIDAL editorial stays reachable', () => {
		expect(page).toContain('href="/videos/editorial"');
	});
});

describe('archive tile contract', () => {
	test('Search card language with the shared artwork fallback and menus', () => {
		expect(tile).toContain('<ArtworkImage className="poster" src={song.best.artwork_url} size={320}');
		expect(tile).toContain('buildVideoMenu(song.best)');
		expect(tile).toContain('buildArtistMenu({ tidal_id: artistId, name: artistName }, { isLocal: false })');
		expect(tile).toContain('<LightDot light={song.light} />');
	});

	test('the cuts badge only shows for songs with more than one cut', () => {
		expect(tile).toContain('{#if song.cut_count > 1}');
		expect(tile).toContain('x{song.cut_count} cuts');
	});
});

describe('lens bar contract', () => {
	test('every lens shows a live count from the facets', () => {
		expect(lensBar).toContain("facets?.scene");
		expect(lensBar).toContain('hueColor(scene.hue)');
		expect(lensBar).toContain('chipCount(cutCounts.get(cut) ?? 0)');
		expect(lensBar).toContain('count(depthCounts, option.value)');
		expect(lensBar).toContain('count(lengthCounts, option.value)');
		expect(lensBar).toContain('<EraRiver');
		expect(lensBar).toContain('options={SOURCES}');
		expect(lensBar).toContain('options={SORTS}');
	});

	test('the era river is draggable and has keyboard decade buttons', () => {
		expect(river).toContain('onpointerdown={onPointerDown}');
		expect(river).toContain('setPointerCapture(event.pointerId)');
		expect(river).toContain('onclick={(event) => pickDecade(d.decade, event.shiftKey)}');
	});
});

describe('versions and concerts contract', () => {
	test('versions load by song key, best first, every row playable with a menu', () => {
		expect(drawer).toContain('api.getVideoSongVersions(song.song_key)');
		expect(drawer).toContain('orderVersions(data.versions)');
		expect(drawer).toContain('playVersion(version, versions, data.title)');
		expect(drawer).toContain('buildVideoMenu(version.video)');
		expect(drawer).toContain('openConcert(show.id)');
	});

	test('a concert plays whole, in order, and any row plays from there', () => {
		expect(setlist).toContain('onclick={() => void playConcert(concert)}');
		expect(setlist).toContain('playConcert(concert, video.tidal_id)');
		expect(setlist).toContain('buildVideoMenu(video)');
	});
});
