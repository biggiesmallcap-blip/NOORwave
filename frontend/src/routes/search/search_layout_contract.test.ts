import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '+page.svelte'), 'utf8');

function cssBlock(selector: string): string {
	const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const match = source.match(new RegExp(`${escaped}\\s*\\{(?<body>[^}]*)\\}`));
	if (!match?.groups?.body) {
		throw new Error(`Missing CSS block for ${selector}`);
	}
	return match.groups.body;
}

describe('search layout contracts', () => {
	test('filter pills stay centered under the search input', () => {
		const block = cssBlock('.filter-pills');

		expect(block).toContain('margin: 14px auto 0');
		expect(block).toContain('max-width: 720px');
		expect(block).toContain('justify-content: center');
	});

	test('search page renders local results before external providers finish', () => {
		expect(source).toContain('let searchGeneration = $state(0)');
		expect(source).toContain('let loadMoreSeq = 0');
		expect(source).toContain('}, 120)');
		expect(source).toContain('loadMoreSeq += 1');
		expect(source).toContain('loadingMore = false');
		expect(source).toContain('const localPromise = cachedApi.search(q, SEARCH_PAGE_SIZE)');
		expect(source).toContain('void localPromise.then((localResults) => {');
		expect(source).toContain('if (!isCurrentSearch(q, generation, signal)) return');
		expect(source).toContain('void tracksPromise.then((tidalResults) => {');
		expect(source).toContain('void tidalPlaylistPromise.then((playlistResults) => {');
		expect(source).toContain('void spotifyPlaylistPromise.then((playlistResults) => {');
		expect(source).toContain('const providerSearchDone = $derived(');
		expect(source).toContain('{:else if allProviderResultsEmpty && providerSearchDone}');
	});

	test('search pagination ignores stale load-more responses', () => {
		expect(source).toContain('const seq = ++loadMoreSeq');
		expect(source).toContain('const pageQuery = lastQuery');
		expect(source).toContain('const pageMode = filterMode');
		expect(source).toContain('const generation = searchGeneration');
		expect(source).toContain('const isCurrentLoadMore = () =>');
		expect(source).toContain('seq === loadMoreSeq');
		expect(source).toContain('searchGeneration === generation');
		expect(source).toContain('lastQuery === pageQuery');
		expect(source).toContain('filterMode === pageMode');
		expect(source).toContain('const next = await api.searchTidal(pageQuery, SEARCH_PAGE_SIZE, undefined, tidalOffset)');
		expect(source).toContain('if (!isCurrentLoadMore()) return');
		expect(source).toContain('searchTidalPlaylists(pageQuery, undefined');
		expect(source).toContain('searchSpotifyPlaylists(pageQuery, SEARCH_PAGE_SIZE');
		expect(source).toContain('searchSpotifyTracks(pageQuery, SEARCH_PAGE_SIZE');
		expect(source).toContain('searchSpotifyAlbums(pageQuery, SEARCH_PAGE_SIZE');
		expect(source).toContain('if (seq === loadMoreSeq) loadingMore = false');
	});

	test('search page routes artist artwork through the shared fallback component', () => {
		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(source).toContain('<ArtworkImage');
		expect(source).not.toContain('failedArtistImages');
		expect(source).not.toContain('topArtistImageFailed');
		expect(source).not.toContain('function artworkSrc(');
		expect(source).not.toContain('upscaleTidalArtwork');
		expect(source).not.toMatch(/<img[^\n]*(artwork_url|photo_url|picture_url|image_url|cover_url|thumbnail_url)/);
		expect(source).not.toContain('background-image:url');
	});

	test('search page normalizes TIDAL search tracks before radio actions', () => {
		expect(source).toContain('const toPlayable = tidalSearchTrackToPlayable;');
		expect(source).toContain('void startTidalSongRadio(toPlayable(track))');
		expect(source).not.toContain('void startTidalSongRadio(track)');
	});

	test('search track menus suppress native and row events', () => {
		expect(source).toContain('function openTidalTrackContextMenu(event: MouseEvent, track: TidalSearchTrack)');
		expect(source).toContain('event.preventDefault()');
		expect(source).toContain('event.stopPropagation()');
		expect(source).toContain('openContextMenu(event, trackContextMenu(track))');
		expect(source).toContain('oncontextmenu={(e) => openTidalTrackContextMenu(e, track)}');
		expect(source).toContain('onclick={(e) => openTidalTrackContextMenu(e, track)}');
	});
});
