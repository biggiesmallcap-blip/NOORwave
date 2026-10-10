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
		expect(source).toContain("import { PRIMARY_SEARCH_DEBOUNCE_MS, SECONDARY_PROVIDER_DELAY_MS } from '$lib/search/search_timing'");
		expect(source).toContain('const INITIAL_SEARCH_PAGE_SIZE = 12');
		expect(source).toContain('const SECONDARY_SEARCH_PAGE_SIZE = 8');
		expect(source).toContain('const PRIMARY_TIDAL_SEARCH_TIMEOUT_MS = 8000');
		expect(source).toContain('const SECONDARY_PROVIDER_TIMEOUT_MS = 2500');
		expect(source).toContain('const SPOTIFY_PLAYLIST_SEARCH_TIMEOUT_MS = 8000');
		expect(source).toContain('let searchGeneration = $state(0)');
		expect(source).toContain('let loadMoreSeq = 0');
		expect(source).toContain('}, PRIMARY_SEARCH_DEBOUNCE_MS)');
		expect(source).toContain('loadMoreSeq += 1');
		expect(source).toContain('loadingMore = false');
		expect(source).toContain('const localPromise = cachedApi.search(q, INITIAL_SEARCH_PAGE_SIZE, signal)');
		expect(source).toContain('api.searchTidal(q, INITIAL_SEARCH_PAGE_SIZE, signal, 0, PRIMARY_TIDAL_SEARCH_TIMEOUT_MS)');
		expect(source).toContain('void localPromise.then((localResults) => {');
		expect(source).toContain('if (!isCurrentSearch(q, generation, signal)) return');
		expect(source).toContain('void tracksPromise.then((tidalResults) => {');
		expect(source).toContain('secondarySpotifyQueued = true');
		expect(source).toContain('secondarySpotifyTimer = scheduleSearchIdleTask(() => {');
		expect(source).toContain('}, SECONDARY_PROVIDER_DELAY_MS)');
		expect(source).toContain('loadingTidalPlaylists = true');
		expect(source).toContain('loadingSpotifyPlaylists = true');
		expect(source).toContain('timeoutMs: SECONDARY_PROVIDER_TIMEOUT_MS');
		expect(source).toContain('SPOTIFY_PLAYLIST_SEARCH_TIMEOUT_MS,');
		expect(source).toContain('void tidalPlaylistPromise.then((playlistResults) => {');
		expect(source).toContain('void spotifyPlaylistPromise.then((playlistResults) => {');
		expect(source).toContain('const primaryProviderSearchDone = $derived(');
		expect(source).toContain('const providerSearchDone = $derived(');
		expect(source).toContain('{:else if allProviderResultsEmpty && providerSearchDone}');
	});

	test('search URL query and Spotify playlist links preserve resolver flow', () => {
		// The href itself is built and behaviour-tested in $lib/search/search_results.
		expect(source).toContain("const urlQuery = new URLSearchParams(window.location.search).get('q')?.trim()");
		expect(source).toContain('query = urlQuery');
		expect(source).toContain('return buildSpotifyPlaylistHref(spotifyId, activeQueryText)');
		expect(source).toContain('href={spotifyPlaylistHref(playlist.spotifyId)}');
		expect(source).toContain('onSelect: () => void goto(spotifyPlaylistHref(playlist.spotifyId))');
		expect(source).not.toContain('href="/spotify-playlist/{playlist.spotifyId}"');
	});

	test('playlist provider pending state renders a visible loader', () => {
		expect(source).toContain('{#if showPlaylists && (playlistRailPending || visiblePlaylists.length > 0)}');
		expect(source).toContain('{#if playlistRailPending && visiblePlaylists.length === 0}');
		expect(source).toContain('class="album-card playlist-loading-card"');
		expect(source).toContain('.playlist-loading-art');
		expect(source).toContain('@keyframes playlist-loading-pulse');
	});

	test('spotify search stays scoped to playlists', () => {
		// Spotify exists here to source playlists; track/album/artist search
		// must not come back.
		expect(source).not.toContain('searchSpotifyTracks');
		expect(source).not.toContain('searchSpotifyAlbums');
		expect(source).not.toContain('spotifyTrackResults');
		expect(source).not.toContain('spotifyAlbumResults');
		expect(source).not.toContain('Spotify tracks');
		expect(source).not.toContain('Spotify albums');
	});

	test('playlist rail ranks all sources by relevance to the query', () => {
		// Ranking rules are behaviour-tested in $lib/search/search_results.test.ts.
		expect(source).toContain('rankPlaylists(filteredPlaylists, activeQueryText)');
		expect(source).not.toContain('function playlistRelevance(');
	});

	test('focused category filters prefetch one deeper page after the light initial batch', () => {
		// Which views prefetch is behaviour-tested in $lib/search/search_results.test.ts;
		// the page owns firing it once per query and mode.
		expect(source).toContain("let focusedFilterPrefetchKey = $state('')");
		expect(source).toContain("focusedFilterPrefetchKey = ''");
		expect(source).toContain('focusedViewNeedsPrefetch({');
		expect(source).toContain('if (!focusedFilterNeedsPrefetch) return');
		expect(source).toContain('const key = `${searchGeneration}:${lastQuery}:${filterMode}`');
		expect(source).toContain('if (focusedFilterPrefetchKey === key) return');
		expect(source).toContain('focusedFilterPrefetchKey = key');
		expect(source).toContain('void loadMore()');
	});

	test('all-results view caps each section preview while category views keep full lists', () => {
		// The caps are behaviour-tested in $lib/search/search_results.test.ts.
		expect(source).toContain("previewForView(filterMode, sortedArtists, 'artists')");
		expect(source).toContain("previewForView(filterMode, sortedAlbums, 'albums')");
		expect(source).toContain("previewForView(filterMode, sortedTracks, 'tracks')");
		expect(source).toContain("previewForView(filterMode, rankedPlaylists, 'playlists')");
		expect(source).toContain('{#each visibleArtists as artist (artist.tidal_id)}');
		expect(source).toContain('{#each visibleAlbums as album (album.tidal_id)}');
		expect(source).toContain('{#each visibleTracks as track, idx (track.tidal_id)}');
		expect(source).toContain('{#each visiblePlaylists as entry (entry.key)}');
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
		expect(source).toContain('PRIMARY_TIDAL_SEARCH_TIMEOUT_MS,');
		expect(source).toContain('if (!isCurrentLoadMore()) return');
		expect(source).toContain('searchTidalPlaylists(pageQuery, undefined');
		expect(source).toContain('searchSpotifyPlaylists(pageQuery, LOAD_MORE_PAGE_SIZE');
		expect(source).toContain('if (seq === loadMoreSeq) loadingMore = false');
	});

	test('typing a new query keeps current results through debounce and invalidates side-loads', () => {
		expect(source).toContain("let activeQuery = $state('')");
		expect(source).toContain('const activeQueryText = $derived(activeQuery.trim())');
		expect(source).toContain('function clearSecondarySpotifyTimer()');
		expect(source).toContain('function clearDiscoveryPanelLoad()');
		expect(source).toContain('function clearVisibleSearchResults()');
		expect(source).toContain('function invalidateSearchSideLoads()');
		expect(source).toContain('invalidateSearchSideLoads()');
		expect(source).toContain('clearSecondarySpotifyTimer()');
		expect(source).toContain('clearDiscoveryPanelLoad()');
		expect(source).toContain('if (!query.trim()) {');
		expect(source).toContain("activeQuery = ''");
		expect(source).toContain('debounceTimer = setTimeout(async () => {');
		// The previous results stay, dimmed, until the new query's first results.
		expect(source).toContain("const q = query.trim()\n      activeQuery = q\n      loading = true\n      resultsStale = results !== null || audioResults !== null\n      if (!resultsStale) clearVisibleSearchResults()");
		expect(source).toContain('function takeFreshResults(q: string)');
		expect(source).toContain('LOCAL_RESULTS_HOLD_MS');
		expect(source).toContain("lastQuery = ''");
		expect(source).toContain('vibeTrack = null');
		expect(source).toContain('underratedTracks = null');
		expect(source).toContain('discoveryLoadSeq += 1');
	});

	test('navigation abort clears visible search activity', () => {
		expect(source).toContain('function resetSearchActivity()');
		expect(source).toContain('loading = false');
		expect(source).toContain('loadingMore = false');
		expect(source).toContain('resetProviderLoading()');
		expect(source).toContain('beforeNavigate(() => {');
		expect(source).toContain('abortController?.abort()');
		expect(source).toContain('clearSecondarySpotifyTimer()');
		expect(source).toContain('resetSearchActivity()');
	});

	test('visible search results stay keyed to the committed query while typing the next one', () => {
		expect(source).toContain('if (!activeQueryText)');
		expect(source).toContain('const q = activeQueryText.toLowerCase()');
		expect(source).toContain('if (!results || !activeQueryText) return null');
		expect(source).toContain('{#if results && activeQueryText}');
		expect(source).toContain('No results for "{activeQueryText}"');
		expect(source).toContain(`No {filterMode === 'library' ? 'library' : filterMode} matches for "{activeQueryText}"`);
	});

	test('search discovery panels ignore stale top-result responses', () => {
		expect(source).toContain('let discoveryLoadSeq = 0');
		expect(source).toContain('const seq = ++discoveryLoadSeq');
		expect(source).toContain('const isCurrentDiscoveryLoad = () => seq === discoveryLoadSeq && topResult === top');
		expect(source).toContain('cancelDiscoveryPanelLoad = scheduleSearchIdleTask(() => {');
		expect(source).toContain('if (!isCurrentDiscoveryLoad()) return');
		expect(source).toContain('vibeTrack = r.tracks');
		expect(source).toContain('underratedTracks = r.tracks');
		expect(source).not.toContain('then(r => { vibeTrack = r.tracks })');
		expect(source).not.toContain('then(r => { underratedTracks = r.tracks })');
	});

	test('search page routes artist artwork through the shared fallback component', () => {
		expect(source).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte'");
		expect(source).toContain('<ArtworkImage');
		expect(source).toContain('loading="eager"');
		expect(source).toContain('fetchPriority="high"');
		expect(source).toContain('loading="lazy"');
		expect(source).toContain('decoding="async"');
		expect(source).toContain('artistFallbackArtwork(artist)');
		expect(source).not.toContain('failedArtistImages');
		expect(source).not.toContain('topArtistImageFailed');
		expect(source).not.toContain('function artworkSrc(');
		expect(source).not.toContain('upscaleTidalArtwork');
		expect(source).not.toMatch(/<img[^\n]*(artwork_url|photo_url|picture_url|image_url|cover_url|thumbnail_url)/);
		expect(source).not.toContain('background-image:url');
	});

	test('search results never fetch a full artist discography just to decorate artwork', () => {
		expect(source).not.toContain('cachedApi.getArtistDiscography');
		expect(source).not.toContain('loadArtistDiscographyArtwork');
	});

	test('search page normalizes TIDAL search tracks before radio actions', () => {
		expect(source).toContain('const toPlayable = tidalSearchTrackToPlayable;');
		expect(source).toContain('void startTidalSongRadio(toPlayable(track))');
		expect(source).not.toContain('void startTidalSongRadio(track)');
	});

	test('search track menus suppress native and row events', () => {
		expect(source).toContain('function openSearchContextMenu(event: MouseEvent, items: MenuItem[], title?: string)');
		expect(source).toContain('function openTidalTrackContextMenu(event: MouseEvent, track: TidalSearchTrack)');
		expect(source).toContain('event.preventDefault()');
		expect(source).toContain('event.stopPropagation()');
		expect(source).toContain('openContextMenu(event, items, title)');
		expect(source).toContain('openSearchContextMenu(event, trackContextMenu(track))');
		expect(source).toContain('oncontextmenu={(e) => openTidalTrackContextMenu(e, track)}');
		expect(source).toContain('onclick={(e) => openTidalTrackContextMenu(e, track)}');
	});

	test('search result card menus share the app-owned context-menu path', () => {
		expect(source).toContain('function audioTrackMenu(track: AudioSearchResult): MenuItem[]');
		expect(source).toContain('function libraryBasicTrackMenu(track: BasicTrack): MenuItem[]');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, audioTrackMenu(track))}');
		expect(source).toContain('onclick={(e) => openSearchContextMenu(e, audioTrackMenu(track))}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, artistMenuItems(artist))}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, albumMenuItems(album))}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, localPlaylistMenuItems(playlist), playlist.name)}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, tidalPlaylistMenuItems(playlist), playlist.title)}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, spotifyPlaylistMenuItems(playlist), playlist.title ?? \'Spotify playlist\')}');
		expect(source).toContain('oncontextmenu={(e) => openSearchContextMenu(e, libraryBasicTrackMenu(track))}');
	});
});
