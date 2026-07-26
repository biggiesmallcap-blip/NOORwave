<script lang="ts" module>
	import type { Track as CachedTrack } from '$lib/api/client';

	type CachedHomeAlbumCard = {
		id: number;
		title: string;
		artist_id: number | null;
		artist_name: string | null;
		artwork_url: string | null;
	};

	const HOME_PANEL_CACHE_REFRESH_MS = 5 * 60 * 1000;
	const homePanelCandidateCache = {
		recentTracks: [] as CachedTrack[],
		randomTracks: [] as CachedTrack[],
		randomAlbums: [] as CachedHomeAlbumCard[],
		randomRequestKey: '',
		suggestionTracks: [] as CachedTrack[],
		suggestionServerTracks: [] as CachedTrack[],
		suggestionRequestKey: '',
	};
</script>

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import type { Snapshot } from './$types';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import {
		tracks, albums, artists as artistsStore, isLoading, isLoadingMore, totalTracks, totalAlbums,
		sortBy, sortDir, viewMode, searchQuery,
		loadTracks, loadAlbums,
		selectedTrackIds, selectedAlbumIds,
		lastSelectedTrackId, lastSelectedAlbumId,
		selectTrackIds, selectAlbumIds, clearSelection,
	} from '$lib/stores/library';
	import { formatTrackDuration, formatDateShort, getQualityClass } from '$lib/utils/format';
	import { api, type Album, type Artist, type AudioSearchResult, type Genre, type Playlist, type Track } from '$lib/api/client';
	import { cachedApi, invalidateLibraryCaches } from '$lib/cache/api_queries';
	import {
		currentTrack,
		isPlaying,
		playTrackNow,
		playTracksInContext,
		playLibrary,
		addTrackToQueue,
		playTrackNext,
		shuffleMode,
		playAlbum as playAlbumNow,
		playArtist as playArtistNow,
		shuffleArtist as shuffleArtistNow
	} from '$lib/stores/player';
	import SelectionBar from '$lib/components/ui/SelectionBar.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import LibraryHero from '$lib/components/LibraryHero.svelte';
	import ArtistCarousel from '$lib/components/ArtistCarousel.svelte';
	import AlbumCarousel from '$lib/components/AlbumCarousel.svelte';
	import AlbumDetailPopup from '$lib/components/AlbumDetailPopup.svelte';
	import { lazyTidalArt, composeTidalArtQuery, peekTidalArt } from '$lib/actions/lazy-tidal-art';
	import { portal } from '$lib/actions/portal';
	import { openContextMenu, openMenuAtElement, type MenuItem } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';
	import { parseQuery } from '$lib/search/query_parser';
	import { buildAudioParams, hasAnyFilter } from '$lib/search/audio_params';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import { goto } from '$app/navigation';
	import { showToast } from '$lib/stores/toast';
	import { wsMessages } from '$lib/api/ws';

	function buildAddToPlaylistSubmenu(
		getTrackIds: () => Promise<number[]>,
	): MenuItem[] {
		return [...playlists]
			.sort((a, b) => {
				if (a.is_favorite !== b.is_favorite) return a.is_favorite ? -1 : 1;
				return a.name.localeCompare(b.name);
			})
			.map((playlist) => ({
				label: playlist.name,
				icon: playlist.is_favorite ? '♥' : '♩',
				onSelect: async () => {
					const trackIds = await getTrackIds();
					if (!trackIds.length) return;
					const { added } = await api.addTracksToPlaylist(playlist.id, trackIds);
					showToast(`Added ${added} track${added !== 1 ? 's' : ''} to ${playlist.name}`, 'success');
				},
			}));
	}

	function handleHomeArtistContextMenu(e: MouseEvent, artistId: number) {
		const card = artists.find(a => a.id === artistId);
		const artist = card ?? { id: artistId, tidal_id: null, name: '' };
		openContextMenu(e, buildArtistMenu(artist, {
			isLocal: true,
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
				const { tracks: t } = await cachedApi.getArtistTracks(artistId);
				return t.map(tr => tr.id);
			}),
		}), artist.name);
	}

	function handleHomeAlbumContextMenu(e: MouseEvent, albumId: number, fallback?: HomeAlbumCard) {
		const card = recentAlbums.find(a => a.id === albumId) ?? fallback;
		const album = $albums.find(a => a.id === albumId) ?? (card ? albumFromHomeCard(card) : { id: albumId, title: '' });
		openContextMenu(e, buildAlbumMenu(album, {
			isLocal: true,
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
				const { tracks: t } = await cachedApi.getAlbumTracks(albumId);
				return t.map(tr => tr.id);
			}),
		}), album.title);
	}

	// Track-row artist/album cells are links; right-clicking one opens the
	// shared artist/album menu (not the track menu the row otherwise shows).
	function handleRowArtistContextMenu(e: MouseEvent, track: Track) {
		e.preventDefault();
		e.stopPropagation();
		if (track.artist_id == null) return;
		openContextMenu(e, buildArtistMenu({
			id: track.artist_id,
			tidal_id: track.artist_tidal_id ?? null,
			name: track.artist_name ?? '',
			in_library: true,
		}, {
			isLocal: true,
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
				const { tracks: t } = await cachedApi.getArtistTracks(track.artist_id!);
				return t.map(tr => tr.id);
			}),
		}), track.artist_name ?? '');
	}

	function handleRowAlbumContextMenu(e: MouseEvent, track: Track) {
		e.preventDefault();
		e.stopPropagation();
		if (track.album_id == null) return;
		openContextMenu(e, buildAlbumMenu({
			id: track.album_id,
			title: track.album_title ?? '',
			artist_id: track.artist_id ?? null,
			artist_name: track.artist_name ?? null,
			in_library: true,
		}, {
			isLocal: true,
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
				const { tracks: t } = await cachedApi.getAlbumTracks(track.album_id!);
				return t.map(tr => tr.id);
			}),
		}), track.album_title ?? '');
	}

	const PAGE_SIZE = 100;
	// Depth of the random sample Shuffle pulls from a filtered view. Matches the
	// whole-library Shuffle queue depth; automix extends past it.
	const SHUFFLE_SAMPLE_SIZE = 200;
	const RECENT_TRACK_LIMIT = 10;
	const HOME_MURAL_ITEM_LIMIT = 12;
	const ALL_SEARCH_ARTIST_PREVIEW_LIMIT = 12;
	const ALL_SEARCH_ALBUM_PREVIEW_LIMIT = 12;
	const ALL_SEARCH_TRACK_PREVIEW_LIMIT = 10;

	let activeTab = $state<'all' | 'tracks' | 'liked' | 'albums' | 'artists'>('all');
	// Only render the second toolbar row when the tab actually contributes
	// controls to it, so tabs without any never leave a gap behind.
	const hasToolbarActions = $derived(
		activeTab === 'tracks' || activeTab === 'liked' || activeTab === 'albums'
	);
	let playlists = $state<Playlist[]>([]);
	let genres = $state<Genre[]>([]);
	let selectedPlaylistId = $state('');
	let selectedGenreId = $state('');
	let batchMessage = $state<string | null>(null);
	let batchError = $state<string | null>(null);
	let batchBusy = $state<'playlist' | 'genre' | 'delete' | null>(null);
	// Removed items held for a real Undo: batchDelete removes TIDAL favorites, and
	// Undo re-adds them via setTrackFavorite/setAlbumFavorite (both tracks and
	// albums), then reloads. Cleared when the undo window lapses.
	let pendingUndo = $state<{ tracks: Track[]; albums: Album[] }>({ tracks: [], albums: [] });
	let undoBusy = $state(false);
	const UNDO_WINDOW_MS = 8000;
	let albumActionBusyId = $state<number | null>(null);
	let activeTrackMenuId = $state<number | null>(null);
	let searchBusy = $state(false);
	let searchError = $state<string | null>(null);
	let searchResults = $state<{ tracks: Track[]; albums: Album[]; artists: Artist[] }>({ tracks: [], albums: [], artists: [] });
	// Full matching-set size for filtered (audio) searches, so the capped
	// display can say "top 50 of N" instead of looking like only 50 exist.
	let searchTotal = $state<number | null>(null);
	let searchUnmatchedGenres = $state<string[]>([]);
	let searchLoadingMore = $state(false);
	let searchTimer: ReturnType<typeof setTimeout> | null = null;
	let infiniteSentinel = $state<HTMLDivElement | null>(null);
	let infiniteObserver: IntersectionObserver | null = null;
	let undoTimer: ReturnType<typeof setTimeout> | null = null;
	let artists = $state<Artist[]>([]);
	let artistsLoading = $state(false);
	let artistsLoadingMore = $state(false);
	// The artists endpoint returns no total, so we page until a short page tells
	// us we've hit the end. `artistsExhausted` then stops further fetches.
	let artistsExhausted = $state(false);
	let recentTracks = $state<Track[]>(homePanelCandidateCache.recentTracks);

	// Keyboard cursor for track list
	let cursorIndex = $state(-1);

	// Windowed rendering for the track list. Infinite scroll can accumulate
	// tens of thousands of rows; only the rows near the viewport are mounted,
	// with spacer elements preserving the true scroll height (so the
	// IntersectionObserver sentinel and snapshot scroll restore keep working).
	// The app scrolls `main.workspace` (see $lib/navigation/scroll.ts), so the
	// window is computed against that container, not the window.
	const VLIST_ROW_BUFFER = 12;
	let trackRowsEl = $state<HTMLDivElement | null>(null);
	// Row pitch (height + inter-row spacing), measured from rendered rows.
	let trackRowPitch = $state(34);
	let vlistStart = $state(0);
	let vlistEnd = $state(80);
	let vlistRaf = 0;

	// The workspace is the scroll container on desktop, but the mobile layout
	// sets it to `overflow: visible` and lets the document scroll instead.
	function trackScroller(): HTMLElement | null {
		const workspace = document.querySelector<HTMLElement>('main.workspace');
		if (workspace && /(auto|scroll)/.test(getComputedStyle(workspace).overflowY)) return workspace;
		return null;
	}

	function updateTrackWindow() {
		if (!trackRowsEl) return;
		const scroller = trackScroller();
		const viewTop = scroller ? scroller.getBoundingClientRect().top : 0;
		const viewHeight = scroller ? scroller.clientHeight : window.innerHeight;
		const rowsTop = trackRowsEl.getBoundingClientRect().top - viewTop;
		const total = visibleTracks.length;
		const firstVisible = Math.floor(-rowsTop / trackRowPitch);
		const visibleCount = Math.ceil(viewHeight / trackRowPitch);
		const start = Math.max(0, Math.min(firstVisible - VLIST_ROW_BUFFER, total));
		const end = Math.max(start, Math.min(total, firstVisible + visibleCount + VLIST_ROW_BUFFER));
		if (start !== vlistStart || end !== vlistEnd) {
			vlistStart = start;
			vlistEnd = end;
		}
	}

	function scheduleTrackWindowUpdate() {
		if (vlistRaf) return;
		vlistRaf = requestAnimationFrame(() => {
			vlistRaf = 0;
			updateTrackWindow();
		});
	}

	// Decade filter for albums tab. `decadeChips` is fetched server-side so the
	// chips (and the selection) cover the whole library, not just the album pages
	// already loaded on the client. Selecting a decade re-queries the server for a
	// complete set instead of narrowing the loaded page.
	let activeDecade = $state<number | null>(null);
	let decadeChips = $state<number[]>([]);

	// Album ordering lives in its own state, separate from the track-list sort
	// ($sortBy/$sortDir), because albums sort on different columns (title/artist/
	// year) than tracks and the two tabs must not clobber each other's choice.
	type AlbumSortField = 'title' | 'artist' | 'year';
	let albumSortField = $state<AlbumSortField>('title');
	let albumSortDir = $state<'asc' | 'desc'>('asc');
	const ALBUM_SORT_LABELS: Record<AlbumSortField, string> = {
		title: 'Title',
		artist: 'Artist',
		year: 'Year',
	};

	// Track detail panel
	let expandedTrackId = $state<number | null>(null);
	let detailTrack = $state<Track | null>(null);
	let detailAlbumTracks = $state<Track[]>([]);
	let detailLoading = $state(false);

	// Album detail panel
	let expandedAlbumId = $state<number | null>(null);
	let detailAlbum = $state<Album | null>(null);
	let detailAlbumTracksList = $state<Track[]>([]);
	let detailAlbumLoading = $state(false);

	// Visible track columns
	let showPlaysColumn = $state(true);
	let showDateColumn = $state(true);
	let showQualityColumn = $state(true);
	let showFavColumn = $state(true);
	let showBpmColumn = $state(false);
	let showKeyColumn = $state(false);
	let showEnergyColumn = $state(false);
	let showDanceColumn = $state(false);

	// Reactive grid template - must match the order of cells in .track-header and .track-row.
	// Cells that get conditionally removed via {#if showXColumn} drop their column track here too,
	// so header and row stay aligned.
	// All non-fr columns must be explicit px - 'auto' sizes independently per row-grid,
	// causing header/data drift when badge content differs from header text.
	let trackGridColumns = $derived.by(() => {
		const cols: string[] = ['40px', 'minmax(0, 2fr)', 'minmax(0, 1.5fr)', 'minmax(0, 1.5fr)']; // # title artist album
		if (showQualityColumn) cols.push('88px');
		if (showPlaysColumn) cols.push('54px');
		if (showDateColumn) cols.push('88px', '94px'); // date_added + last_played
		if (showBpmColumn) cols.push('60px');
		if (showKeyColumn) cols.push('50px');
		if (showEnergyColumn) cols.push('60px');
		if (showDanceColumn) cols.push('60px');
		cols.push('68px', '56px'); // duration, actions
		return cols.join(' ');
	});


	onMount(() => {
		// Load only if the persistent stores are empty. On a back-nav the stores
		// still hold every page the user scrolled through; reloading page 1 here
		// would discard that depth and strand the snapshot's scroll restore at the
		// bottom of the first page. A fresh visit starts empty and loads normally.
		if (get(albums).length === 0) void loadAlbums(albumSortField, albumSortDir);
		if (get(tracks).length === 0) void loadTracks();
		void loadBatchMeta();
		void loadRecentTracks();
		void loadDecadeChips();
		const unsubscribeWs = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;
			if (latest.type === 'listen_history_updated') {
				void loadRecentTracks();
			}
		});
		return () => {
			unsubscribeWs();
			if (searchTimer) clearTimeout(searchTimer);
			infiniteObserver?.disconnect();
			if (undoTimer) clearTimeout(undoTimer);
		};
	});

	async function loadRecentTracks() {
		try {
			// Sourced from play history (listen_history), not the favorited library,
			// so externally-sourced plays (radio, discover) show up here too.
			const data = await cachedApi.getHistory(RECENT_TRACK_LIMIT, 0);
			recentTracks = data.tracks.slice(0, RECENT_TRACK_LIMIT);
			homePanelCandidateCache.recentTracks = recentTracks;
			if (recentTracks.length === 0) {
				suggestionCandidateTracks = [];
				suggestionServerTracks = [];
				suggestionCandidateRequestKey = '';
				homePanelCandidateCache.suggestionTracks = [];
				homePanelCandidateCache.suggestionServerTracks = [];
				homePanelCandidateCache.suggestionRequestKey = '';
			}
		} catch (error) {
			console.error('Failed to load recent tracks:', error);
		}
	}

	async function loadDecadeChips() {
		try {
			const { decades } = await cachedApi.getAlbumDecades();
			decadeChips = Array.isArray(decades) ? decades : [];
		} catch (error) {
			// Older server builds lack /api/albums/decades; fall back to decades
			// derived from the albums already loaded (see decadeOptions).
			decadeChips = [];
		}
	}

	// Selecting a decade re-queries the server for the complete set (server-side
	// filter), rather than narrowing only the album page already loaded. Clicking
	// the active decade again clears the filter.
	function selectDecade(decade: number | null) {
		const next = decade != null && activeDecade === decade ? null : decade;
		if (next === activeDecade) return;
		activeDecade = next;
		clearSelection();
		if (!$searchQuery.trim()) {
			void loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, 0, next);
		}
	}

	// Change album ordering. Re-picking the active field flips direction;
	// switching field resets to a sensible default (newest-first for Year).
	function setAlbumSort(field: AlbumSortField) {
		if (albumSortField === field) {
			albumSortDir = albumSortDir === 'asc' ? 'desc' : 'asc';
		} else {
			albumSortField = field;
			albumSortDir = field === 'year' ? 'desc' : 'asc';
		}
		clearSelection();
		if (!$searchQuery.trim()) {
			void loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, 0, activeDecade);
		}
	}

	async function loadBatchMeta() {
		try {
			const [playlistData, genreData] = await Promise.all([
				cachedApi.getPlaylists(),
				cachedApi.getGenres(),
			]);
			playlists = playlistData.playlists.filter((playlist) => !playlist.is_smart);
			selectedPlaylistId = playlists[0] ? String(playlists[0].id) : '';
			genres = flattenGenres(genreData.genres);
			selectedGenreId = genres[0] ? String(genres[0].id) : '';
		} catch (error) {
			console.error('Failed to load batch metadata:', error);
		}
	}

	function handleSort(field: string) {
		if ($sortBy === field) {
			sortDir.update(d => d === 'asc' ? 'desc' : 'asc');
		} else {
			sortBy.set(field);
			sortDir.set('asc');
		}
		if ($searchQuery.trim()) return;
		// Albums have their own ordering control (setAlbumSort); handleSort only
		// drives the track/liked list column headers.
		if (activeTab === 'tracks') {
			loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, false);
		} else if (activeTab === 'liked') {
			loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, true);
		}
		clearSelection();
	}

	function switchTab(tab: 'all' | 'tracks' | 'liked' | 'albums' | 'artists') {
		activeTab = tab;
		expandedTrackId = null;
		expandedAlbumId = null;
		detailTrack = null;
		detailAlbum = null;
		if (!$searchQuery.trim()) {
			// Tracks and Liked share the $tracks store but represent different result sets,
			// so always refetch from offset 0 when entering either - never reuse stale rows.
			if (tab === 'tracks') loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, false);
			if (tab === 'liked') loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, true);
			if (tab === 'albums') loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, 0, activeDecade);
		}
		if (tab === 'artists' && artists.length === 0) void loadArtists();
		clearSelection();
	}

	async function loadArtists() {
		artistsLoading = true;
		artistsExhausted = false;
		try {
			// First page. Subsequent pages append via loadMoreArtists on scroll, so
			// the browse view paginates like Albums/Tracks instead of capping out.
			// When the user types a query, the search effect calls api.search()
			// server-side and shows searchResults.artists (FTS).
			const data = await cachedApi.getArtists('name', 'asc', PAGE_SIZE, 0);
			artists = data.artists;
			if (data.artists.length < PAGE_SIZE) artistsExhausted = true;
		} catch (err) {
			console.error('Failed to load artists:', err);
		} finally {
			artistsLoading = false;
		}
	}

	// Re-page artists up to `targetCount` in one restore pass (back-nav), so a
	// deep scroll position is reachable. Pages sequentially like loadMoreArtists
	// and fills the grid progressively; stops when the source is exhausted.
	async function loadArtistsUpTo(targetCount: number) {
		artistsLoading = true;
		artistsExhausted = false;
		try {
			let acc: Artist[] = [];
			for (;;) {
				const data = await cachedApi.getArtists('name', 'asc', PAGE_SIZE, acc.length);
				const seen = new Set(acc.map((a) => a.id));
				acc = [...acc, ...data.artists.filter((a) => !seen.has(a.id))];
				artists = acc;
				if (data.artists.length < PAGE_SIZE) {
					artistsExhausted = true;
					break;
				}
				if (acc.length >= targetCount) break;
			}
		} catch (err) {
			console.error('Failed to restore artists:', err);
		} finally {
			artistsLoading = false;
		}
	}

	async function loadMoreArtists() {
		if (artistsExhausted || artistsLoading || artistsLoadingMore) return;
		artistsLoadingMore = true;
		try {
			const data = await cachedApi.getArtists('name', 'asc', PAGE_SIZE, artists.length);
			// Dedupe by id so a shifted page can't create duplicate {#each} keys.
			const seen = new Set(artists.map((a) => a.id));
			const fresh = data.artists.filter((a) => !seen.has(a.id));
			artists = [...artists, ...fresh];
			if (data.artists.length < PAGE_SIZE) artistsExhausted = true;
		} catch (err) {
			console.error('Failed to load more artists:', err);
		} finally {
			artistsLoadingMore = false;
		}
	}

	// Clicking a track plays it in the context of the list the user is looking at:
	// the visible rows become the queue, starting at the clicked track (TIDAL /
	// Spotify behavior) instead of playing one orphan track and letting automix
	// improvise. `visibleTracks` already reflects the active tab, sort, and search.
	async function playTrack(track: typeof $tracks[0]) {
		await playTracksInContext(visibleTracks.map((t) => t.id), track.id);
	}

	// "Play" / "Shuffle" for the current track view. In search mode the visible
	// results are the context; otherwise pull the full sorted/liked list from the
	// server so the queue isn't limited to the rows scrolled into view.
	async function playTrackView(shuffle = false) {
		batchError = null;
		batchMessage = null;
		const trimmed = $searchQuery.trim();
		if (trimmed) {
			// A filtered/searched Shuffle must randomize across the FULL matching
			// set, not just the ~50 rows the search endpoints return for display.
			// Re-query the audio search with random ordering and a deeper sample so
			// "genre:dnb", "instrumental:true", or a plain-text query all shuffle
			// through everything that matches (liked-only on the Liked tab).
			if (shuffle) {
				try {
					const params = buildAudioParams(parseQuery(trimmed));
					const audio = await api.searchAudio({
						...params,
						shuffle: true,
						liked_only: activeTab === 'liked',
						limit: SHUFFLE_SAMPLE_SIZE,
					});
					const ids = audio.tracks.map((t) => t.id);
					if (ids.length === 0) {
						batchError = 'No tracks to play in the current view.';
						return;
					}
					await playTracksInContext(ids, undefined, { shuffle: true });
					return;
				} catch (error) {
					batchError = `Shuffle failed: ${error}`;
					return;
				}
			}
			const ids = visibleTracks.map((t) => t.id);
			if (ids.length === 0) {
				batchError = 'No tracks to play in the current view.';
				return;
			}
			await playTracksInContext(ids, undefined, { shuffle });
			return;
		}
		await playLibrary({
			sortBy: $sortBy,
			sortDir: $sortDir,
			likedOnly: activeTab === 'liked',
			shuffle,
		});
	}

	async function queueTrack(track: typeof $tracks[0], event: MouseEvent) {
		event.stopPropagation();
		await addTrackToQueue(track.id);
	}

	async function playAlbum(albumId: number, event: MouseEvent) {
		event.stopPropagation();
		albumActionBusyId = albumId;
		batchError = null;
		batchMessage = null;
		try {
			const data = await cachedApi.getAlbumTracks(albumId);
			const total = data.tracks.length + (data.tidal_tracks?.length ?? 0);
			if (total === 0) {
				throw new Error('No synced tracks found for this album yet.');
			}
			// Route through the store so a partially-owned album queues the WHOLE
			// album (owned + TIDAL-only rows) in track order, not just the synced
			// subset - queueing only the owned rows left a short queue that automix
			// padded with unrelated tracks. Pass the data we already fetched so the
			// store skips a second (live-TIDAL) round trip.
			await playAlbumNow(albumId, undefined, data);
			batchMessage = `Playing album (${total} track${total === 1 ? '' : 's'}).`;
		} catch (error) {
			batchError = `Failed to play album: ${error}`;
		} finally {
			albumActionBusyId = null;
		}
	}

	async function queueAlbum(albumId: number, event: MouseEvent) {
		event.stopPropagation();
		albumActionBusyId = albumId;
		batchError = null;
		batchMessage = null;
		try {
			const data = await cachedApi.getAlbumTracks(albumId);
			if (data.tracks.length === 0) {
				throw new Error('No synced tracks found for this album yet.');
			}
			for (const track of data.tracks) {
				await addTrackToQueue(track.id);
			}
			batchMessage = `Queued ${data.tracks.length} track${data.tracks.length === 1 ? '' : 's'} from the album.`;
		} catch (error) {
			batchError = `Failed to queue album: ${error}`;
		} finally {
			albumActionBusyId = null;
		}
	}

	async function openTrackDetail(track: Track) {
		if (expandedTrackId === track.id) {
			expandedTrackId = null;
			detailTrack = null;
			detailAlbumTracks = [];
			return;
		}
		expandedTrackId = track.id;
		detailTrack = track;
		detailLoading = false;

		if (track.album_id) {
			detailLoading = true;
			try {
				const data = await cachedApi.getAlbumTracks(track.album_id);
				detailAlbumTracks = data.tracks;
			} catch (err) {
				console.error('Failed to load album tracks:', err);
				detailAlbumTracks = [];
			} finally {
				detailLoading = false;
			}
		}
	}

	async function openAlbumDetail(album: Album) {
		if (expandedAlbumId === album.id) {
			expandedAlbumId = null;
			detailAlbum = null;
			detailAlbumTracksList = [];
			return;
		}
		expandedAlbumId = album.id;
		detailAlbum = album;
		detailAlbumLoading = true;
		try {
			const data = await cachedApi.getAlbumTracks(album.id);
			detailAlbumTracksList = data.tracks;
		} catch (err) {
			console.error('Failed to load album tracks:', err);
			detailAlbumTracksList = [];
		} finally {
			detailAlbumLoading = false;
		}
	}

	function flattenGenres(nodes: Genre[], prefix: string[] = []): Genre[] {
		return nodes.flatMap((node) => {
			const path = [...prefix, node.name];
			const label = path.join(' > ');
			return [{ ...node, name: label }, ...flattenGenres(node.children ?? [], path)];
		});
	}

	function formatSearchSummary(artistCount: number, albumCount: number, trackCount: number) {
		const parts: string[] = [];
		if (artistCount > 0) parts.push(`${artistCount} artist match${artistCount === 1 ? '' : 'es'}`);
		if (albumCount > 0) parts.push(`${albumCount} album match${albumCount === 1 ? '' : 'es'}`);
		if (trackCount > 0) parts.push(`${trackCount} track match${trackCount === 1 ? '' : 'es'}`);
		return parts.length ? parts.join(', ') : 'No library matches';
	}

	function selectionRange<T extends { id: number }>(
		items: T[],
		clickedId: number,
		lastId: number | null
	): number[] {
		if (lastId === null) return [clickedId];
		const currentIndex = items.findIndex((item) => item.id === clickedId);
		const lastIndex = items.findIndex((item) => item.id === lastId);
		if (currentIndex === -1 || lastIndex === -1) return [clickedId];
		const [start, end] = currentIndex < lastIndex ? [currentIndex, lastIndex] : [lastIndex, currentIndex];
		return items.slice(start, end + 1).map((item) => item.id);
	}

	function updateTrackSelection(trackId: number, additive = false, range = false) {
		const ids = range ? selectionRange(visibleTracks, trackId, $lastSelectedTrackId) : [trackId];
		selectTrackIds(ids, additive);
		batchMessage = null;
		batchError = null;
	}

	function updateAlbumSelection(albumId: number, additive = false, range = false) {
		const ids = range ? selectionRange(visibleAlbums, albumId, $lastSelectedAlbumId) : [albumId];
		selectAlbumIds(ids, additive);
		batchMessage = null;
		batchError = null;
	}

	function runOnActivation(event: KeyboardEvent, action: () => void) {
		if (event.key !== 'Enter' && event.key !== ' ') return;
		event.preventDefault();
		action();
	}

	function handleTrackRowClick(trackId: number, event: MouseEvent) {
		const additive = event.ctrlKey || event.metaKey;
		const range = event.shiftKey;
		updateTrackSelection(trackId, additive, range);
	}

	function handleAlbumCardClick(album: Album, event: MouseEvent) {
		const additive = event.ctrlKey || event.metaKey;
		const range = event.shiftKey;
		if (additive || range) {
			updateAlbumSelection(album.id, additive, range);
			return;
		}
		void openAlbumDetail(album);
	}

	async function removeAlbumFromLibrary(album: Album) {
		// Optimistically drop it, then remove the TIDAL favorite. Keep the full
		// object so Undo can re-favorite and restore it. Same path as the batch
		// delete below, so single and multi removal behave identically.
		if (undoTimer) clearTimeout(undoTimer);
		albums.update((list) => list.filter((a) => a.id !== album.id));
		searchResults = {
			tracks: searchResults.tracks,
			albums: searchResults.albums.filter((a) => a.id !== album.id),
			artists: searchResults.artists,
		};
		batchError = null;
		try {
			await api.batchDelete([], [album.id]);
			invalidateLibraryCaches();
			startUndoWindow({ tracks: [], albums: [album] }, `Removed "${album.title}" from your library.`);
		} catch (error) {
			// Roll the optimistic removal back in place.
			albums.update((list) => (list.some((a) => a.id === album.id) ? list : [album, ...list]));
			batchError = `Failed to remove album: ${error}`;
		}
	}

	// Arm the Undo banner for a set of just-removed items and schedule it to lapse.
	function startUndoWindow(removed: { tracks: Track[]; albums: Album[] }, message: string) {
		if (undoTimer) clearTimeout(undoTimer);
		pendingUndo = removed;
		batchMessage = message;
		undoTimer = setTimeout(() => {
			pendingUndo = { tracks: [], albums: [] };
			batchMessage = null;
		}, UNDO_WINDOW_MS);
	}

	function handleTrackRowKeydown(trackId: number, event: KeyboardEvent) {
		runOnActivation(event, () => updateTrackSelection(trackId));
		if (event.key === 'Enter' || event.key === ' ') event.stopPropagation();
	}

	function handleTrackListKeydown(event: KeyboardEvent) {
		if (activeTab !== 'tracks' && activeTab !== 'liked') return;
		if (event.key === 'ArrowDown') {
			event.preventDefault();
			if (visibleTracks.length === 0) return;
			cursorIndex = cursorIndex < 0 ? 0 : Math.min(cursorIndex + 1, visibleTracks.length - 1);
			return;
		}
		if (event.key === 'ArrowUp') {
			event.preventDefault();
			if (visibleTracks.length === 0) return;
			cursorIndex = cursorIndex <= 0 ? -1 : cursorIndex - 1;
			return;
		}
		if (event.key === 'Enter') {
			event.preventDefault();
			const track = cursorIndex >= 0 ? visibleTracks[cursorIndex] : null;
			if (!track) return;
			if (event.shiftKey) {
				void addTrackToQueue(track.id);
			} else if (event.metaKey || event.ctrlKey) {
				void playTrackNext(track.id);
			} else {
				void playTrack(track);
			}
			return;
		}
	}

	function handleAlbumCardKeydown(albumId: number, event: KeyboardEvent) {
		runOnActivation(event, () => updateAlbumSelection(albumId));
	}

	function handleSortKeydown(field: string, event: KeyboardEvent) {
		runOnActivation(event, () => handleSort(field));
	}

	async function handleBatchAddToPlaylist() {
		if (!selectedPlaylistId || $selectedTrackIds.size === 0) return;
		batchBusy = 'playlist';
		batchError = null;
		batchMessage = null;
		try {
			const result = await api.batchAddToPlaylist(Number(selectedPlaylistId), [...$selectedTrackIds]);
			batchMessage = `Added ${result.added} of ${result.resolved_tracks} selected tracks to the playlist.`;
			clearSelection();
		} catch (error) {
			batchError = `Failed to add tracks to playlist: ${error}`;
		} finally {
			batchBusy = null;
		}
	}

	async function handleBatchSetGenre() {
		if (!selectedGenreId || $selectedTrackIds.size === 0) return;
		batchBusy = 'genre';
		batchError = null;
		batchMessage = null;
		try {
			const result = await api.batchSetGenre(Number(selectedGenreId), [...$selectedTrackIds]);
			batchMessage = `Assigned the genre to ${result.affected} selected tracks.`;
			clearSelection();
		} catch (error) {
			batchError = `Failed to set genre: ${error}`;
		} finally {
			batchBusy = null;
		}
	}

	async function confirmDeleteSelection() {
		if ($selectedTrackIds.size === 0 && $selectedAlbumIds.size === 0) return;
		batchBusy = 'delete';
		batchError = null;
		batchMessage = null;
		if (undoTimer) clearTimeout(undoTimer);

		// Snapshot the full objects (not just ids) so Undo can re-favorite and
		// re-insert them. Selection is always made from on-screen rows/tiles.
		const removedTracks = visibleTracks.filter((t) => $selectedTrackIds.has(t.id));
		const removedAlbums = visibleAlbums.filter((a) => $selectedAlbumIds.has(a.id));
		const removedTrackIds = new Set(removedTracks.map((t) => t.id));
		const removedAlbumIds = new Set(removedAlbums.map((a) => a.id));

		tracks.update((list) => list.filter((track) => !removedTrackIds.has(track.id)));
		albums.update((list) => list.filter((album) => !removedAlbumIds.has(album.id)));
		searchResults = {
			tracks: searchResults.tracks.filter((track) => !removedTrackIds.has(track.id)),
			albums: searchResults.albums.filter((album) => !removedAlbumIds.has(album.id)),
			artists: searchResults.artists
		};

		try {
			const result = await api.batchDelete([...removedTrackIds], [...removedAlbumIds]);
			invalidateLibraryCaches();
			clearSelection();
			const parts: string[] = [];
			if (result.removed_tracks) parts.push(`${result.removed_tracks} track${result.removed_tracks === 1 ? '' : 's'}`);
			if (result.removed_albums) parts.push(`${result.removed_albums} album${result.removed_albums === 1 ? '' : 's'}`);
			startUndoWindow(
				{ tracks: removedTracks, albums: removedAlbums },
				`Removed ${parts.join(' and ') || 'selection'} from your library.`,
			);
		} catch (error) {
			// Restore the optimistic removals from the server on failure.
			batchError = `Failed to delete selection: ${error}`;
			invalidateLibraryCaches();
			void loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, activeTab === 'liked');
			void loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, 0, activeDecade);
		} finally {
			batchBusy = null;
		}
	}

	// A real Undo: re-add the TIDAL favorites that batchDelete removed, then
	// reload the affected view(s). No more "run sync to restore" hand-waving.
	async function undoDelete() {
		if (undoBusy) return;
		if (undoTimer) clearTimeout(undoTimer);
		const { tracks: undoTracks, albums: undoAlbums } = pendingUndo;
		if (undoTracks.length === 0 && undoAlbums.length === 0) return;
		pendingUndo = { tracks: [], albums: [] };
		undoBusy = true;
		batchError = null;
		batchMessage = 'Restoring…';
		try {
			await Promise.all([
				...undoTracks.map((t) => api.setTrackFavorite(t.id, true)),
				...undoAlbums.map((a) => api.setAlbumFavorite(a.id, true)),
			]);
			invalidateLibraryCaches();
			if (undoTracks.length) await loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, activeTab === 'liked');
			if (undoAlbums.length) await loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, 0, activeDecade);
			const count = undoTracks.length + undoAlbums.length;
			batchMessage = `Restored ${count} item${count === 1 ? '' : 's'} to your library.`;
		} catch (error) {
			batchError = `Undo failed: ${error}. Run a sync to restore favorites.`;
			batchMessage = null;
		} finally {
			undoBusy = false;
		}
	}

	function closeMenus() {
		activeTrackMenuId = null;
	}

	function toggleTrackMenu(trackId: number, event: MouseEvent) {
		event.stopPropagation();
		activeTrackMenuId = activeTrackMenuId === trackId ? null : trackId;
	}

	async function playTrackFromMenu(track: typeof $tracks[0], event: MouseEvent) {
		event.stopPropagation();
		closeMenus();
		await playTrack(track);
	}

	function selectTrackFromMenu(trackId: number, event: MouseEvent) {
		event.stopPropagation();
		closeMenus();
		updateTrackSelection(trackId);
	}

	function adaptAudioTracks(rows: AudioSearchResult[]): Track[] {
		return rows.map((r) => ({
			id: r.id,
			title: r.title,
			artist_id: 0,
			artist_name: r.artist_name,
			album_id: null,
			album_title: r.album_title,
			disc_number: null,
			track_number: null,
			duration_ms: r.duration_ms,
			isrc: null,
			tidal_id: r.tidal_id,
			best_quality: null,
			best_source: null,
			fidelity_score: 0,
			is_favorite: r.is_favorite,
			play_count: r.play_count,
			last_played_at: null,
			date_added: null,
			source: r.source,
			artwork_url: r.artwork_url,
			bpm: r.bpm,
			key_signature: r.key_signature,
			camelot_key: r.camelot_key,
			energy: r.energy,
			danceability: r.danceability,
		}));
	}

	async function runLibrarySearch(query: string) {
		const trimmed = query.trim();
		if (!trimmed) {
			searchResults = { tracks: [], albums: [], artists: [] };
			searchBusy = false;
			searchError = null;
			searchTotal = null;
			searchUnmatchedGenres = [];
			return;
		}

		searchBusy = true;
		searchError = null;
		try {
			const parsed = parseQuery(trimmed);
			if (hasAnyFilter(parsed)) {
				// DSP/filter syntax (bpm:138, key:Am, energy:>0.7, genre:dnb, etc.) - route to audio search.
				const params = buildAudioParams(parsed);
				const audio = await api.searchAudio(params);
				searchResults = { tracks: adaptAudioTracks(audio.tracks), albums: [], artists: [] };
				searchTotal = audio.total ?? null;
				searchUnmatchedGenres = audio.unmatched_genres ?? [];
			} else {
				// Plain text - server-side FTS. No more preloading the full library.
				const r = await cachedApi.search(trimmed, 100);
				searchResults = {
					tracks: r.tracks,
					albums: r.albums,
					artists: r.artists,
				};
				searchTotal = null;
				searchUnmatchedGenres = [];
			}
			clearSelection();
		} catch (error) {
			searchError = `Search failed: ${error}`;
			searchResults = { tracks: [], albums: [], artists: [] };
			searchTotal = null;
			searchUnmatchedGenres = [];
		} finally {
			searchBusy = false;
		}
	}

	// "Show more" for filtered searches: page past the 50-row display cap with
	// the server-side offset, appending without disturbing already-loaded rows.
	async function loadMoreSearchResults() {
		const trimmed = $searchQuery.trim();
		if (!trimmed || searchLoadingMore || searchBusy) return;
		const parsed = parseQuery(trimmed);
		if (!hasAnyFilter(parsed)) return;
		if (searchTotal !== null && searchResults.tracks.length >= searchTotal) return;
		searchLoadingMore = true;
		try {
			const audio = await api.searchAudio({
				...buildAudioParams(parsed),
				offset: searchResults.tracks.length,
			});
			const seen = new Set(searchResults.tracks.map((t) => t.id));
			searchResults = {
				...searchResults,
				tracks: [
					...searchResults.tracks,
					...adaptAudioTracks(audio.tracks).filter((t) => !seen.has(t.id)),
				],
			};
			searchTotal = audio.total ?? searchTotal;
		} catch (error) {
			searchError = `Search failed: ${error}`;
		} finally {
			searchLoadingMore = false;
		}
	}

	async function loadMoreVisibleItems() {
		if ($searchQuery.trim()) return;
		if (activeTab === 'artists') {
			await loadMoreArtists();
			return;
		}
		if ($isLoading || $isLoadingMore) return;
		if (activeTab === 'tracks' || activeTab === 'liked') {
			if ($tracks.length >= $totalTracks) return;
			await loadTracks($sortBy, $sortDir, PAGE_SIZE, $tracks.length, activeTab === 'liked');
			return;
		}
		if ($albums.length >= $totalAlbums) return;
		await loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, $albums.length, activeDecade);
	}

	async function playRandomLibrary() {
		batchError = null;
		batchMessage = null;
		try {
			const trimmed = $searchQuery.trim();
			if (trimmed) {
				// Draw the random pick from the FULL matching set, not just the ~50
				// rows on screen. Same server-side random sample the Shuffle button
				// uses, capped at one track.
				const params = buildAudioParams(parseQuery(trimmed));
				const audio = await api.searchAudio({
					...params,
					shuffle: true,
					liked_only: activeTab === 'liked',
					limit: 1,
				});
				const randomTrack = audio.tracks[0];
				if (!randomTrack) {
					throw new Error('No searchable tracks are available in the current library view.');
				}
				await playTrackNow(randomTrack.id);
				batchMessage = `Playing a random pick: ${randomTrack.title}.`;
				return;
			}

			if ($totalTracks === 0) {
				throw new Error('No tracks are loaded in the library yet.');
			}

			const randomOffset = Math.floor(Math.random() * $totalTracks);
			const data = await cachedApi.getTracks('date_added', 'desc', 1, randomOffset);
			const randomTrack = data.tracks[0];
			if (!randomTrack) {
				throw new Error('Could not resolve a random track from the library.');
			}
			await playTrackNow(randomTrack.id);
			batchMessage = `Playing a random pick: ${randomTrack.title}.`;
		} catch (error) {
			batchError = `Random play failed: ${error}`;
		}
	}

	let selectionSummary = $derived(
		`${$selectedTrackIds.size} track${$selectedTrackIds.size === 1 ? '' : 's'}${$selectedAlbumIds.size > 0 ? ` and ${$selectedAlbumIds.size} album${$selectedAlbumIds.size === 1 ? '' : 's'}` : ''} selected`
	);
	let selectionCount = $derived($selectedTrackIds.size + $selectedAlbumIds.size);
	let libraryModeLabel = $derived(
		activeTab === 'albums' ? 'Album view'
			: activeTab === 'artists' ? 'Artist view'
			: activeTab === 'liked' ? 'Liked view'
			: 'Track view'
	);
	let libraryModeCopy = $derived(
		activeTab === 'albums'
			? 'Artwork-first browse with quick album actions.'
			: activeTab === 'artists'
			? 'Browse your artists and explore their tracks.'
			: activeTab === 'liked'
			? "Tracks you've explicitly liked."
			: 'Dense track management with direct playback and batch work.'
	);
	let isSearchMode = $derived(Boolean($searchQuery.trim()));
	let visibleTracks = $derived.by(() => {
		if (!$searchQuery.trim()) return $tracks;
		// Search results don't know about liked_only, so filter client-side
		// to keep the Liked tab's promise honest while a query is active.
		const results = activeTab === 'liked'
			? searchResults.tracks.filter(t => t.is_favorite)
			: searchResults.tracks;
		if (!$sortBy || $sortBy === 'relevance') return results;
		const dir = $sortDir === 'desc' ? -1 : 1;
		return [...results].sort((a, b) => {
			let av: string | number | null | undefined;
			let bv: string | number | null | undefined;
			switch ($sortBy) {
				case 'title':    av = a.title?.toLowerCase();      bv = b.title?.toLowerCase();      break;
				case 'artist':   av = a.artist_name?.toLowerCase(); bv = b.artist_name?.toLowerCase(); break;
				case 'album':    av = a.album_title?.toLowerCase(); bv = b.album_title?.toLowerCase(); break;
				case 'play_count':     av = a.play_count;          bv = b.play_count;                 break;
				case 'date_added':     av = a.date_added;          bv = b.date_added;                 break;
				case 'last_played_at': av = a.last_played_at;      bv = b.last_played_at;             break;
				case 'bpm':            av = a.bpm;                 bv = b.bpm;                        break;
				case 'energy':         av = a.energy;              bv = b.energy;                     break;
				case 'danceability':   av = a.danceability;        bv = b.danceability;               break;
				default: return 0;
			}
			if (av == null && bv == null) return 0;
			if (av == null) return 1;
			if (bv == null) return -1;
			return dir * (av < bv ? -1 : av > bv ? 1 : 0);
		});
	});
	let vlistTracks = $derived(visibleTracks.slice(vlistStart, vlistEnd));
	let vlistTopPad = $derived(Math.min(vlistStart, visibleTracks.length) * trackRowPitch);
	let vlistBottomPad = $derived(Math.max(0, visibleTracks.length - vlistEnd) * trackRowPitch);
	let decadeBuckets = $derived.by(() => {
		const seen = new Set<number>();
		for (const a of $albums) {
			if (a.year != null) seen.add(Math.floor(a.year / 10) * 10);
		}
		return [...seen].sort((a, b) => a - b);
	});
	// Prefer the server-side decade list (covers the whole library); fall back to
	// decades derived from the loaded albums when the endpoint is unavailable.
	let decadeOptions = $derived(decadeChips.length > 0 ? decadeChips : decadeBuckets);
	let visibleAlbums = $derived.by(() => {
		let base = $searchQuery.trim() ? searchResults.albums : $albums;
		// In search mode the server doesn't re-sort results, so apply the album
		// ordering client-side to the matches. (Browse mode is sorted server-side.)
		if ($searchQuery.trim()) {
			const dir = albumSortDir === 'desc' ? -1 : 1;
			base = [...base].sort((a, b) => {
				let av: string | number | null | undefined;
				let bv: string | number | null | undefined;
				switch (albumSortField) {
					case 'title':  av = a.title?.toLowerCase();      bv = b.title?.toLowerCase();      break;
					case 'artist': av = a.artist_name?.toLowerCase(); bv = b.artist_name?.toLowerCase(); break;
					case 'year':   av = a.year;                      bv = b.year;                       break;
					default: return 0;
				}
				if (av == null && bv == null) return 0;
				if (av == null) return 1;
				if (bv == null) return -1;
				return dir * (av < bv ? -1 : av > bv ? 1 : 0);
			});
		}
		if (!activeDecade) return base;
		return base.filter(a => a.year != null && Math.floor(a.year / 10) * 10 === activeDecade);
	});
	let visibleArtists = $derived.by(() => {
		return $searchQuery.trim() ? searchResults.artists : artists;
	});
	let allSearchArtists = $derived(searchResults.artists);
	let allSearchArtistPreview = $derived(allSearchArtists.slice(0, ALL_SEARCH_ARTIST_PREVIEW_LIMIT));
	let allSearchAlbumPreview = $derived(visibleAlbums.slice(0, ALL_SEARCH_ALBUM_PREVIEW_LIMIT));
	let allSearchTrackPreview = $derived(visibleTracks.slice(0, ALL_SEARCH_TRACK_PREVIEW_LIMIT));
	let allSearchTotal = $derived(allSearchArtists.length + visibleAlbums.length + visibleTracks.length);
	let canLoadMore = $derived(
		!$searchQuery.trim() &&
		((activeTab === 'tracks' || activeTab === 'liked')
			? $tracks.length < $totalTracks
			: activeTab === 'albums'
			? $albums.length < $totalAlbums
			: activeTab === 'artists'
			? artists.length > 0 && !artistsExhausted
			: false)
	);
	// Filtered searches are display-capped server-side; when more matches
	// exist than rows loaded, say so instead of passing the cap off as the
	// whole result set.
	let searchTruncated = $derived(
		searchTotal !== null && searchResults.tracks.length < searchTotal
	);
	let searchSummary = $derived.by(() => {
		if (searchTruncated && (activeTab === 'tracks' || activeTab === 'liked' || activeTab === 'all')) {
			return `top ${searchResults.tracks.length} of ${searchTotal} track matches`;
		}
		return activeTab === 'all'
			? formatSearchSummary(allSearchArtists.length, visibleAlbums.length, visibleTracks.length)
			: (activeTab === 'tracks' || activeTab === 'liked')
			? `${visibleTracks.length} track match${visibleTracks.length === 1 ? '' : 'es'}`
			: `${visibleAlbums.length} album match${visibleAlbums.length === 1 ? '' : 'es'}`;
	});
	let loadedSummary = $derived(
		activeTab === 'albums'
			? `${$albums.length} of ${$totalAlbums} albums loaded`
			: activeTab === 'liked'
			? `${$tracks.length} of ${$totalTracks} liked tracks loaded`
			: `${$tracks.length} of ${$totalTracks} tracks loaded`
	);

	// ── Home view derived data ──────────────────────────────────────────────

	interface HomeArtist {
		id: number;
		name: string;
		photo_url: string | null;
		fallback_art_url: string | null;
		playCount: number;
		trackCount: number;
		albumCount: number;
	}

	type HeroArtist = HomeArtist & { kind: 'top' | 'forgotten_favorite' };

	let heroArtists = $derived.by<HeroArtist[]>(() => {
		const artistMap = new Map($artistsStore.map((a: Artist) => [a.id, a]));
		const countMap = new Map<number, HomeArtist>();
		const albumsByArtist = new Map<number, Set<number>>();

		for (const track of $tracks) {
			if (!track.artist_id) continue;
			const storeArtist = artistMap.get(track.artist_id);
			const info = countMap.get(track.artist_id);
			if (info) {
				info.playCount += track.play_count ?? 0;
				info.trackCount++;
				if (!info.fallback_art_url && track.artwork_url) {
					info.fallback_art_url = track.artwork_url;
				}
			} else {
				countMap.set(track.artist_id, {
					id: track.artist_id,
					name: track.artist_name ?? 'Unknown Artist',
					photo_url: storeArtist?.photo_url ?? null,
					fallback_art_url: track.artwork_url ?? null,
					playCount: track.play_count ?? 0,
					trackCount: 1,
					albumCount: 0,
				});
			}
			if (track.album_id) {
				if (!albumsByArtist.has(track.artist_id)) albumsByArtist.set(track.artist_id, new Set());
				albumsByArtist.get(track.artist_id)!.add(track.album_id);
			}
		}
		for (const [id, data] of countMap) {
			data.albumCount = albumsByArtist.get(id)?.size ?? 0;
		}

		const all = [...countMap.values()];
		const played = all.filter(a => a.playCount > 0).sort((a, b) => b.playCount - a.playCount);
		const top: HeroArtist[] = played.slice(0, 20).map(a => ({ ...a, kind: 'top' }));

		return top;
	});

	interface HomeArtistCard {
		id: number;
		name: string;
		photo_url: string | null;
		fallback_art_url: string | null;
	}

	let recentArtists = $derived.by<HomeArtistCard[]>(() => {
		const artistMap = new Map($artistsStore.map((a: Artist) => [a.id, a]));
		const seen = new Set<number>();
		const result: HomeArtistCard[] = [];

		const sorted = [...$tracks].sort((a, b) => {
			if (!a.last_played_at && !b.last_played_at) return 0;
			if (!a.last_played_at) return 1;
			if (!b.last_played_at) return -1;
			return b.last_played_at.localeCompare(a.last_played_at);
		});

		for (const track of sorted) {
			if (!track.artist_id || seen.has(track.artist_id)) continue;
			seen.add(track.artist_id);
			const storeArtist = artistMap.get(track.artist_id);
			result.push({
				id: track.artist_id,
				name: track.artist_name ?? 'Unknown',
				photo_url: storeArtist?.photo_url ?? null,
				fallback_art_url: track.artwork_url ?? null,
			});
			if (result.length >= 20) break;
		}
		return result;
	});

	let artistArtworkById = $derived.by(() => {
		const map = new Map<number, string>();
		for (const track of $tracks) {
			if (!track.artist_id || !track.artwork_url) continue;
			if (!map.has(track.artist_id)) map.set(track.artist_id, track.artwork_url);
		}
		return map;
	});

	interface HomeAlbumCard {
		id: number;
		title: string;
		artist_id: number | null;
		artist_name: string | null;
		artwork_url: string | null;
	}

	type HomeMuralItemKind = 'track' | 'album';

	interface HomeMuralItem {
		id: number;
		kind: HomeMuralItemKind;
		title: string;
		subtitle: string;
		artwork_url: string | null;
		track?: Track;
		album?: HomeAlbumCard;
	}

	interface HomeMuralPanel {
		id: string;
		label: string;
		caption: string;
		kind: HomeMuralItemKind;
		items: HomeMuralItem[];
	}

	let recentAlbums = $derived.by<HomeAlbumCard[]>(() => {
		const albumDateMap = new Map<number, { card: HomeAlbumCard; date: string }>();

		for (const track of $tracks) {
			if (!track.album_id || !track.date_added) continue;
			const existing = albumDateMap.get(track.album_id);
			if (!existing || track.date_added > existing.date) {
				albumDateMap.set(track.album_id, {
					card: {
						id: track.album_id,
						title: track.album_title ?? 'Unknown Album',
						artist_id: track.artist_id ?? null,
						artist_name: track.artist_name,
						artwork_url: track.artwork_url,
					},
					date: track.date_added,
				});
			}
		}

		return [...albumDateMap.values()]
			.sort((a, b) => b.date.localeCompare(a.date))
			.slice(0, 20)
			.map(({ card }) => card);
	});

	let randomPanelTracks = $state<Track[]>(homePanelCandidateCache.randomTracks);
	let randomPanelAlbums = $state<HomeAlbumCard[]>(homePanelCandidateCache.randomAlbums);
	let randomPanelRequestKey = $state(homePanelCandidateCache.randomRequestKey);
	// Same-artist / same-album expansion of the seeds. Kept only as a fallback
	// (offline / server error) and as tail-fill when the server returns fewer
	// than a full panel; the server list is the primary, cross-artist source.
	let suggestionCandidateTracks = $state<Track[]>(homePanelCandidateCache.suggestionTracks);
	// Server-ranked, cross-artist, library-resolved picks from /api/home/suggestions.
	let suggestionServerTracks = $state<Track[]>(homePanelCandidateCache.suggestionServerTracks);
	let suggestionCandidateRequestKey = $state(homePanelCandidateCache.suggestionRequestKey);

	// Max tracks (and albums) one artist may contribute to a suggestion panel, so
	// a single prolific neighbour can't clone-fill it. Mirrors the server cap.
	const SUGGESTION_ARTIST_CAP = 2;

	function suggestionArtistKey(track: Track): number | string {
		return track.artist_id ?? track.artist_name ?? '';
	}

	// Greedy per-artist cap. An empty key (missing artist) is never capped so
	// those tracks don't all collapse into one synthetic bucket.
	function capPerArtist(tracks: Track[], max: number): Track[] {
		const perArtist = new Map<number | string, number>();
		const out: Track[] = [];
		for (const track of tracks) {
			const key = suggestionArtistKey(track);
			if (max > 0 && key !== '') {
				const count = perArtist.get(key) ?? 0;
				if (count >= max) continue;
				perArtist.set(key, count + 1);
			}
			out.push(track);
		}
		return out;
	}

	// The current same-artist expansion, scored/sorted exactly as before. Used
	// only when there is no server list (offline) and for tail-fill.
	function sameArtistScoredTracks(seeds: Track[]): Track[] {
		const seedTrackIds = new Set(seeds.map(track => track.id));
		const seedArtistIds = new Set(seeds.map(track => track.artist_id).filter((id): id is number => id != null));
		const seedAlbumIds = new Set(seeds.map(track => track.album_id).filter((id): id is number => id != null));
		return suggestionCandidateTracks
			.map(track => ({ track, score: listenHistoryTrackScore(track, seedTrackIds, seedArtistIds, seedAlbumIds) }))
			.filter(({ score }) => score > 0)
			.sort((a, b) => b.score - a.score || stableRank(a.track.id, dailySalt(11)) - stableRank(b.track.id, dailySalt(11)))
			.map(({ track }) => track);
	}

	// Ordered candidate pool: server-ranked cross-artist picks first, then the
	// same-artist expansion as tail-fill (deduped, seeds removed). Falls back to
	// the same-artist pool entirely when the server list is empty.
	function combinedSuggestionTracks(): Track[] {
		const seeds = listenHistorySeeds();
		const seedTrackIds = new Set(seeds.map(track => track.id));
		const server = suggestionServerTracks.filter(track => !seedTrackIds.has(track.id));
		const fallback = sameArtistScoredTracks(seeds);
		if (server.length === 0) return fallback;
		const seen = new Set(server.map(track => track.id));
		const tail = fallback.filter(track => !seen.has(track.id));
		return [...server, ...tail];
	}

	let suggestedTrackItems = $derived.by<HomeMuralItem[]>(() =>
		capPerArtist(combinedSuggestionTracks(), SUGGESTION_ARTIST_CAP)
			.slice(0, HOME_MURAL_ITEM_LIMIT)
			.map(trackToMuralItem)
	);

	let suggestedAlbumItems = $derived.by<HomeMuralItem[]>(() => {
		const seeds = listenHistorySeeds();
		const seedAlbumIds = new Set(seeds.map(track => track.album_id).filter((id): id is number => id != null));
		const seedArtistIds = new Set(seeds.map(track => track.artist_id).filter((id): id is number => id != null));

		// One album card per distinct non-seed album, in candidate order (server
		// ranking first, same-artist tail after). Albums by artists you did NOT
		// seed come first so the panel is genuine cross-artist discovery; albums by
		// the seed artists themselves only fill in if the cross-artist pool is thin.
		const seenAlbums = new Set<number>();
		const crossArtist: { card: HomeAlbumCard; artistKey: number | string }[] = [];
		const seedArtist: { card: HomeAlbumCard; artistKey: number | string }[] = [];
		for (const track of combinedSuggestionTracks()) {
			if (!track.album_id || seedAlbumIds.has(track.album_id) || seenAlbums.has(track.album_id)) continue;
			seenAlbums.add(track.album_id);
			const entry = { card: homeAlbumCardFromTrack(track), artistKey: suggestionArtistKey(track) };
			const isSeedArtist = track.artist_id != null && seedArtistIds.has(track.artist_id);
			(isSeedArtist ? seedArtist : crossArtist).push(entry);
		}
		const chosen: HomeAlbumCard[] = [];
		const chosenIds = new Set<number>();

		// Drain a bucket in two passes: artist-diverse first (cap per artist) so
		// the panel leads with variety, then top up from whatever the cap skipped.
		// Cross-artist is fully exhausted before seed-artist albums are touched, so
		// the discography of a seed artist only ever fills a genuine shortfall.
		const drain = (bucket: { card: HomeAlbumCard; artistKey: number | string }[]) => {
			const perArtist = new Map<number | string, number>();
			for (const { card, artistKey } of bucket) {
				if (chosen.length >= HOME_MURAL_ITEM_LIMIT) return;
				if (chosenIds.has(card.id)) continue;
				if (artistKey !== '') {
					const count = perArtist.get(artistKey) ?? 0;
					if (count >= SUGGESTION_ARTIST_CAP) continue;
					perArtist.set(artistKey, count + 1);
				}
				chosen.push(card);
				chosenIds.add(card.id);
			}
			for (const { card } of bucket) {
				if (chosen.length >= HOME_MURAL_ITEM_LIMIT) return;
				if (chosenIds.has(card.id)) continue;
				chosen.push(card);
				chosenIds.add(card.id);
			}
		};

		drain(crossArtist);
		drain(seedArtist);

		return chosen.map(albumToMuralItem);
	});

	let randomTrackItems = $derived.by<HomeMuralItem[]>(() =>
		randomPanelTracks.map(trackToMuralItem)
	);

	let randomAlbumItems = $derived.by<HomeMuralItem[]>(() =>
		randomPanelAlbums.map(albumToMuralItem)
	);

	let homeMuralPanels = $derived.by<HomeMuralPanel[]>(() => {
		const panels: HomeMuralPanel[] = [
			{
				id: 'suggested-tracks',
				label: 'Suggested tracks',
				caption: 'Listen history suggestions',
				kind: 'track',
				items: suggestedTrackItems,
			},
			{
				id: 'suggested-albums',
				label: 'Suggested albums',
				caption: 'Listen history suggestions',
				kind: 'album',
				items: suggestedAlbumItems,
			},
			{
				id: 'random-tracks',
				label: 'Random tracks',
				caption: 'Library shuffle picks',
				kind: 'track',
				items: randomTrackItems,
			},
			{
				id: 'random-albums',
				label: 'Random albums',
				caption: 'Library shuffle picks',
				kind: 'album',
				items: randomAlbumItems,
			},
		];
		return panels.filter(panel => panel.items.length > 0);
	});

	// Per-tile lazy artwork. Keyed by domain-prefixed id so we never collide
	// (track 5 and album 5 are independent entries). Populated by lazyTidalArt
	// when a tile without baked artwork scrolls into view.
	let lazyArt = $state<Record<string, string>>({});
	let artistLazyArt = $state<Record<number, string>>({});

	function dailySalt(offset: number): number {
		const now = new Date();
		return now.getFullYear() * 10000 + (now.getMonth() + 1) * 100 + now.getDate() + offset;
	}

	function homePanelRefreshBucket(): number {
		return Math.floor(Date.now() / HOME_PANEL_CACHE_REFRESH_MS);
	}

	function stableRank(id: number, salt: number): number {
		let value = Math.imul(id ^ salt, 0x45d9f3b);
		value = Math.imul(value ^ (value >>> 16), 0x45d9f3b);
		return (value ^ (value >>> 16)) >>> 0;
	}

	function stableRandomOffsets(total: number, saltOffset: number, limit: number, refreshBucket: number): number[] {
		const count = Math.min(Math.max(total, 0), limit);
		if (count === 0) return [];

		const salt = dailySalt(saltOffset) ^ total ^ refreshBucket;
		const offsets: number[] = [];
		const used = new Set<number>();
		let attempt = 0;

		while (offsets.length < count && attempt < count * 16 + 64) {
			const offset = stableRank(attempt + 1, salt) % total;
			if (!used.has(offset)) {
				used.add(offset);
				offsets.push(offset);
			}
			attempt++;
		}

		for (let offset = 0; offsets.length < count && offset < total; offset++) {
			if (!used.has(offset)) offsets.push(offset);
		}

		return offsets;
	}

	async function loadRandomPanelCandidates(trackTotal: number, albumTotal: number, requestKey: string, refreshBucket: number) {
		const [tracksForPanel, albumsForPanel] = await Promise.all([
			loadRandomPanelTracks(trackTotal, refreshBucket),
			loadRandomPanelAlbums(albumTotal, refreshBucket),
		]);
		if (randomPanelRequestKey === requestKey) {
			randomPanelTracks = tracksForPanel;
			randomPanelAlbums = albumsForPanel;
			homePanelCandidateCache.randomTracks = tracksForPanel;
			homePanelCandidateCache.randomAlbums = albumsForPanel;
			homePanelCandidateCache.randomRequestKey = requestKey;
		}
	}

	async function loadRandomPanelTracks(trackTotal: number, refreshBucket: number): Promise<Track[]> {
		if (trackTotal <= 0) return [];
		const offsets = stableRandomOffsets(trackTotal, 31, HOME_MURAL_ITEM_LIMIT, refreshBucket);
		const responses = await Promise.all(
			offsets.map(offset => cachedApi.getTracks('date_added', 'desc', 1, offset, true, false))
		);
		return uniqueById(responses.flatMap(response => response.tracks));
	}

	async function loadRandomPanelAlbums(albumTotal: number, refreshBucket: number): Promise<HomeAlbumCard[]> {
		if (albumTotal <= 0) return [];
		const offsets = stableRandomOffsets(albumTotal, 41, HOME_MURAL_ITEM_LIMIT, refreshBucket);
		const responses = await Promise.all(
			offsets.map(offset => cachedApi.getAlbums('title', 'asc', 1, offset, true))
		);
		return uniqueById(responses.flatMap(response => response.albums).map(album => ({
			id: album.id,
			title: album.title,
			artist_id: album.artist_id ?? null,
			artist_name: album.artist_name,
			artwork_url: album.artwork_url,
		})));
	}

	function uniqueById<T extends { id: number }>(items: T[]): T[] {
		const seen = new Set<number>();
		const result: T[] = [];
		for (const item of items) {
			if (seen.has(item.id)) continue;
			seen.add(item.id);
			result.push(item);
		}
		return result;
	}

	function uniquePositiveIds(ids: Array<number | null | undefined>, limit: number): number[] {
		const seen = new Set<number>();
		const result: number[] = [];
		for (const id of ids) {
			if (id == null || id <= 0 || seen.has(id)) continue;
			seen.add(id);
			result.push(id);
			if (result.length >= limit) break;
		}
		return result;
	}

	// Same-artist / same-album expansion of the seeds. This is the legacy
	// candidate source, now demoted to fallback + tail-fill behind the server list.
	async function sameArtistExpansion(seedTracks: Track[]): Promise<Track[]> {
		const seedArtistIds = uniquePositiveIds(seedTracks.map(track => track.artist_id), 8);
		const seedAlbumIds = uniquePositiveIds(seedTracks.map(track => track.album_id), 8);
		const artistResults = await Promise.allSettled(seedArtistIds.map(id => cachedApi.getArtistTracks(id)));
		const albumResults = await Promise.allSettled(seedAlbumIds.map(id => cachedApi.getAlbumTracks(id)));
		const artistTracks = artistResults.flatMap(result =>
			result.status === 'fulfilled' ? result.value.tracks : []
		);
		const albumTracks = albumResults.flatMap(result =>
			result.status === 'fulfilled' ? result.value.tracks : []
		);
		return uniqueById([...seedTracks, ...albumTracks, ...artistTracks]);
	}

	async function loadSuggestionCandidates(seedTracks: Track[], requestKey: string) {
		const seedIds = uniquePositiveIds(seedTracks.map(track => track.id), HOME_MURAL_ITEM_LIMIT);
		// Server-ranked cross-artist picks are the primary source; the same-artist
		// expansion runs in parallel as fallback + tail-fill. Both degrade to []
		// on failure so a hiccup never blanks the panels.
		const [serverTracks, expansion] = await Promise.all([
			cachedApi
				.getHomeSuggestions(seedIds, 50)
				.then(res => res.tracks ?? [])
				.catch(error => {
					console.error('Failed to load home suggestions:', error);
					return [] as Track[];
				}),
			sameArtistExpansion(seedTracks).catch(error => {
				console.error('Failed to load same-artist expansion:', error);
				return [] as Track[];
			}),
		]);
		if (suggestionCandidateRequestKey === requestKey) {
			suggestionServerTracks = serverTracks;
			suggestionCandidateTracks = expansion;
			homePanelCandidateCache.suggestionServerTracks = serverTracks;
			homePanelCandidateCache.suggestionTracks = expansion;
			homePanelCandidateCache.suggestionRequestKey = requestKey;
		}
	}

	function listenHistorySeeds(): Track[] {
		const seen = new Set<number>();
		const seeds: Track[] = [];
		const playedTracks = [...$tracks]
			.filter(track => track.last_played_at)
			.sort((a, b) => (b.last_played_at ?? '').localeCompare(a.last_played_at ?? ''));

		for (const track of [...recentTracks, ...playedTracks]) {
			if (seen.has(track.id)) continue;
			seen.add(track.id);
			seeds.push(track);
			if (seeds.length >= HOME_MURAL_ITEM_LIMIT) break;
		}

		return seeds;
	}

	function listenHistoryTrackScore(
		track: Track,
		seedTrackIds: Set<number>,
		seedArtistIds: Set<number>,
		seedAlbumIds: Set<number>,
	): number {
		if (seedTrackIds.has(track.id)) return 0;
		let score = 0;
		if (seedArtistIds.has(track.artist_id)) score += 8;
		if (track.album_id && seedAlbumIds.has(track.album_id)) score += 3;
		if (track.is_favorite) score += 1.2;
		if ((track.play_count ?? 0) === 0) score += 1.4;
		else score += Math.min(track.play_count ?? 0, 12) * 0.12;
		if (track.fidelity_score > 0) score += Math.min(track.fidelity_score, 100) / 100;
		if (track.last_played_at) score -= 0.8;
		return score;
	}

	function homeAlbumCardFromTrack(track: Track): HomeAlbumCard {
		return {
			id: track.album_id ?? 0,
			title: track.album_title ?? 'Unknown Album',
			artist_id: track.artist_id ?? null,
			artist_name: track.artist_name,
			artwork_url: track.artwork_url,
		};
	}

	function albumFromHomeCard(card: HomeAlbumCard): Album {
		return {
			id: card.id,
			tidal_id: null,
			title: card.title,
			artist_id: card.artist_id ?? 0,
			artist_name: card.artist_name,
			year: null,
			artwork_url: card.artwork_url,
			release_type: null,
			track_count: null,
			source: 'tidal',
		};
	}

	function trackToMuralItem(track: Track): HomeMuralItem {
		return {
			id: track.id,
			kind: 'track',
			title: track.title,
			subtitle: track.artist_name ?? track.album_title ?? 'Unknown artist',
			artwork_url: track.artwork_url,
			track,
		};
	}

	function albumToMuralItem(album: HomeAlbumCard): HomeMuralItem {
		return {
			id: album.id,
			kind: 'album',
			title: album.title,
			subtitle: album.artist_name ?? 'Unknown artist',
			artwork_url: album.artwork_url,
			album,
		};
	}

	function fallbackLetters(label: string): string {
		return label.split(/\s+/).map(part => part[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
	}

	// Domain-prefixed key so a track and an album with the same numeric id never
	// collide in the lazyArt map (mirrors the track/album row keys).
	function muralItemKey(item: HomeMuralItem): string {
		return `${item.kind}-${item.id}`;
	}

	// Search terms the lazy Tidal-art lookup resolves against, shared by the
	// mural's lazy action and the synchronous cache peek so both hit the same key.
	function muralItemLazyQuery(item: HomeMuralItem): { artist: string | null; title: string } {
		if (item.kind === 'album') {
			return { artist: item.album?.artist_name ?? null, title: item.album?.title ?? item.title };
		}
		return { artist: item.track?.artist_name ?? null, title: item.title };
	}

	// Artwork with the same "always loaded" chain as the home-recs murals: baked
	// art -> already-resolved lazy art -> previously-cached art (peek). The peek
	// paints a full collage on first launch; live lookups swap in fresh art.
	function muralItemArtwork(item: HomeMuralItem): string | null {
		const resolved = item.artwork_url ?? lazyArt[muralItemKey(item)];
		if (resolved) return resolved;
		const query = muralItemLazyQuery(item);
		return peekTidalArt(composeTidalArtQuery(query.artist, query.title));
	}

	function artistImageSources(
		photoUrl: string | null | undefined,
		lazyUrl: string | null | undefined,
		fallbackUrl: string | null | undefined,
	): string[] {
		return [photoUrl, lazyUrl, fallbackUrl]
			.filter((source): source is string => typeof source === 'string' && source.trim().length > 0);
	}

	function openHomeAlbumCard(card: HomeAlbumCard) {
		const found = $albums.find(album => album.id === card.id);
		void openAlbumDetail(found ?? albumFromHomeCard(card));
	}

	async function playHomeMuralTrack(item: HomeMuralItem, panel: HomeMuralPanel) {
		if (!item.track) return;
		const seen = new Set<number>();
		const trackIds = panel.items
			.filter((candidate) => candidate.kind === 'track' && candidate.track)
			.map((candidate) => candidate.track!.id)
			.filter((trackId) => {
				if (seen.has(trackId)) return false;
				seen.add(trackId);
				return true;
			});
		if (!seen.has(item.track.id)) {
			trackIds.unshift(item.track.id);
		}

		try {
			if (trackIds.length > 0) {
				const replaced = await api.replacePlaybackQueue(
					trackIds.map((track_id) => ({ track_id })),
					{ shuffleMode: get(shuffleMode) }
				);
				const selected = replaced.queue.find((queueItem) => queueItem.track.id === item.track!.id);
				if (selected) await api.playQueueItem(selected.id);
			}
		} catch (error) {
			console.error('Failed to play home panel track:', error);
			await playTrackNow(item.track.id);
		}
	}

	function openHomeMuralItem(item: HomeMuralItem, panel: HomeMuralPanel) {
		if (item.kind === 'track' && item.track) {
			void playHomeMuralTrack(item, panel);
			return;
		}
		if (item.kind === 'album' && item.album) {
			openHomeAlbumCard(item.album);
		}
	}

	function openHomeMuralItemContextMenu(event: MouseEvent, item: HomeMuralItem) {
		event.preventDefault();
		event.stopPropagation();
		if (item.kind === 'track' && item.track) {
			openContextMenu(event, buildTrackMenu(item.track), item.title);
			return;
		}
		if (item.kind === 'album' && item.album) {
			handleHomeAlbumContextMenu(event, item.id, item.album);
		}
	}

	$effect(() => {
		const trackTotal = $totalTracks;
		const albumTotal = $totalAlbums;
		if (trackTotal <= 0 && albumTotal <= 0) return;

		const refreshBucket = homePanelRefreshBucket();
		const requestKey = `${dailySalt(0)}:${refreshBucket}:${trackTotal}:${albumTotal}`;
		if (randomPanelRequestKey === requestKey) return;
		randomPanelRequestKey = requestKey;

		void loadRandomPanelCandidates(trackTotal, albumTotal, requestKey, refreshBucket).catch((error) => {
			console.error('Failed to load random library panels:', error);
		});
	});

	$effect(() => {
		const seeds = listenHistorySeeds();
		const seedKey = seeds.map(track => `${track.id}:${track.last_played_at ?? ''}`).join('|');
		const requestKey = seedKey ? `${homePanelRefreshBucket()}:${seedKey}` : '';
		if (!requestKey) {
			return;
		}
		if (suggestionCandidateRequestKey === requestKey) return;
		suggestionCandidateRequestKey = requestKey;

		// Keep the last-good candidates on failure instead of zeroing (which made
		// the whole panel vanish). loadSuggestionCandidates already degrades each
		// source to [] internally, so this only fires on unexpected throws.
		void loadSuggestionCandidates(seeds, requestKey).catch((error) => {
			console.error('Failed to load suggestion candidates:', error);
		});
	});

	// ── Home view handlers ─────────────────────────────────────────────────

	function playAllForArtist(artistId: number) {
		void playArtistNow(artistId);
	}

	function shuffleArtist(artistId: number) {
		void shuffleArtistNow(artistId);
	}

	function handleHomeArtistClick(artistId: number) {
		void goto(`/artists/${artistId}`);
	}

	function handleHomeAlbumClick(albumId: number) {
		const found = $albums.find(a => a.id === albumId);
		if (found) {
			void openAlbumDetail(found);
		} else {
			// Album not in current loaded page - build a stub from recentAlbums card
			const card = recentAlbums.find(a => a.id === albumId);
			if (!card) return;
			const stub: import('$lib/api/client').Album = {
				id: card.id,
				tidal_id: null,
				title: card.title,
				artist_id: 0,
				artist_name: card.artist_name,
				year: null,
				artwork_url: card.artwork_url,
				release_type: null,
				track_count: null,
				source: 'tidal',
			};
			void openAlbumDetail(stub);
		}
	}

	$effect(() => {
		const nextQuery = $searchQuery;
		if (searchTimer) clearTimeout(searchTimer);
		searchTimer = setTimeout(() => {
			void runLibrarySearch(nextQuery);
		}, 220);

		return () => {
			if (searchTimer) clearTimeout(searchTimer);
		};
	});

	$effect(() => {
		infiniteObserver?.disconnect();
		if (!infiniteSentinel || !canLoadMore || $searchQuery.trim()) return;

		infiniteObserver = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) {
					void loadMoreVisibleItems();
				}
			},
			{ rootMargin: '240px 0px' }
		);
		infiniteObserver.observe(infiniteSentinel);

		return () => {
			infiniteObserver?.disconnect();
			infiniteObserver = null;
		};
	});

	// Track-list virtualization: follow the workspace scroll while a track
	// list is on screen.
	$effect(() => {
		if (activeTab !== 'tracks' && activeTab !== 'liked') return;
		// Capturing listener on the document sees scrolls of any container
		// (workspace on desktop, document on mobile) without re-binding when
		// the responsive layout flips between them.
		document.addEventListener('scroll', scheduleTrackWindowUpdate, { passive: true, capture: true });
		window.addEventListener('resize', scheduleTrackWindowUpdate);
		scheduleTrackWindowUpdate();
		return () => {
			document.removeEventListener('scroll', scheduleTrackWindowUpdate, { capture: true });
			window.removeEventListener('resize', scheduleTrackWindowUpdate);
			if (vlistRaf) {
				cancelAnimationFrame(vlistRaf);
				vlistRaf = 0;
			}
		};
	});

	// Re-window when the data set or row pitch changes (page appended, search,
	// sort, tab switch).
	$effect(() => {
		// eslint-disable-next-line @typescript-eslint/no-unused-expressions
		visibleTracks.length; trackRowPitch;
		scheduleTrackWindowUpdate();
	});

	// Measure the real row pitch from rendered rows. With two rows the pitch
	// includes inter-row spacing (mobile adds a margin); one row falls back to
	// its own height. Settles after the first write since equal values no-op.
	$effect(() => {
		// eslint-disable-next-line @typescript-eslint/no-unused-expressions
		vlistStart; vlistEnd;
		const rows = trackRowsEl?.querySelectorAll<HTMLElement>('.track-row');
		if (!rows || rows.length === 0) return;
		const pitch = rows.length >= 2
			? rows[1].getBoundingClientRect().top - rows[0].getBoundingClientRect().top
			: rows[0].offsetHeight;
		if (pitch > 0 && Math.abs(pitch - trackRowPitch) > 0.5) trackRowPitch = pitch;
	});

	// Position memory (Phase 5B - SvelteKit snapshot)
	// Snapshot binds state to the browser's history entry. Multi-selection is
	// active-task state and is intentionally not captured (resets on nav).
	let pendingRestoreScroll = $state<number | null>(null)

	$effect(() => {
		if (pendingRestoreScroll !== null) {
			const target = pendingRestoreScroll
			pendingRestoreScroll = null
			restoreScroll(target)
		}
	})

	type LibrarySnapshot = {
		activeTab: typeof activeTab
		searchQuery: string
		sortBy: string
		sortDir: 'asc' | 'desc'
		viewMode: 'grid' | 'list'
		activeDecade: number | null
		albumSortField: AlbumSortField
		albumSortDir: 'asc' | 'desc'
		scrollY: number
		// How many rows were loaded via infinite scroll, so a back-nav can
		// re-page to the same depth before restoring scroll. Tracks/albums live
		// in persistent stores; artists in component-local state.
		loadedCount: number
	}
	function currentLoadedCount(): number {
		if (activeTab === 'artists') return artists.length
		if (activeTab === 'albums') return get(albums).length
		if (activeTab === 'tracks' || activeTab === 'liked') return get(tracks).length
		return 0
	}
	export const snapshot: Snapshot<LibrarySnapshot> = {
		capture: () => ({
			activeTab,
			searchQuery: get(searchQuery),
			sortBy: get(sortBy),
			sortDir: get(sortDir),
			viewMode: get(viewMode),
			activeDecade,
			albumSortField,
			albumSortDir,
			scrollY: captureScroll(),
			loadedCount: currentLoadedCount()
		}),
		restore: (saved) => {
			const validTabs = ['all', 'tracks', 'liked', 'albums', 'artists'] as const
			if ((validTabs as readonly string[]).includes(saved.activeTab)) {
				activeTab = saved.activeTab as typeof activeTab
			}
			if (typeof saved.searchQuery === 'string') searchQuery.set(saved.searchQuery)
			if (typeof saved.sortBy === 'string') sortBy.set(saved.sortBy)
			if (saved.sortDir === 'asc' || saved.sortDir === 'desc') sortDir.set(saved.sortDir)
			if (saved.viewMode === 'grid' || saved.viewMode === 'list') viewMode.set(saved.viewMode)
			if (saved.albumSortField === 'title' || saved.albumSortField === 'artist' || saved.albumSortField === 'year') albumSortField = saved.albumSortField
			if (saved.albumSortDir === 'asc' || saved.albumSortDir === 'desc') albumSortDir = saved.albumSortDir
			activeDecade = saved.activeDecade
			if (typeof saved.scrollY === 'number') pendingRestoreScroll = saved.scrollY
			// Artists live in component-local state (not a store), and onMount only
			// reloads albums/tracks. Without this, restoring into the artists tab on
			// a back-nav shows an empty "No artists yet" state. Re-page to the depth
			// the user had scrolled to so the saved scroll offset is reachable.
			// Browse list only - the search effect repopulates a restored query.
			if (activeTab === 'artists' && !saved.searchQuery?.trim() && artists.length === 0) {
				const targetScroll = typeof saved.scrollY === 'number' ? saved.scrollY : 0
				void loadArtistsUpTo(saved.loadedCount ?? PAGE_SIZE).then(() => restoreScroll(targetScroll))
			}
		}
	}

	// Reset cursor when switching tabs or changing the search query.
	$effect(() => {
		// eslint-disable-next-line @typescript-eslint/no-unused-expressions
		activeTab; $searchQuery;
		cursorIndex = -1;
	})

	// Reset decade filter when leaving albums tab.
	$effect(() => {
		if (activeTab !== 'albums') activeDecade = null;
	})

	// Keep the highlighted track in view as the cursor moves.
	$effect(() => {
		if (cursorIndex < 0) return;
		const el = document.querySelector<HTMLElement>(`.track-row[data-cursor-idx="${cursorIndex}"]`);
		if (el) {
			el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
			return;
		}
		// Cursor row is outside the rendered window - scroll its slot into view
		// and let the window update mount it.
		if (!trackRowsEl) return;
		const scroller = trackScroller();
		const scrollTop = scroller ? scroller.scrollTop : window.scrollY;
		const viewTop = scroller ? scroller.getBoundingClientRect().top : 0;
		const viewHeight = scroller ? scroller.clientHeight : window.innerHeight;
		const rowsTop = trackRowsEl.getBoundingClientRect().top - viewTop + scrollTop;
		const target = { top: rowsTop + cursorIndex * trackRowPitch - viewHeight / 2, behavior: 'smooth' as const };
		if (scroller) scroller.scrollTo(target);
		else window.scrollTo(target);
	})
</script>

<svelte:window onclick={closeMenus} onkeydown={(e) => {
	if (e.key === 'Escape') {
		if (expandedAlbumId !== null) { expandedAlbumId = null; detailAlbum = null; detailAlbumTracksList = []; }
		if (expandedTrackId !== null) { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }
	}
}} />

<div class="page-shell library">
	<div class="library-search-shell">
		<SearchField
			bind:value={$searchQuery}
			variant="page"
			facets
			inlineCompletion
			filterChips
			placeholder={activeTab === 'albums' ? 'Search albums or artists' : 'Search tracks, albums, or artists'}
		/>
		<div class="filter-pills">
			<div class="filter-pill-group filter-pill-group--primary">
				<button class="filter-pill" class:active={activeTab === 'all'}     onclick={() => switchTab('all')}>All</button>
				<button class="filter-pill" class:active={activeTab === 'tracks'}  onclick={() => switchTab('tracks')}>Tracks</button>
				<button class="filter-pill" class:active={activeTab === 'liked'}   onclick={() => switchTab('liked')}>Liked</button>
				<button class="filter-pill" class:active={activeTab === 'albums'}  onclick={() => switchTab('albums')}>Albums</button>
				<button class="filter-pill" class:active={activeTab === 'artists'} onclick={() => switchTab('artists')}>Artists</button>
				<button class="filter-pill" onclick={() => void playRandomLibrary()} title="Random play">
					<span class="pill-glyph" aria-hidden="true">⤮</span>Random
				</button>
			</div>

			{#if hasToolbarActions}
			<div class="filter-pill-actions">
				{#if activeTab === 'tracks' || activeTab === 'liked'}
					<div class="play-controls" role="group" aria-label="Play this view">
						<button class="filter-pill filter-pill--accent" onclick={() => void playTrackView(false)} title="Play this view">
							<span class="pill-glyph" aria-hidden="true">▶</span>Play
						</button>
						<button class="filter-pill" onclick={() => void playTrackView(true)} title="Shuffle this view">
							<span class="pill-glyph" aria-hidden="true">⤮</span>Shuffle
						</button>
					</div>
				{/if}
				{#if activeTab === 'albums'}
					<div class="album-sort" role="group" aria-label="Sort albums">
						<span class="album-sort-label">Sort</span>
						{#each (['title', 'artist', 'year'] as const) as field (field)}
							<button
								class="album-sort-btn"
								class:active={albumSortField === field}
								onclick={() => setAlbumSort(field)}
								aria-pressed={albumSortField === field}
								title="Sort by {ALBUM_SORT_LABELS[field]}{albumSortField === field ? (albumSortDir === 'asc' ? ' (ascending)' : ' (descending)') : ''}"
							>
								{ALBUM_SORT_LABELS[field]}{#if albumSortField === field}<span class="album-sort-arrow">{albumSortDir === 'asc' ? '↑' : '↓'}</span>{/if}
							</button>
						{/each}
					</div>
					<div class="view-toggle" role="group" aria-label="Album view layout">
						<button
							class="view-toggle-btn"
							class:active={$viewMode === 'grid'}
							onclick={() => viewMode.set('grid')}
							aria-pressed={$viewMode === 'grid'}
							aria-label="Grid view"
							title="Grid view"
						>
							<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
								<rect x="2" y="2" width="5" height="5" rx="1"/>
								<rect x="9" y="2" width="5" height="5" rx="1"/>
								<rect x="2" y="9" width="5" height="5" rx="1"/>
								<rect x="9" y="9" width="5" height="5" rx="1"/>
							</svg>
						</button>
						<button
							class="view-toggle-btn"
							class:active={$viewMode === 'list'}
							onclick={() => viewMode.set('list')}
							aria-pressed={$viewMode === 'list'}
							aria-label="List view"
							title="List view"
						>
							<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
								<line x1="3" y1="4" x2="13" y2="4"/>
								<line x1="3" y1="8" x2="13" y2="8"/>
								<line x1="3" y1="12" x2="13" y2="12"/>
							</svg>
						</button>
					</div>
				{/if}
			</div>
			{/if}
		</div>

		<div class="library-search-meta">
			{#if searchBusy}
				<span class="library-status">Searching…</span>
			{:else if isSearchMode}
				<span class="library-status">{searchSummary}</span>
				{#if searchTruncated && (activeTab === 'tracks' || activeTab === 'liked' || activeTab === 'all')}
					<button
						class="filter-pill"
						disabled={searchLoadingMore}
						onclick={() => void loadMoreSearchResults()}
					>
						{searchLoadingMore ? 'Loading…' : 'Show more'}
					</button>
				{/if}
				<button class="filter-pill" onclick={() => (searchQuery.set(''))}>Clear</button>
			{/if}
		</div>
	</div>


	{#if searchError}
		<div class="batch-feedback error glass">{searchError}</div>
	{/if}

	{#if isSearchMode && searchUnmatchedGenres.length > 0}
		<div class="batch-feedback error glass">
			No genre named {searchUnmatchedGenres.map((g) => `"${g}"`).join(', ')} - nothing was
			filtered by it. Genre filters match library genre names or slugs (see the Genres page).
		</div>
	{/if}

	{#if $selectedTrackIds.size > 0 || $selectedAlbumIds.size > 0}
		<SelectionBar summary={selectionSummary} sticky={true}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={clearSelection}>Clear</button>
				<button class="btn btn-glass" disabled={batchBusy === 'delete'} onclick={confirmDeleteSelection}>
					{batchBusy === 'delete' ? 'Deleting…' : 'Delete'}
				</button>
				{#if activeTab === 'tracks' || activeTab === 'liked'}
					<select bind:value={selectedPlaylistId} class="batch-select">
						{#each playlists as playlist}
							<option value={playlist.id}>{playlist.name}</option>
						{/each}
					</select>
					<button class="btn btn-glass" disabled={!selectedPlaylistId || batchBusy === 'playlist'} onclick={handleBatchAddToPlaylist}>
						{batchBusy === 'playlist' ? 'Adding…' : 'Add to playlist'}
					</button>
					<select bind:value={selectedGenreId} class="batch-select">
						{#each genres as genre}
							<option value={genre.id}>{genre.name}</option>
						{/each}
					</select>
					<button class="btn btn-glass" disabled={!selectedGenreId || batchBusy === 'genre'} onclick={handleBatchSetGenre}>
						{batchBusy === 'genre' ? 'Assigning…' : 'Set genre'}
					</button>
				{/if}
			{/snippet}
		</SelectionBar>
	{/if}

	{#if batchMessage}
		<div class="batch-feedback success glass">
			<span>{batchMessage}</span>
			{#if pendingUndo.tracks.length > 0 || pendingUndo.albums.length > 0}
				<button class="btn btn-glass" disabled={undoBusy} onclick={undoDelete}>
					{undoBusy ? 'Restoring…' : 'Undo'}
				</button>
			{/if}
		</div>
	{/if}

	{#if batchError}
		<div class="batch-feedback error glass">{batchError}</div>
	{/if}

	{#if $isLoading}
		<div class="loading"><div class="spinner"></div><span>Loading library…</span></div>

	{:else if activeTab === 'all' && isSearchMode}
		<div class="library-search-results">
			{#if allSearchArtists.length > 0}
				<section class="library-search-section">
					<div class="section-header-row">
						<h3 class="section-label">Artists ({allSearchArtists.length})</h3>
						{#if allSearchArtists.length > ALL_SEARCH_ARTIST_PREVIEW_LIMIT}
							<button class="view-all-link" onclick={() => switchTab('artists')}>View all →</button>
						{/if}
					</div>
					<div class="artist-grid search-preview-grid">
						{#each allSearchArtistPreview as artist (artist.id)}
							{@const fallbackSrc = artistArtworkById.get(artist.id)}
							<button
								class="artist-card"
								onclick={() => void goto(`/artists/${artist.id}`)}
								oncontextmenu={(e) => {
									e.preventDefault();
									e.stopPropagation();
									openContextMenu(e, buildArtistMenu(artist, { isLocal: true, hideOpen: true }), artist.name);
								}}
								title="Open {artist.name}"
								use:lazyTidalArt={{
									enabled: !artistLazyArt[artist.id] && !fallbackSrc,
									query: { artist: artist.name },
									onResolve: (url) => (artistLazyArt[artist.id] = url),
								}}
							>
								<div class="artist-photo">
									<ArtworkImage
										className="artist-photo-img"
										src={artistImageSources(artist.photo_url, artistLazyArt[artist.id], fallbackSrc)}
										alt={artist.name}
										size={320}
										fallbackText={artist.name.charAt(0).toUpperCase()}
									/>
								</div>
								<span class="artist-name">{artist.name}</span>
							</button>
						{/each}
					</div>
				</section>
			{/if}

			{#if visibleAlbums.length > 0}
				<section class="library-search-section">
					<div class="section-header-row">
						<h3 class="section-label">Albums ({visibleAlbums.length})</h3>
						{#if visibleAlbums.length > ALL_SEARCH_ALBUM_PREVIEW_LIMIT}
							<button class="view-all-link" onclick={() => switchTab('albums')}>View all →</button>
						{/if}
					</div>
					<div class="album-grid search-preview-grid">
						{#each allSearchAlbumPreview as album (album.id)}
							{@const albumKey = `album-${album.id}`}
							{@const albumArt = album.artwork_url ?? lazyArt[albumKey] ?? null}
							<div
								class="album-card"
								role="button"
								tabindex="0"
								onclick={() => void openAlbumDetail(album)}
								oncontextmenu={(event) => {
									event.preventDefault();
									event.stopPropagation();
									openContextMenu(event, buildAlbumMenu(album, {
										isLocal: true,
										addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
											const { tracks: t } = await cachedApi.getAlbumTracks(album.id);
											return t.map(tr => tr.id);
										}),
									}), album.title);
								}}
								onkeydown={(event) => runOnActivation(event, () => void openAlbumDetail(album))}
								use:lazyTidalArt={{
									enabled: !album.artwork_url && !lazyArt[albumKey],
									query: { artist: album.artist_name, title: album.title },
									onResolve: (url) => (lazyArt[albumKey] = url),
								}}
							>
								<div class="album-art">
									<ArtworkImage
										className="album-art-img"
										src={albumArt}
										alt={album.title}
										size={320}
										tint={true}
										fallbackText={album.title.slice(0, 2).toUpperCase()}
									/>
									<button
										class="art-play-btn"
										aria-label="Play {album.title}"
										onclick={(event) => void playAlbum(album.id, event)}
									>
										<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg>
									</button>
								</div>
								<div class="album-meta">
									<span class="album-title">{album.title}</span>
									<span class="album-artist">{album.artist_name ?? 'Unknown'}</span>
									<div class="album-chips">
										{#if album.year}<span class="album-chip">{album.year}</span>{/if}
										{#if album.release_type}<span class="album-chip">{album.release_type}</span>{/if}
									</div>
								</div>
							</div>
						{/each}
					</div>
				</section>
			{/if}

			{#if visibleTracks.length > 0}
				<section class="library-search-section">
					<div class="section-header-row">
						<h3 class="section-label">Tracks ({visibleTracks.length})</h3>
						{#if visibleTracks.length > ALL_SEARCH_TRACK_PREVIEW_LIMIT}
							<button class="view-all-link" onclick={() => switchTab('tracks')}>View all →</button>
						{/if}
					</div>
					<div class="home-track-list">
						{#each allSearchTrackPreview as track (track.id)}
							{@const trackKey = `track-${track.id}`}
							{@const trackArt = track.artwork_url ?? lazyArt[trackKey] ?? null}
							<!-- svelte-ignore a11y_click_events_have_key_events -->
							<div
								class="home-track-row"
								class:playing={$currentTrack?.id === track.id && $isPlaying}
								role="button"
								onclick={() => void playTrack(track)}
								oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, buildTrackMenu(track)); }}
								tabindex="0"
								onkeydown={(e) => e.key === 'Enter' && void playTrack(track)}
								use:lazyTidalArt={{
									enabled: !track.artwork_url && !lazyArt[trackKey],
									query: { artist: track.artist_name, title: track.title },
									onResolve: (url) => (lazyArt[trackKey] = url),
								}}
							>
								<div class="ht-art" class:ht-art--fallback={!trackArt}>
									<ArtworkImage
										className="ht-art-img"
										src={trackArt}
										size={320}
										fallbackText={track.title.slice(0, 2).toUpperCase()}
										decorative={true}
									/>
								</div>
								<div class="ht-meta">
									<span class="ht-title">{track.title}</span>
									<span class="ht-sub">{track.artist_name ?? ''}{track.album_title ? ` - ${track.album_title}` : ''}</span>
								</div>
								<span class="ht-duration">{formatTrackDuration(track.duration_ms)}</span>
								<div class="ht-actions">
									<button
										class="btn-icon"
										title="View details"
										onclick={(e) => { e.stopPropagation(); void openTrackDetail(track); }}
										aria-label="View details"
									>
										i
									</button>
									<button
										class="btn-icon"
										title="Add to queue"
										onclick={(e) => { e.stopPropagation(); void addTrackToQueue(track.id); }}
										aria-label="Add to queue"
									>
										<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
											<line x1="2" y1="4" x2="14" y2="4"/>
											<line x1="2" y1="8" x2="14" y2="8"/>
											<line x1="2" y1="12" x2="10" y2="12"/>
											<line x1="13" y1="10" x2="13" y2="16"/>
											<line x1="10" y1="13" x2="16" y2="13"/>
										</svg>
									</button>
								</div>
							</div>
						{/each}
					</div>
				</section>
			{/if}

			{#if allSearchTotal === 0}
				<EmptyState title="No library matches" copy="Try a different artist, album, or track name." />
			{/if}
		</div>

	{:else if activeTab === 'all'}
		<div class="library-home">
			{#if heroArtists.length > 0}
				<LibraryHero
					artists={heroArtists}
					onPlayAll={playAllForArtist}
					onShuffle={shuffleArtist}
					onArtistClick={handleHomeArtistClick}
					onContextMenu={handleHomeArtistContextMenu}
				/>
			{:else if $isLoading}
				<div class="home-loading">Loading your library…</div>
			{/if}

			{#if homeMuralPanels.length > 0}
				<section class="home-mural-grid" aria-label="Library suggestion panels">
					{#each homeMuralPanels as panel, i (panel.id)}
						<article class="home-mural-panel" aria-label={panel.label} style={`--mural-index: ${i}`}>
							<div class="home-mural-bg">
								{#each panel.items as item (`${panel.id}-${item.kind}-${item.id}`)}
									{@const muralArt = muralItemArtwork(item)}
									<button
										class="home-mural-tile"
										class:home-mural-tile--album={item.kind === 'album'}
										type="button"
										onclick={() => openHomeMuralItem(item, panel)}
										oncontextmenu={(event) => openHomeMuralItemContextMenu(event, item)}
										aria-label={`${item.kind === 'track' ? 'Play' : 'Open'} ${item.title}`}
										title={`${item.title}${item.subtitle ? ` - ${item.subtitle}` : ''}`}
										use:lazyTidalArt={{
											enabled: muralArt === null,
											query: muralItemLazyQuery(item),
											onResolve: (url) => (lazyArt[muralItemKey(item)] = url),
										}}
									>
										<ArtworkImage
											className="home-mural-art"
											src={muralArt}
											size={320}
											fallbackText={fallbackLetters(item.title)}
											decorative={true}
											loading="eager"
											fadeIn={true}
										/>
									</button>
								{/each}
							</div>
							<div class="home-mural-shade"></div>
							<div class="home-mural-copy">
								<span class="home-mural-caption">{panel.caption}</span>
								<h3 class="home-mural-title">{panel.label}</h3>
								<span class="home-mural-count">{panel.items.length} picks</span>
							</div>
						</article>
					{/each}
				</section>
			{/if}

			{#if recentArtists.length > 0}
				<section class="home-section">
					<h3 class="section-label">Recently Played Artists</h3>
					<ArtistCarousel
						artists={recentArtists}
						onArtistClick={handleHomeArtistClick}
						onContextMenu={handleHomeArtistContextMenu}
					/>
				</section>
			{/if}

			{#if recentAlbums.length > 0}
				<section class="home-section">
					<h3 class="section-label">Recently Added</h3>
					<AlbumCarousel
						albums={recentAlbums}
						onAlbumClick={handleHomeAlbumClick}
						onContextMenu={handleHomeAlbumContextMenu}
						onArtistClick={handleHomeArtistClick}
						onArtistContextMenu={handleHomeArtistContextMenu}
					/>
				</section>
			{/if}

			{#if recentTracks.length > 0}
				<section class="home-section">
					<div class="section-header-row">
						<h3 class="section-label">Recent Tracks</h3>
						<button class="view-all-link" onclick={() => void goto('/history')}>View all →</button>
					</div>
					<div class="home-track-list">
						{#each recentTracks as track (track.id)}
							{@const trackKey = `track-${track.id}`}
							{@const trackArt = track.artwork_url ?? lazyArt[trackKey] ?? null}
							<!-- svelte-ignore a11y_click_events_have_key_events -->
							<div
								class="home-track-row"
								class:playing={$currentTrack?.id === track.id && $isPlaying}
								role="button"
								onclick={() => void playTrackNow(track.id)}
								oncontextmenu={(e) => { e.preventDefault(); e.stopPropagation(); openContextMenu(e, buildTrackMenu(track)); }}
								tabindex="0"
								onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
								use:lazyTidalArt={{
									enabled: !track.artwork_url && !lazyArt[trackKey],
									query: { artist: track.artist_name, title: track.title },
									onResolve: (url) => (lazyArt[trackKey] = url),
								}}
							>
								<div class="ht-art" class:ht-art--fallback={!trackArt}>
									<ArtworkImage
										className="ht-art-img"
										src={trackArt}
										size={320}
										fallbackText={track.title.slice(0, 2).toUpperCase()}
										decorative={true}
									/>
								</div>
								<div class="ht-meta">
									<span class="ht-title">{track.title}</span>
									<span class="ht-sub">{track.artist_name ?? ''}{track.album_title ? ` - ${track.album_title}` : ''}</span>
								</div>
								<span class="ht-duration">{formatTrackDuration(track.duration_ms)}</span>
								<div class="ht-actions">
									<button
										class="btn-icon"
										title="Add to queue"
										onclick={(e) => { e.stopPropagation(); void addTrackToQueue(track.id); }}
										aria-label="Add to queue"
									>
										<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">
											<line x1="2" y1="4" x2="14" y2="4"/>
											<line x1="2" y1="8" x2="14" y2="8"/>
											<line x1="2" y1="12" x2="10" y2="12"/>
											<line x1="13" y1="10" x2="13" y2="16"/>
											<line x1="10" y1="13" x2="16" y2="13"/>
										</svg>
									</button>
								</div>
							</div>
						{/each}
					</div>
				</section>
			{/if}
		</div>

	{:else if activeTab === 'albums'}
		{#if decadeOptions.length > 1}
			<div class="decade-strip">
				<button class="decade-chip" class:active={activeDecade === null} onclick={() => selectDecade(null)}>All</button>
				{#each decadeOptions as decade (decade)}
					<button
						class="decade-chip"
						class:active={activeDecade === decade}
						onclick={() => selectDecade(decade)}
					>{decade}s</button>
				{/each}
			</div>
		{/if}
		<!-- Skeleton grid while the first page loads, so we never flash an empty state -->
		{#if $isLoading && visibleAlbums.length === 0 && !isSearchMode}
			<div class="album-grid" aria-hidden="true">
				{#each Array(18) as _, i (i)}
					<div class="album-card album-skeleton">
						<div class="album-art skeleton-shimmer"></div>
						<div class="album-meta">
							<span class="skeleton-shimmer skeleton-text" style="width: 78%"></span>
							<span class="skeleton-shimmer skeleton-text" style="width: 52%"></span>
						</div>
					</div>
				{/each}
			</div>
		{/if}
		<!-- Album Grid -->
		<div class="album-grid" class:album-list={$viewMode === 'list'}>
			{#each visibleAlbums as album (album.id)}
				{@const albumKey = `album-${album.id}`}
				{@const albumArt = album.artwork_url ?? lazyArt[albumKey] ?? null}
				<div
					class="album-card"
					class:selected={$selectedAlbumIds.has(album.id)}
					role="button"
					tabindex="0"
					aria-pressed={$selectedAlbumIds.has(album.id)}
					onclick={(event) => handleAlbumCardClick(album, event)}
					oncontextmenu={(event) => {
						event.preventDefault();
						event.stopPropagation();
						openContextMenu(event, buildAlbumMenu(album, {
							isLocal: true,
							addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
								const { tracks: t } = await cachedApi.getAlbumTracks(album.id);
								return t.map(tr => tr.id);
							}),
						}), album.title);
					}}
					onkeydown={(event) => handleAlbumCardKeydown(album.id, event)}
					use:lazyTidalArt={{
						enabled: !album.artwork_url && !lazyArt[albumKey],
						query: { artist: album.artist_name, title: album.title },
						onResolve: (url) => (lazyArt[albumKey] = url),
					}}
				>
					<div class="album-art">
						<ArtworkImage
							className="album-art-img"
							src={albumArt}
							alt={album.title}
							size={320}
							tint={true}
							fallbackText={album.title.slice(0, 2).toUpperCase()}
						/>
						<button
							class="art-play-btn"
							aria-label="Play {album.title}"
							onclick={(event) => void playAlbum(album.id, event)}
						>
							<svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg>
						</button>
					</div>
					<div class="album-meta">
						<span class="album-title">{album.title}</span>
						<span class="album-artist">{album.artist_name ?? 'Unknown'}</span>
						<div class="album-chips">
							{#if album.year}<span class="album-chip">{album.year}</span>{/if}
							{#if album.release_type}<span class="album-chip">{album.release_type}</span>{/if}
						</div>
					</div>
					<div class="album-actions">
						<button
							class="menu-trigger"
							aria-label="Album actions"
							onclick={(event) => {
								event.preventDefault();
								event.stopPropagation();
								openMenuAtElement(event.currentTarget, buildAlbumMenu(album, {
									isLocal: true,
									includeSelect: true,
									includeRemove: true,
									onSelect: () => updateAlbumSelection(album.id, false, false),
									onRemove: () => void removeAlbumFromLibrary(album),
									addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
										const { tracks: t } = await cachedApi.getAlbumTracks(album.id);
										return t.map(tr => tr.id);
									}),
								}), album.title);
							}}
						>
							⋯
						</button>
					</div>
				</div>
			{/each}
		</div>

		{#if visibleAlbums.length === 0 && !$isLoading}
			<EmptyState title={isSearchMode ? 'No albums match this search' : 'No albums yet'} copy={isSearchMode ? 'Try a broader search term or switch to tracks.' : 'Connect TIDAL in Settings and run a sync to populate the library.'} />
		{:else if !isSearchMode && $albums.length < $totalAlbums}
			<div class="load-more-row">
				<span class="load-more-count">{$albums.length} of {$totalAlbums} albums</span>
				<button
					class="btn btn-glass"
					disabled={$isLoadingMore}
					onclick={() => loadAlbums(albumSortField, albumSortDir, PAGE_SIZE, $albums.length, activeDecade)}
				>
					{$isLoadingMore ? 'Loading…' : 'Load More'}
				</button>
			</div>
		{/if}

	{:else if activeTab === 'artists'}
		<!-- Artist Grid -->
		{#if artistsLoading && !isSearchMode}
			<div class="loading">Loading artists…</div>
		{:else if visibleArtists.length === 0}
			<EmptyState
				title={isSearchMode ? 'No artists match' : 'No artists yet'}
				copy={isSearchMode ? `Nothing in your library matches "${$searchQuery.trim()}". Try a different search.` : 'Sync your TIDAL library in Settings to populate artists.'}
			/>
		{:else}
			<div class="artist-grid">
				{#each visibleArtists as artist (artist.id)}
					{@const fallbackSrc = artistArtworkById.get(artist.id)}
					<button
						class="artist-card"
						onclick={() => void goto(`/artists/${artist.id}`)}
						oncontextmenu={(e) => {
							e.preventDefault();
							e.stopPropagation();
							openContextMenu(e, buildArtistMenu(artist, { isLocal: true, hideOpen: true }), artist.name);
						}}
						title="Open {artist.name}"
						use:lazyTidalArt={{
							enabled: !artistLazyArt[artist.id] && !fallbackSrc,
							query: { artist: artist.name },
							onResolve: (url) => (artistLazyArt[artist.id] = url),
						}}
					>
						<div class="artist-photo">
							<ArtworkImage
								className="artist-photo-img"
								src={artistImageSources(artist.photo_url, artistLazyArt[artist.id], fallbackSrc)}
								alt={artist.name}
								size={320}
								fallbackText={artist.name.charAt(0).toUpperCase()}
							/>
						</div>
						<span class="artist-name">{artist.name}</span>
					</button>
				{/each}
			</div>

			{#if !isSearchMode && !artistsExhausted && artists.length > 0}
				<div class="load-more-row">
					<span class="load-more-count">{artists.length} artists</span>
					<button
						class="btn btn-glass"
						disabled={artistsLoadingMore}
						onclick={() => void loadMoreArtists()}
					>
						{artistsLoadingMore ? 'Loading…' : 'Load More'}
					</button>
				</div>
			{/if}
		{/if}

	{:else if activeTab === 'tracks' || activeTab === 'liked'}
		<!-- Track List (shared between Tracks and Liked tabs - server filters via likedOnly) -->
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div class="track-list" role="list" onkeydown={handleTrackListKeydown}>
			<div class="track-header" style="grid-template-columns: {trackGridColumns}">
				<span class="col-num">#</span>
				<button
					type="button"
					class="header-sort col-title"
					class:sorted={$sortBy === 'title'}
					onclick={() => handleSort('title')}
					onkeydown={(event) => handleSortKeydown('title', event)}
				>
					Title <span class="sort-arrow">{$sortBy === 'title' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
				</button>
				<button
					type="button"
					class="header-sort col-artist"
					class:sorted={$sortBy === 'artist'}
					onclick={() => handleSort('artist')}
					onkeydown={(event) => handleSortKeydown('artist', event)}
				>
					Artist <span class="sort-arrow">{$sortBy === 'artist' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
				</button>
				<button
					type="button"
					class="header-sort col-album"
					class:sorted={$sortBy === 'album'}
					onclick={() => handleSort('album')}
					onkeydown={(event) => handleSortKeydown('album', event)}
				>
					Album <span class="sort-arrow">{$sortBy === 'album' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
				</button>
				{#if showQualityColumn}
					<span class="col-quality">Quality</span>
				{/if}
				{#if showPlaysColumn}
					<button
						type="button"
						class="header-sort col-plays"
						class:sorted={$sortBy === 'play_count'}
						onclick={() => handleSort('play_count')}
						onkeydown={(event) => handleSortKeydown('play_count', event)}
					>
						Plays <span class="sort-arrow">{$sortBy === 'play_count' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				{#if showDateColumn}
					<button
						type="button"
						class="header-sort col-date"
						class:sorted={$sortBy === 'date_added'}
						onclick={() => handleSort('date_added')}
						onkeydown={(event) => handleSortKeydown('date_added', event)}
					>
						Date Added <span class="sort-arrow">{$sortBy === 'date_added' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				{#if showDateColumn}
					<button
						type="button"
						class="header-sort col-date"
						class:sorted={$sortBy === 'last_played_at'}
						onclick={() => handleSort('last_played_at')}
						onkeydown={(event) => handleSortKeydown('last_played_at', event)}
					>
						Last Played <span class="sort-arrow">{$sortBy === 'last_played_at' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				{#if showBpmColumn}
					<button
						type="button"
						class="header-sort col-bpm"
						class:sorted={$sortBy === 'bpm'}
						onclick={() => handleSort('bpm')}
						onkeydown={(event) => handleSortKeydown('bpm', event)}
					>
						BPM <span class="sort-arrow">{$sortBy === 'bpm' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				{#if showKeyColumn}
					<span class="col-key">Key</span>
				{/if}
				{#if showEnergyColumn}
					<button
						type="button"
						class="header-sort col-energy"
						class:sorted={$sortBy === 'energy'}
						onclick={() => handleSort('energy')}
						onkeydown={(event) => handleSortKeydown('energy', event)}
					>
						Energy <span class="sort-arrow">{$sortBy === 'energy' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				{#if showDanceColumn}
					<button
						type="button"
						class="header-sort col-dance"
						class:sorted={$sortBy === 'danceability'}
						onclick={() => handleSort('danceability')}
						onkeydown={(event) => handleSortKeydown('danceability', event)}
					>
						Dance <span class="sort-arrow">{$sortBy === 'danceability' ? ($sortDir === 'asc' ? '↑' : '↓') : '⇅'}</span>
					</button>
				{/if}
				<span class="col-duration">Duration</span>
				<span class="col-actions"></span>
			</div>

			<div class="track-rows" bind:this={trackRowsEl}>
			<div class="vlist-spacer" style="height: {vlistTopPad}px" aria-hidden="true"></div>
			{#each vlistTracks as track, vi (track.id)}
				{@const i = vlistStart + vi}
				<div
					class="track-row"
					class:selected={$selectedTrackIds.has(track.id)}
					class:playing={$currentTrack?.id === track.id}
					class:cursor={cursorIndex === i}
					role="button"
					tabindex="0"
					aria-pressed={$selectedTrackIds.has(track.id)}
					data-cursor-idx={i}
					style="grid-template-columns: {trackGridColumns}"
					ondblclick={() => void playTrack(track)}
					onclick={(event) => handleTrackRowClick(track.id, event)}
					oncontextmenu={(event) => { event.preventDefault(); event.stopPropagation(); openContextMenu(event, buildTrackMenu(track)); }}
					onkeydown={(event) => handleTrackRowKeydown(track.id, event)}
				>
					<span class="col-num">
						{#if $currentTrack?.id === track.id && $isPlaying}
							<span class="playing-indicator">▶</span>
						{:else}
							<button
								class="track-play-num"
								aria-label="Play {track.title}"
								onclick={(event) => { event.stopPropagation(); void playTrack(track); }}
							>
								<span class="track-num-label">{i + 1}</span>
								<span class="track-num-play">▶</span>
							</button>
						{/if}
					</span>
					<span class="col-title">
						<span class="track-title">{track.title}</span>
						{#if track.camelot_key}
							<span class="camelot-badge-inline">{track.camelot_key}</span>
						{/if}
						{#if track.bpm}
							<span class="bpm-inline">{Math.round(track.bpm)}</span>
						{/if}
					</span>
					<span class="col-artist">
						{#if track.artist_name && track.artist_id != null}
							<a
								href={`/artists/${track.artist_id}`}
								class="subtitle-link"
								onclick={(e) => e.stopPropagation()}
								oncontextmenu={(e) => handleRowArtistContextMenu(e, track)}
							>{track.artist_name}</a>
						{:else}
							{track.artist_name ?? 'Unknown'}
						{/if}
					</span>
					<span class="col-album">
						{#if track.album_title && track.album_id != null}
							<a
								href={`/albums/${track.album_id}`}
								class="subtitle-link"
								onclick={(e) => e.stopPropagation()}
								oncontextmenu={(e) => handleRowAlbumContextMenu(e, track)}
							>{track.album_title}</a>
						{:else}
							{track.album_title ?? ''}
						{/if}
					</span>
					{#if showQualityColumn}
						<span class="col-quality">
							{#if track.best_quality}
								<span class="quality-badge {getQualityClass(track.best_quality)}">
									{track.best_quality.replace(/_/g, ' ')}
								</span>
							{/if}
						</span>
					{/if}
					{#if showPlaysColumn}
						<span class="col-plays">
							<span class="plays-count">{track.play_count > 0 ? track.play_count.toLocaleString() : '-'}</span>
						</span>
					{/if}
					{#if showDateColumn}
						<span class="col-date">
							<span class="date-added">{track.date_added ? formatDateShort(track.date_added) : '-'}</span>
						</span>
						<span class="col-date">
							<span class="last-played">{track.last_played_at ? formatDateShort(track.last_played_at) : '-'}</span>
						</span>
					{/if}
					{#if showBpmColumn}
						<span class="col-bpm">
							<span class="bpm-value">{track.bpm ? Math.round(track.bpm) : '-'}</span>
						</span>
					{/if}
					{#if showKeyColumn}
						<span class="col-key">
							{#if track.camelot_key}
								<span class="camelot-badge">{track.camelot_key}</span>
							{:else}
								<span>-</span>
							{/if}
						</span>
					{/if}
					{#if showEnergyColumn}
						<span class="col-energy">
							{#if track.energy != null}
								<span class="mini-bar">
									<span class="mini-bar-fill" style="width: {track.energy * 100}%"></span>
								</span>
								<span class="mini-bar-label">{track.energy.toFixed(2)}</span>
							{:else}
								<span>-</span>
							{/if}
						</span>
					{/if}
					{#if showDanceColumn}
						<span class="col-dance">
							{#if track.danceability != null}
								<span class="mini-bar">
									<span class="mini-bar-fill dance" style="width: {track.danceability * 100}%"></span>
								</span>
								<span class="mini-bar-label">{track.danceability.toFixed(2)}</span>
							{:else}
								<span>-</span>
							{/if}
						</span>
					{/if}
					<span class="col-duration">{formatTrackDuration(track.duration_ms)}</span>
					<span class="col-actions">
						<button class="detail-btn" title="View details" onclick={(event) => { event.stopPropagation(); void openTrackDetail(track); }}>ℹ</button>
						<button class="menu-trigger" aria-label="Track actions" onclick={(event) => toggleTrackMenu(track.id, event)}>
							⋯
						</button>
						{#if activeTrackMenuId === track.id}
							<div class="item-menu track-menu" role="menu" tabindex="-1" onmousedown={(event) => event.stopPropagation()}>
								<button class="menu-item" onclick={(event) => void playTrackFromMenu(track, event)}>
									Play Track
								</button>
								<button class="menu-item" onclick={(event) => void queueTrack(track, event)}>
									Queue Track
								</button>
								<button class="menu-item" onclick={(event) => { event.stopPropagation(); void openTrackDetail(track); closeMenus(); }}>
									View Details
								</button>
								<button class="menu-item secondary" onclick={(event) => selectTrackFromMenu(track.id, event)}>
									Select Track
								</button>
							</div>
						{/if}
					</span>
				</div>
			{/each}
			<div class="vlist-spacer" style="height: {vlistBottomPad}px" aria-hidden="true"></div>
			</div>
		</div>

		{#if visibleTracks.length === 0}
			<EmptyState title={isSearchMode ? 'No tracks match this search' : 'No tracks yet'} copy={isSearchMode ? 'Try a different artist, album, or track name.' : 'Connect TIDAL in Settings to sync your library.'} />
		{:else if !isSearchMode && $tracks.length < $totalTracks}
			<div class="load-more-row">
				<span class="load-more-count">{$tracks.length} of {$totalTracks} {activeTab === 'liked' ? 'liked tracks' : 'tracks'}</span>
				<button
					class="btn btn-glass"
					disabled={$isLoadingMore}
					onclick={() => loadTracks($sortBy, $sortDir, PAGE_SIZE, $tracks.length, activeTab === 'liked')}
				>
					{$isLoadingMore ? 'Loading…' : 'Load More'}
				</button>
			</div>
		{/if}
	{/if}

	{#if canLoadMore}
		<div bind:this={infiniteSentinel} class="infinite-sentinel" aria-hidden="true"></div>
	{/if}
</div>

<!-- ─── Album Detail Modal ─────────────────────── -->
{#if expandedAlbumId !== null && detailAlbum}
	<AlbumDetailPopup
		album={detailAlbum}
		tracks={detailAlbumTracksList}
		loading={detailAlbumLoading}
		onClose={() => { expandedAlbumId = null; detailAlbum = null; detailAlbumTracksList = []; }}
	/>
{/if}

<!-- ─── Track Detail Modal ─────────────────────── -->
{#if expandedTrackId !== null && detailTrack}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		class="modal-backdrop"
		role="presentation"
		onclick={() => { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }}
		use:portal
	>
		<div class="modal-panel glass-panel" onclick={(e) => e.stopPropagation()} role="dialog" tabindex="-1" aria-modal="true" aria-label={detailTrack.title}>
			<div class="modal-topbar">
				<button class="modal-close" aria-label="Close" onclick={() => { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }}>✕</button>
			</div>

			<div class="detail-track-hero">
				<ArtworkImage
					className="detail-track-art-large"
					src={detailTrack.artwork_url}
					alt={detailTrack.title}
					size={640}
					fallbackText={detailTrack.title.slice(0, 2).toUpperCase()}
					decorative={true}
				/>
				<div class="detail-track-info">
					<h2>{detailTrack.title}</h2>
					<p class="detail-artist">{detailTrack.artist_name ?? 'Unknown Artist'}</p>
					{#if detailTrack.album_title}<p class="detail-album-name">{detailTrack.album_title}</p>{/if}
					<div class="detail-meta-row">
						{#if detailTrack.best_quality}
							<span class="quality-badge {getQualityClass(detailTrack.best_quality)}">{detailTrack.best_quality.replace(/_/g, ' ')}</span>
						{/if}
						{#if detailTrack.fidelity_score > 0}<span class="detail-chip">Fidelity: {detailTrack.fidelity_score}</span>{/if}
						<span class="detail-chip">{detailTrack.source}</span>
					</div>
					<div class="detail-actions">
						<button class="btn btn-primary" onclick={() => void playTrackNow(detailTrack!.id)}>▶ Play</button>
						<button class="btn btn-glass" onclick={() => void addTrackToQueue(detailTrack!.id)}>+ Queue</button>
						<button class="btn btn-glass" onclick={() => { selectTrackIds([detailTrack!.id]); }}>Select</button>
					</div>
				</div>
			</div>

			<div class="detail-meta-grid">
				{#if detailTrack.isrc}
					<div class="meta-block">
						<span class="meta-label">ISRC</span>
						<span class="meta-value">{detailTrack.isrc}</span>
					</div>
				{/if}
				{#if detailTrack.date_added}
					<div class="meta-block">
						<span class="meta-label">Date Added</span>
						<span class="meta-value">{new Date(detailTrack.date_added).toLocaleDateString('en-US', { year: 'numeric', month: 'long', day: 'numeric' })}</span>
					</div>
				{/if}
				<div class="meta-block">
					<span class="meta-label">Duration</span>
					<span class="meta-value">{formatTrackDuration(detailTrack.duration_ms)}</span>
				</div>
				{#if detailTrack.disc_number && detailTrack.disc_number > 1}
					<div class="meta-block">
						<span class="meta-label">Disc</span>
						<span class="meta-value">{detailTrack.disc_number}</span>
					</div>
				{/if}
				{#if detailTrack.track_number}
					<div class="meta-block">
						<span class="meta-label">Track No.</span>
						<span class="meta-value">{detailTrack.track_number}</span>
					</div>
				{/if}
				<div class="meta-block">
					<span class="meta-label">Play Count</span>
					<span class="meta-value">{detailTrack.play_count}</span>
				</div>
				{#if detailTrack.last_played_at}
					<div class="meta-block">
						<span class="meta-label">Last Played</span>
						<span class="meta-value">{new Date(detailTrack.last_played_at).toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric' })}</span>
					</div>
				{/if}
				{#if detailTrack.tidal_id}
					<div class="meta-block">
						<span class="meta-label">TIDAL ID</span>
						<span class="meta-value">{detailTrack.tidal_id}</span>
					</div>
				{/if}
			</div>

			{#if detailTrack.album_id && detailAlbumTracks.length > 0}
				<div class="detail-album-tracks">
					<h3>From this album</h3>
					<div class="detail-track-list">
						{#each detailAlbumTracks as track, i (track.id)}
							<div
								class="detail-track-row"
								class:playing={$currentTrack?.id === track.id}
								class:active={track.id === detailTrack!.id}
								role="button"
								tabindex="0"
								onclick={() => void playTrackNow(track.id)}
								onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
							>
								<span class="detail-track-num">{i + 1}</span>
								<ArtworkImage
									className="detail-track-art"
									src={track.artwork_url}
									alt={track.title}
									size={320}
									fallbackText={track.title.slice(0, 2).toUpperCase()}
									decorative={true}
								/>
								<span class="detail-track-title">{track.title}</span>
								<span class="detail-track-artist">{track.artist_name ?? ''}</span>
								<span class="detail-track-duration">{formatTrackDuration(track.duration_ms)}</span>
								<button class="detail-track-queue" onclick={(e) => { e.stopPropagation(); void addTrackToQueue(track.id); }}>+</button>
							</div>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	</div>
{/if}

<style>
	/* Every toolbar control here - the filter pills, the album sort segment,
	   the view toggle and the decade chips - sizes off the app-wide
	   --control-h token in app.css, so the rows under the search field read as
	   one system and match the other pages' pill rows. */
	.library {
		padding-bottom: 8px;
	}

	/* ─── All / Home view ───────────────── */

	.library-home {
		display: flex;
		flex-direction: column;
		gap: 24px;
		padding: 8px 0 40px;
	}

	.library-search-results {
		display: flex;
		flex-direction: column;
		gap: var(--space-6);
		padding: var(--space-2) 0 var(--space-6);
	}

	.library-search-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.search-preview-grid {
		margin: 0;
	}

	.home-section {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.home-mural-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-4);
	}

	.home-mural-panel {
		position: relative;
		min-height: clamp(140px, 15vw, 210px);
		border-radius: var(--radius-md);
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		background: var(--panel-bg);
		/* Ease each panel in (lightly staggered) once its data arrives, so the
		   grid settles in gracefully instead of the panels snapping into place. */
		animation: home-mural-panel-in 360ms ease-out both;
		animation-delay: calc(var(--mural-index, 0) * 70ms);
	}

	@keyframes home-mural-panel-in {
		from {
			opacity: 0;
			transform: translateY(10px);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.home-mural-panel {
			animation: none;
		}
	}

	.home-mural-bg {
		position: absolute;
		inset: -7%;
		z-index: 0;
		display: grid;
		grid-template-columns: repeat(6, minmax(0, 1fr));
		grid-template-rows: repeat(2, minmax(0, 1fr));
		background: linear-gradient(120deg, var(--panel-bg), color-mix(in srgb, var(--accent-soft) 24%, transparent));
	}

	.home-mural-bg::after {
		content: '';
		position: absolute;
		inset: 0;
		background:
			radial-gradient(circle at 78% 42%, rgba(255,255,255,0.2), transparent 30%),
			linear-gradient(90deg, rgba(0,0,0,0.06), transparent 42%, rgba(0,0,0,0.02));
		pointer-events: none;
	}

	.home-mural-tile {
		appearance: none;
		position: relative;
		min-width: 0;
		min-height: 0;
		padding: 0;
		border: 0;
		background: var(--bg-raised);
		color: var(--text-primary);
		cursor: pointer;
		overflow: hidden;
		opacity: 0.96;
		filter: saturate(1.18) brightness(1.16);
		transform: skewX(-7deg) scaleX(1.08);
		transform-origin: center;
		transition:
			filter var(--motion-fast),
			opacity var(--motion-fast),
			transform var(--motion-base),
			box-shadow var(--motion-base);
	}

	.home-mural-tile::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(90deg, rgba(0,0,0,0.18), transparent 48%, rgba(0,0,0,0.2));
		opacity: 0.18;
		pointer-events: none;
	}

	.home-mural-tile:hover,
	.home-mural-tile:focus-visible {
		z-index: var(--z-raised);
		opacity: 1;
		filter: saturate(1.8) brightness(1.42);
		transform: skewX(-7deg) scaleX(1.08) scale(1.045);
		box-shadow:
			0 0 0 1px rgba(255,255,255,0.32),
			0 14px 30px rgba(0,0,0,0.32),
			0 0 24px color-mix(in srgb, var(--accent) 38%, transparent);
		outline: none;
	}

	.home-mural-tile :global(.home-mural-art) {
		display: block;
		width: 100%;
		height: 100%;
	}

	.home-mural-tile :global(.home-mural-art:not(.fallback)) {
		object-fit: cover;
		transform: skewX(7deg) scale(1.24);
		/* opacity here (not just transform) so the ArtworkImage fadeIn actually
		   eases in - this rule outranks the component's own transition. */
		transition: transform var(--motion-base), opacity 260ms ease-out;
	}

	.home-mural-tile:hover :global(.home-mural-art:not(.fallback)),
	.home-mural-tile:focus-visible :global(.home-mural-art:not(.fallback)) {
		transform: skewX(7deg) scale(1.34);
	}

	.home-mural-tile :global(.home-mural-art.fallback) {
		display: grid;
		place-items: center;
		background: linear-gradient(135deg, var(--bg-raised), color-mix(in srgb, var(--accent-soft) 28%, var(--bg-surface)));
		color: rgba(255,255,255,0.78);
		transform: skewX(7deg) scale(1.08);
	}

	.home-mural-tile :global(.home-mural-art.fallback span) {
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
	}

	.home-mural-shade {
		position: absolute;
		inset: 0;
		z-index: var(--z-base);
		background: linear-gradient(90deg, rgba(0,0,0,0.68) 0%, rgba(0,0,0,0.3) 42%, rgba(0,0,0,0.06) 78%, transparent 100%);
		pointer-events: none;
	}

	.home-mural-copy {
		position: relative;
		z-index: calc(var(--z-base) + 1);
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		gap: var(--space-1);
		min-height: clamp(140px, 15vw, 210px);
		max-width: min(22rem, 70%);
		padding: var(--space-4);
		text-shadow: 0 2px 18px rgba(0,0,0,0.62);
		pointer-events: none;
	}

	.home-mural-caption,
	.home-mural-count {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0;
		text-transform: uppercase;
		color: var(--accent);
	}

	.home-mural-title {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
	}

	.home-mural-count {
		color: var(--text-secondary);
		text-transform: none;
	}

	.section-label {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
		color: var(--accent);
		margin: 0;
	}

	.section-header-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
	}

	.view-all-link {
		font-size: var(--font-size-xs);
		color: var(--text-secondary, rgba(255,255,255,0.5));
		background: none;
		border: none;
		cursor: pointer;
		padding: 0;
		transition: color 0.15s;
	}

	.view-all-link:hover { color: var(--text-primary, #fff); }

	.home-track-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
	}

	.home-track-row {
		display: grid;
		grid-template-columns: 38px 1fr auto auto;
		gap: 12px;
		align-items: center;
		padding: 6px 8px;
		border-radius: 6px;
		cursor: pointer;
		transition: background 0.1s;
	}

	.home-track-row:hover { background: var(--bg-hover); }

	.home-track-row.playing .ht-title { color: var(--accent); }

	.ht-art {
		width: 36px;
		height: 36px;
		border-radius: 4px;
		background-size: cover;
		background-position: center;
		overflow: hidden;
	}

	.ht-art :global(.ht-art-img) {
		width: 100%;
		height: 100%;
	}

	.ht-art :global(.ht-art-img:not(.fallback)) {
		display: block;
		object-fit: cover;
	}

	.ht-art--fallback {
		background: var(--bg-hover);
	}

	.ht-art :global(.ht-art-img.fallback) {
		display: grid;
		place-items: center;
		background: var(--bg-hover);
		color: var(--text-tertiary);
	}

	.ht-art :global(.ht-art-img.fallback span) {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.ht-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.ht-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary, #fff);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.ht-sub {
		font-size: var(--font-size-xs);
		color: var(--text-secondary, rgba(255,255,255,0.5));
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.ht-duration {
		font-size: var(--font-size-xs);
		color: var(--text-muted, rgba(255,255,255,0.4));
		font-variant-numeric: tabular-nums;
	}

	.ht-actions {
		opacity: 0;
		transition: opacity 0.15s;
	}

	.home-track-row:hover .ht-actions { opacity: 1; }

	.btn-icon {
		background: none;
		border: none;
		cursor: pointer;
		color: var(--text-secondary, rgba(255,255,255,0.5));
		padding: 4px;
		border-radius: 4px;
		display: flex;
		align-items: center;
		transition: color 0.15s;
	}

	.btn-icon:hover { color: var(--text-primary, #fff); }

	.home-loading {
		color: var(--text-secondary, rgba(255,255,255,0.5));
		font-size: var(--font-size-sm);
		padding: 40px;
		text-align: center;
	}

	/* ─── Loading ───────────────────────── */

	.loading {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		padding: 48px 0;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
	}

	.spinner {
		width: 24px;
		height: 24px;
		border: 2px solid var(--border-subtle);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.6s linear infinite;
	}

	.spinner-sm {
		width: 16px;
		height: 16px;
		border-width: 2px;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
	}

	/* ─── Hero Section ──────────────────── */

	.library-hero {
		padding: 18px 20px;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.library-hero-main {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 18px;
	}

	.library-hero-copy {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	.library-hero-heading {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: 10px;
	}

	.library-mode-pill,
	.library-stat-chip {
		display: inline-flex;
		align-items: center;
		padding: 6px 10px;
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.library-mode-pill {
		color: var(--text-primary);
		background: rgba(124, 128, 255, 0.12);
		border-color: rgba(124, 128, 255, 0.22);
	}

	.decade-strip {
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
		margin-bottom: 16px;
	}
	/* Match the primary tab pills (.filter-pill) so the Albums toolbar reads as
	   one system - pill radius, subtle border, bg-hover, accent when active. */
	.decade-chip {
		display: inline-flex;
		align-items: center;
		height: var(--control-h);
		padding: 0 14px;
		border-radius: 999px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		border: 1px solid var(--border-subtle);
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		transition: background 0.15s, color 0.15s, border-color 0.15s;
	}
	.decade-chip:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}
	.decade-chip.active {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.library-hero-subtitle {
		max-width: 48ch;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
	}

	.library-hero-actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 10px;
		flex-wrap: wrap;
	}

	.library-hero-stats {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.library-stat-chip.emphasis {
		color: var(--text-primary);
		background: rgba(255, 255, 255, 0.06);
	}

	/* ─── Toolbar ───────────────────────── */

	.library-toolbar {
		display: grid;
		grid-template-columns: minmax(220px, 420px) 1fr;
		gap: var(--gap);
		align-items: center;
		padding: 12px 14px;
		margin-bottom: var(--gap);
	}

	.toolbar-meta {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 10px;
		flex-wrap: wrap;
	}

	.toolbar-note {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	/* ─── New DSP Columns ───────────────────────── */

	.col-bpm, .col-key, .col-energy, .col-dance {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		text-align: center;
	}

	.bpm-value {
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
	}

	.camelot-badge {
		display: inline-block;
		padding: 2px 6px;
		border-radius: 4px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		font-family: var(--font-mono);
	}

	.camelot-badge-inline {
		display: inline-block;
		padding: 1px 5px;
		margin-left: 4px;
		border-radius: 4px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		font-family: var(--font-mono);
		vertical-align: middle;
	}

	.bpm-inline {
		display: inline-block;
		padding: 1px 5px;
		margin-left: 4px;
		border-radius: 4px;
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		vertical-align: middle;
	}

	.mini-bar {
		display: inline-block;
		width: 40px;
		height: 4px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.08);
		overflow: hidden;
		vertical-align: middle;
	}

	.mini-bar-fill {
		display: block;
		height: 100%;
		border-radius: 2px;
		background: linear-gradient(90deg, var(--accent), #b0b3ff);
		transition: width 200ms ease;
	}

	.mini-bar-fill.dance {
		background: linear-gradient(90deg, #06d6a0, #4cc9f0);
	}

	.mini-bar-label {
		display: inline-block;
		margin-left: 3px;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
		vertical-align: middle;
	}

	.library-search-shell {
		display: flex;
		flex-direction: column;
		gap: 10px;
		width: 100%;
		max-width: var(--content-width);
		margin: 0 auto var(--space-5);
		padding: 0 4px;
	}

	.library-status {
		font-size: var(--font-size-xs);
		color: var(--text-muted, rgba(255,255,255,0.4));
	}

	.library-search-meta {
		min-height: 28px;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		width: 100%;
		max-width: 720px;
		margin: -4px auto 0;
		text-align: center;
	}

	/* Two centered rows: the category tabs never move, and whatever the tab
	   brings with it (play controls, sort, view layout) sits on its own row
	   underneath so nothing overflows sideways or drifts off the baseline. */
	.filter-pills {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
	}

	.filter-pill-group,
	.filter-pill-actions {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		flex-wrap: wrap;
		max-width: 100%;
	}

	.play-controls {
		display: inline-flex;
		align-items: center;
		gap: 6px;
	}

	.filter-pill--accent {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.filter-pill--accent:hover {
		background: var(--accent);
		filter: brightness(1.08);
		color: #fff;
	}

	.filter-pill {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: var(--control-h);
		padding: 0 14px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle, rgba(255,255,255,0.1));
		background: transparent;
		color: var(--text-secondary, rgba(255,255,255,0.6));
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background 0.15s, color 0.15s, border-color 0.15s;
		white-space: nowrap;
	}

	.pill-glyph {
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.filter-pill:hover {
		background: var(--bg-hover);
		color: var(--text-primary, #fff);
	}

	.filter-pill.active {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.album-sort {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		height: var(--control-h);
		padding: 0 2px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--border-subtle);
	}

	.album-sort-label {
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-tertiary);
		padding: 0 6px 0 10px;
	}

	.album-sort-btn {
		display: inline-flex;
		align-items: center;
		gap: 3px;
		height: calc(var(--control-h) - 6px);
		padding: 0 10px;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--text-tertiary);
		font-family: inherit;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background 140ms ease, color 140ms ease;
	}

	.album-sort-btn:hover {
		color: var(--text-primary);
		background: rgba(255, 255, 255, 0.06);
	}

	.album-sort-btn.active {
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.album-sort-arrow {
		font-size: var(--font-size-2xs);
		color: var(--accent);
		line-height: 1;
	}

	.view-toggle {
		display: inline-flex;
		align-items: center;
		gap: 2px;
		height: var(--control-h);
		padding: 0 2px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--border-subtle);
	}

	.view-toggle-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: calc(var(--control-h) - 6px);
		height: calc(var(--control-h) - 6px);
		padding: 0;
		border: 0;
		border-radius: 999px;
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
		transition: background 140ms ease, color 140ms ease;
	}

	.view-toggle-btn:hover {
		color: var(--text-primary);
		background: rgba(255, 255, 255, 0.06);
	}

	.view-toggle-btn.active {
		background: var(--accent-soft);
		color: var(--accent);
	}

	/* ─── Batch Bar ─────────────────────── */

	.batch-select {
		min-width: 180px;
		max-width: 220px;
	}

	.batch-feedback {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--gap);
		padding: var(--gap-sm) var(--gap);
		margin-bottom: var(--gap);
	}

	.batch-feedback.success {
		border-color: rgba(91, 211, 154, 0.32);
	}

	.batch-feedback.error {
		border-color: rgba(255, 109, 109, 0.3);
		color: #ffb0b0;
	}

	/* ─── Detail Panel ──────────────────── */

	.detail-panel {
		padding: 24px;
		display: flex;
		flex-direction: column;
		gap: 24px;
		margin-bottom: var(--gap);
		animation: panel-slide 200ms ease-out both;
	}

	@keyframes panel-slide {
		from { opacity: 0; transform: translateY(8px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.detail-header {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.detail-back {
		align-self: flex-start;
		padding: 6px 14px;
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: all var(--motion-fast);
	}

	.detail-back:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
		border-color: var(--border-strong);
	}

	.detail-album-hero,
	.detail-track-hero {
		display: flex;
		gap: 20px;
		align-items: flex-start;
	}

	.detail-album-art,
	:global(.detail-track-art-large) {
		width: 140px;
		height: 140px;
		border-radius: var(--radius);
		object-fit: cover;
		flex-shrink: 0;
		box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
	}

	:global(.detail-track-art-large.fallback) {
		background: var(--accent-soft);
		display: flex;
		align-items: center;
		justify-content: center;
	}

	:global(.detail-track-art-large.fallback span) {
		font-size: var(--font-size-3xl);
		color: var(--accent-strong);
	}

	.detail-album-art.placeholder {
		background: var(--accent-soft);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-3xl);
		color: var(--accent-strong);
	}

	.detail-album-info,
	.detail-track-info {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.detail-track-info h2 {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0;
		line-height: var(--line-height-snug);
	}

	.detail-artist {
		color: var(--text-secondary);
		font-size: var(--font-size-md);
	}

	.detail-album-name {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.detail-meta-row {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin-top: 4px;
	}

	.detail-chip {
		display: inline-flex;
		align-items: center;
		padding: 4px 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--panel-border);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.detail-actions {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
	}

	.detail-meta-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
		gap: 12px;
	}

	.meta-block {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 12px 14px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.02);
		border: 1px solid rgba(255, 255, 255, 0.05);
	}

	.meta-label {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-muted);
	}

	.meta-value {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
		word-break: break-all;
	}

	.detail-album-tracks h3 {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		margin-bottom: 8px;
		color: var(--text-secondary);
	}

	.detail-loading {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 10px;
		padding: 32px 0;
		color: var(--text-secondary);
	}

	/* ─── Detail Track List ─────────────── */

	.detail-track-list {
		display: flex;
		flex-direction: column;
	}

	.detail-track-row {
		display: grid;
		grid-template-columns: 28px 40px 1fr auto auto 32px;
		gap: 10px;
		align-items: center;
		padding: 8px 10px;
		border-radius: var(--radius-xs);
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.detail-track-row:hover {
		background: rgba(255, 255, 255, 0.04);
	}

	.detail-track-row.playing {
		color: var(--accent);
	}

	.detail-track-row.active {
		background: var(--accent-soft);
	}

	.detail-track-num {
		text-align: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	:global(.detail-track-art) {
		width: 40px;
		height: 40px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
	}

	:global(.detail-track-art.fallback) {
		display: grid;
		place-items: center;
		background: var(--bg-raised);
		color: var(--text-tertiary);
	}

	:global(.detail-track-art.fallback span) {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.detail-track-title {
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.detail-track-artist {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 180px;
	}

	.detail-track-duration {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.detail-track-queue {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--panel-border);
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all var(--motion-fast);
	}

	.detail-track-queue:hover {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent);
	}

	@media (max-width: 1180px) {
		.library-hero-main {
			flex-direction: column;
		}

		.library-toolbar {
			grid-template-columns: 1fr;
		}

		.toolbar-meta {
			justify-content: flex-start;
		}

		.detail-album-hero,
		.detail-track-hero {
			flex-direction: column;
			align-items: center;
			text-align: center;
		}

		.detail-album-art,
		:global(.detail-track-art-large) {
			width: 120px;
			height: 120px;
		}

		.detail-meta-row {
			justify-content: center;
		}

		.detail-actions {
			justify-content: center;
		}
	}

	@media (max-width: 760px) {
		.home-mural-grid {
			grid-template-columns: 1fr;
		}

		.home-mural-bg {
			grid-template-columns: repeat(4, minmax(0, 1fr));
			grid-template-rows: repeat(3, minmax(0, 1fr));
		}

		.home-mural-copy {
			max-width: 82%;
		}

		.library-hero {
			padding: 16px;
			gap: 12px;
		}

		.library-hero-heading {
			align-items: flex-start;
			flex-direction: column;
			gap: 8px;
		}

		.library-hero-actions {
			width: 100%;
			justify-content: flex-start;
		}

		.filter-pills,
		.filter-pill-group--primary,
		.filter-pill-actions {
			width: 100%;
		}

		.filter-pill {
			flex: 1;
		}

		.batch-select {
			min-width: 100%;
			max-width: none;
		}

		.batch-feedback,
		.load-more-row {
			flex-direction: column;
			align-items: flex-start;
		}

		.detail-panel {
			padding: 16px;
			gap: 16px;
		}

		.detail-meta-grid {
			grid-template-columns: repeat(2, 1fr);
		}

		.detail-track-row {
			grid-template-columns: 24px 1fr auto 32px;
		}

		:global(.detail-track-art),
		.detail-track-artist {
			display: none;
		}
	}

	/* ─── Modal Overlay ─────────────────── */

	:global(.modal-backdrop) {
		position: fixed;
		inset: 0;
		z-index: 200;
		background: rgba(0, 0, 0, 0.6);
		backdrop-filter: blur(6px);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
		animation: backdrop-in 180ms ease both;
	}

	@keyframes backdrop-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	:global(.modal-panel) {
		width: 100%;
		max-width: 720px;
		max-height: 86vh;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 20px;
		padding: 24px;
		border-radius: var(--radius-lg);
		animation: modal-pop 220ms cubic-bezier(0.22, 1, 0.36, 1) both;
		scrollbar-width: thin;
	}

	@keyframes modal-pop {
		from { opacity: 0; transform: scale(0.96) translateY(10px); }
		to { opacity: 1; transform: scale(1) translateY(0); }
	}

	:global(.modal-topbar) {
		display: flex;
		justify-content: flex-end;
	}

	:global(.modal-close) {
		width: 32px;
		height: 32px;
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.1);
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		display: flex;
		align-items: center;
		justify-content: center;
		cursor: pointer;
		transition: all var(--motion-fast);
	}

	:global(.modal-close:hover) {
		background: rgba(255, 255, 255, 0.12);
		color: var(--text-primary);
	}

	/* ─── Album Grid ─────────────────────── */

	.album-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(158px, 1fr));
		gap: clamp(14px, 1.4vw, 22px) clamp(12px, 1vw, 18px);
		align-items: start;
	}

	@media (min-width: 1600px) {
		.album-grid {
			grid-template-columns: repeat(auto-fill, minmax(172px, 1fr));
		}
	}

	/* ─── Album Skeletons (first-load shimmer) ─────────────────────── */

	.album-skeleton {
		pointer-events: none;
	}

	.album-skeleton .album-art {
		box-shadow: none;
	}

	.skeleton-shimmer {
		background: linear-gradient(
			90deg,
			var(--bg-surface) 0%,
			var(--bg-hover) 50%,
			var(--bg-surface) 100%
		);
		background-size: 200% 100%;
		animation: album-skeleton-shimmer 1.4s ease-in-out infinite;
	}

	.skeleton-text {
		display: block;
		height: 11px;
		border-radius: 999px;
		margin-top: 7px;
	}

	@keyframes album-skeleton-shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}

	@media (prefers-reduced-motion: reduce) {
		.skeleton-shimmer { animation: none; }
	}

	/* ─── Album List Mode ────────────────── */

	/* Clean, borderless rows with per-row rounded hover (Search-list style) -
	   no container chrome, no hairline separators. */
	.album-grid.album-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.album-grid.album-list .album-card {
		display: grid;
		grid-template-columns: 44px minmax(0, 1fr) auto;
		gap: 12px;
		align-items: center;
		padding: 6px 10px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		box-shadow: none;
		transition: background var(--motion-fast);
	}

	.album-grid.album-list .album-card:hover {
		transform: none;
		box-shadow: none;
		background: var(--bg-hover);
	}

	.album-grid.album-list .album-card.selected {
		outline: none;
		outline-offset: 0;
		background: var(--accent-soft);
		box-shadow: inset 2px 0 0 var(--accent);
	}

	.album-grid.album-list .album-art {
		position: relative;
		width: 44px;
		height: 44px;
		aspect-ratio: unset;
		margin-bottom: 0;
		border-radius: var(--radius-sm);
		flex-shrink: 0;
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
	}

	/* Dim only the artwork on hover so the centered play icon stays crisp. */
	.album-grid.album-list .album-card:hover .album-art :global(.album-art-img) {
		filter: brightness(0.55);
		transition: filter var(--motion-fast);
	}

	/* Small centered play affordance over the thumbnail on row hover. */
	.album-grid.album-list .art-play-btn {
		display: grid;
		position: absolute;
		inset: 0;
		margin: auto;
		right: auto;
		bottom: auto;
		width: 26px;
		height: 26px;
		background: transparent;
		box-shadow: none;
		transform: none;
		opacity: 0;
	}

	.album-grid.album-list .art-play-btn svg {
		width: 13px;
		height: 13px;
		margin-left: 0;
		filter: drop-shadow(0 1px 2px rgba(0, 0, 0, 0.65));
	}

	.album-grid.album-list .album-card:hover .art-play-btn,
	.album-grid.album-list .album-card:focus-within .art-play-btn {
		opacity: 1;
		transform: none;
	}

	.album-grid.album-list .art-play-btn:hover {
		transform: scale(1.14);
		filter: none;
	}

	.album-grid.album-list .album-meta {
		padding: 0;
		min-width: 0;
	}

	.album-grid.album-list .album-chips {
		display: flex;
		margin-top: 3px;
	}

	/* Actions stay hidden until row hover, matching the grid tiles. */
	.album-grid.album-list .album-actions {
		position: static;
		opacity: 0;
		margin: 0;
		padding: 0;
		transition: opacity var(--motion-fast);
	}

	.album-grid.album-list .album-card:hover .album-actions,
	.album-grid.album-list .album-card:focus-within .album-actions {
		opacity: 1;
	}

	/* In a row the menu button is a light ghost icon, not a dark artwork overlay. */
	.album-grid.album-list .menu-trigger {
		width: 30px;
		height: 30px;
		border-radius: 50%;
		background: transparent;
		border: 1px solid transparent;
		color: var(--text-secondary);
	}

	.album-grid.album-list .menu-trigger:hover {
		background: rgba(255, 255, 255, 0.1);
		border-color: var(--panel-border);
		color: var(--text-primary);
	}

	.album-card {
		position: relative;
		padding: 0;
		border: 0;
		background: transparent;
		box-shadow: none;
		border-radius: var(--radius-md);
		cursor: pointer;
		transition: transform var(--motion-base);
	}

	.album-card:hover {
		transform: translateY(-4px);
	}

	.album-card.selected {
		outline: 2px solid var(--accent);
		outline-offset: 4px;
		border-radius: var(--radius-md);
	}

	.album-card:focus-visible,
	.track-row:focus-visible,
	.header-sort:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.album-art {
		position: relative;
		aspect-ratio: 1;
		border-radius: var(--radius-md);
		overflow: hidden;
		margin-bottom: 10px;
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: filter var(--motion-base), box-shadow var(--motion-base);
	}

	.album-card:hover .album-art {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.album-art :global(.album-art-img) {
		width: 100%;
		height: 100%;
		display: block;
	}

	.album-art :global(.album-art-img:not(.fallback)) {
		object-fit: cover;
	}

	.album-art :global(.album-art-img.fallback) {
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--bg-surface);
	}

	.album-art :global(.album-art-img.fallback span) {
		font-size: var(--font-size-2xl);
		color: var(--text-tertiary);
		font-weight: var(--font-weight-semibold);
	}

	/* Single corner play button, revealed on hover (search-page style). */
	.art-play-btn {
		position: absolute;
		right: 8px;
		bottom: 8px;
		width: 40px;
		height: 40px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: #fff;
		border: none;
		box-shadow: 0 6px 16px -4px rgba(0, 0, 0, 0.55);
		opacity: 0;
		transform: translateY(6px);
		transition: opacity var(--motion-base), transform var(--motion-base), filter var(--motion-fast);
		pointer-events: auto;
		cursor: pointer;
		z-index: 2;
	}

	.art-play-btn svg {
		margin-left: 1px;
	}

	.art-play-btn:hover {
		transform: translateY(0) scale(1.06);
		filter: brightness(1.08);
	}

	.album-card:hover .art-play-btn,
	.album-card:focus-within .art-play-btn {
		opacity: 1;
		transform: translateY(0);
	}

	.album-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 0 2px;
		min-width: 0;
	}

	.album-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		color: var(--text-primary);
	}

	.album-artist {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.album-year {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}

	.album-chips {
		display: none;
		gap: 4px;
		flex-wrap: nowrap;
		margin-top: 4px;
		overflow: hidden;
	}

	.album-chip {
		display: inline-flex;
		align-items: center;
		padding: 2px 7px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid var(--border-subtle);
		color: var(--text-muted);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
	}

	/* Actions (⋯) float into the artwork's top-right corner on hover in grid mode. */
	.album-grid:not(.album-list) .album-actions {
		position: absolute;
		top: 6px;
		right: 6px;
		z-index: 3;
		opacity: 0;
		margin: 0;
		padding: 0;
		transition: opacity var(--motion-base);
	}

	.album-grid:not(.album-list) .album-card:hover .album-actions,
	.album-grid:not(.album-list) .album-card:focus-within .album-actions {
		opacity: 1;
	}

	.album-grid:not(.album-list) .menu-trigger {
		width: 30px;
		height: 30px;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.6);
		border: 1px solid rgba(255, 255, 255, 0.16);
		color: #fff;
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
	}

	.album-grid:not(.album-list) .menu-trigger:hover {
		background: rgba(0, 0, 0, 0.78);
		border-color: rgba(255, 255, 255, 0.28);
	}

	/* ─── Menus ─────────────────────────── */

	.menu-trigger {
		width: 32px;
		height: 32px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		border: 1px solid var(--panel-border);
		color: var(--text-primary);
		font-size: var(--font-size-md);
		line-height: 1;
		transition: background 0.15s ease, border-color 0.15s ease;
	}

	.menu-trigger:hover {
		background: rgba(255, 255, 255, 0.14);
		border-color: rgba(255, 255, 255, 0.16);
	}

	.detail-btn {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		cursor: pointer;
		transition: all var(--motion-fast);
	}

	.detail-btn:hover {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent);
	}

	.item-menu {
		position: absolute;
		bottom: calc(100% + 6px);
		right: 0;
		min-width: 172px;
		padding: 6px;
		border-radius: 12px;
		display: flex;
		flex-direction: column;
		gap: 2px;
		z-index: 200;
		background: rgba(18, 18, 26, 0.97);
		border: 1px solid rgba(255, 255, 255, 0.14);
		box-shadow:
			0 8px 32px rgba(0, 0, 0, 0.6),
			0 2px 8px rgba(0, 0, 0, 0.4),
			inset 0 1px 0 rgba(255, 255, 255, 0.06);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
	}

	.track-menu {
		top: calc(100% + 6px);
		bottom: auto;
		right: 0;
	}

	.menu-item {
		padding: 9px 12px;
		border-radius: 8px;
		background: transparent;
		border: none;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		text-align: left;
		cursor: pointer;
		transition: background 0.1s ease;
		white-space: nowrap;
	}

	.menu-item:hover {
		background: rgba(255, 255, 255, 0.08);
	}

	.menu-item.secondary {
		color: var(--text-secondary);
	}

	.menu-item.secondary:hover {
		color: var(--text-primary);
	}

	/* ─── Track List ─────────────────────── */

	.track-list {
		display: flex;
		flex-direction: column;
	}

	.track-rows {
		display: flex;
		flex-direction: column;
	}

	.vlist-spacer {
		flex: none;
	}

	.track-header {
		display: grid;
		gap: var(--gap-sm);
		align-items: center;
		padding: 8px var(--gap-sm);
		border-bottom: 1px solid var(--border-glass);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--text-tertiary);
	}

	.header-sort {
		background: transparent;
		border: 0;
		padding: 0;
		color: inherit;
		font: inherit;
		text-transform: inherit;
		letter-spacing: inherit;
		text-align: inherit;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: 4px;
	}

	.header-sort:hover {
		color: var(--text-primary);
	}

	.header-sort.sorted {
		color: var(--text-primary);
	}

	.sort-arrow {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		transition: color var(--motion-fast);
	}

	.header-sort.sorted .sort-arrow {
		color: var(--accent-strong);
	}

	.track-row {
		display: grid;
		gap: var(--gap-sm);
		align-items: center;
		padding: 6px var(--gap-sm);
		border-radius: var(--radius-xs);
		font-size: var(--font-size-sm);
		text-align: left;
		width: 100%;
		transition: background var(--motion-fast);
	}

	.track-row:hover {
		background: var(--bg-hover);
	}

	.track-row.selected {
		background: var(--accent-soft);
	}

	.track-row.cursor {
		background: var(--bg-hover);
		box-shadow: inset 2px 0 0 var(--accent);
	}

	.col-actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 4px;
		position: relative;
	}

	.track-row.playing {
		color: var(--accent);
	}

	.col-num {
		text-align: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.col-artist, .col-album {
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.col-artist .subtitle-link,
	.col-album .subtitle-link {
		color: inherit;
		text-decoration: none;
	}

	.col-artist .subtitle-link:hover,
	.col-album .subtitle-link:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.col-quality {
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.col-plays {
		text-align: right;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.plays-count {
		font-variant-numeric: tabular-nums;
	}

	.col-date {
		text-align: right;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.date-added {
		font-variant-numeric: tabular-nums;
	}

	.col-duration {
		text-align: right;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.col-title {
		display: flex;
		align-items: center;
		gap: 6px;
		overflow: hidden;
		min-width: 0;
	}

	.track-title {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		min-width: 0;
		flex: 1;
	}

	.playing-indicator {
		color: var(--accent);
		font-size: var(--font-size-xs);
	}

	.track-play-num {
		position: relative;
		display: grid;
		place-items: center;
		width: 100%;
		background: none;
		border: none;
		color: inherit;
		cursor: pointer;
		padding: 0;
	}

	/* Stack the number and the play glyph in the SAME grid cell so both stay
	   in normal flow and the taller one gives the button real height (an
	   absolutely-positioned pair collapsed the button to 0px, leaving the play
	   control a zero-height click target). */
	.track-num-label,
	.track-num-play {
		grid-area: 1 / 1;
		display: grid;
		place-items: center;
	}

	.track-num-label {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		visibility: visible;
	}

	.track-num-play {
		color: var(--accent);
		font-size: var(--font-size-xs);
		visibility: hidden;
	}

	/* Reveal the play glyph on hover by swapping visibility, not display. The old
	   display:none<->block swap forced a layout pass on every hover enter/leave;
	   on the un-virtualized track list that reflow walked an ever-larger box tree,
	   so hovering lagged more the deeper you'd scrolled. visibility is paint-only
	   (no reflow) and, unlike opacity, keeps the hidden glyph out of layout-affecting
	   recomputes while staying constant-cost. */
	.track-row:hover .track-num-label {
		visibility: hidden;
	}

	.track-row:hover .track-num-play {
		visibility: visible;
	}

	/* ─── Empty State ────────────────────── */

	.empty-state {
		display: flex;
		align-items: center;
		justify-content: center;
		padding: calc(var(--gap-xl) * 2);
		color: var(--text-secondary);
	}

	.load-more-row {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--gap);
		padding: var(--gap-lg) 0;
	}

	.infinite-sentinel {
		width: 100%;
		height: 1px;
	}

	.load-more-count {
		font-size: var(--font-size-sm);
		color: var(--text-tertiary);
	}

	@media (max-width: 760px) {
		.album-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
			gap: 12px;
		}

		.album-card {
			padding: 0;
		}

		.album-title,
		.album-artist {
			white-space: normal;
			overflow: visible;
			text-overflow: unset;
			display: -webkit-box;
			-webkit-box-orient: vertical;
		}

		.album-title {
			line-clamp: 2;
			-webkit-line-clamp: 2;
		}

		.album-artist {
			line-clamp: 2;
			-webkit-line-clamp: 2;
		}

		/* Mobile track row: multi-line */
		.track-header {
			display: none;
		}

		.track-list {
			gap: 6px;
		}

		/* Rows sit inside .track-rows now, so .track-list's gap no longer
		   separates them; a margin keeps the spacing inside the row pitch the
		   virtual window measures. */
		.track-rows .track-row {
			margin-bottom: 6px;
		}

		.track-row {
			grid-template-columns: 28px minmax(0, 1fr) auto;
			grid-template-areas:
				"num title actions"
				". artist duration"
				". album quality";
			gap: 6px 12px;
			padding: 12px;
			border: 1px solid var(--border-subtle);
			background: rgba(255, 255, 255, 0.02);
		}

		.col-num { grid-area: num; text-align: left; }
		.col-title { grid-area: title; }
		.col-artist { grid-area: artist; }
		.col-album { grid-area: album; }
		.col-quality { grid-area: quality; justify-content: flex-start; }
		.col-plays,
		.col-date { display: none; }
		.col-duration { grid-area: duration; align-self: center; }
		.col-actions { grid-area: actions; align-self: flex-start; }

		.track-title,
		.col-artist,
		.col-album {
			white-space: normal;
			overflow: visible;
			text-overflow: unset;
		}

		.track-play-num {
			width: 28px;
			height: 28px;
		}

		.track-num-label {
			display: none;
		}

		.track-num-play {
			display: block;
			font-size: var(--font-size-xs);
		}
	}

	/* ─── Artist Grid ────────────────────── */

	.artist-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
		gap: var(--gap);
	}

	.artist-card {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 10px;
		padding: 16px 10px 14px;
		border-radius: var(--radius-lg);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			transform 200ms cubic-bezier(0.22, 1, 0.36, 1);
		text-align: center;
	}

	.artist-card:hover {
		background: rgba(255, 255, 255, 0.06);
		border-color: rgba(255, 255, 255, 0.13);
		transform: translateY(-2px);
	}

	.artist-photo {
		width: 72px;
		height: 72px;
		border-radius: 50%;
		overflow: hidden;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		display: grid;
		place-items: center;
		flex-shrink: 0;
	}

	.artist-photo :global(.artist-photo-img) {
		width: 100%;
		height: 100%;
	}

	.artist-photo :global(.artist-photo-img:not(.fallback)) {
		display: block;
		object-fit: cover;
	}

	.artist-photo :global(.artist-photo-img.fallback) {
		display: grid;
		place-items: center;
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	.artist-photo :global(.artist-photo-img.fallback span) {
		font-family: var(--font-body);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		line-height: 1;
	}

	.artist-name {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100%;
	}

	.artist-card.expanded {
		border-color: var(--accent-line);
		background: rgba(255, 255, 255, 0.07);
	}

	/* ─── Artist Panel ───────────────────── */

	.artist-panel {
		padding: 20px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.artist-panel-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
	}

	.artist-panel-identity {
		display: flex;
		align-items: center;
		gap: 14px;
	}

	.artist-panel-photo {
		width: 52px;
		height: 52px;
		border-radius: 50%;
		object-fit: cover;
		border: 1px solid var(--accent-line);
		flex-shrink: 0;
	}

	.artist-panel-photo.placeholder {
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		font-family: var(--font-body);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		color: var(--accent-strong);
	}

	.artist-panel-count {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
	}

	.artist-panel-actions {
		display: flex;
		gap: 8px;
		flex-shrink: 0;
	}

	.artist-panel-loading {
		color: var(--text-muted);
		font-size: var(--font-size-sm);
		padding: 8px 0;
	}

	.artist-discography-section {
		margin-bottom: 16px;
	}
	.artist-discography-label {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 1.4px;
		color: var(--accent);
		margin-bottom: 10px;
	}
	.artist-discography-row {
		display: flex;
		gap: 12px;
		overflow-x: auto;
		padding-bottom: 4px;
		scrollbar-width: none;
	}
	.artist-discography-row::-webkit-scrollbar { display: none; }
	.discography-card {
		flex-shrink: 0;
		width: 96px;
		text-align: center;
	}
	.discography-art {
		width: 96px;
		height: 96px;
		border-radius: 6px;
		object-fit: cover;
		margin-bottom: 5px;
		background: var(--bg-raised);
	}
	.discography-art.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-2xl);
		color: rgba(255,255,255,0.3);
	}
	.discography-title {
		font-size: var(--font-size-2xs);
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		margin-bottom: 2px;
	}
	.discography-year {
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		margin-bottom: 5px;
	}
	.btn-xs {
		padding: 3px 9px;
		font-size: var(--font-size-2xs);
	}

	.artist-track-list {
		display: flex;
		flex-direction: column;
	}

	.artist-track-row {
		display: grid;
		grid-template-columns: 32px 1fr auto auto auto;
		align-items: center;
		gap: 10px;
		padding: 8px 0;
		border-bottom: 1px solid var(--border-subtle);
		cursor: pointer;
		border-radius: 6px;
		transition: background var(--motion-fast);
	}

	.artist-track-row:last-child { border-bottom: none; }

	.artist-track-row:hover { background: rgba(255,255,255,0.04); }

	.artist-track-art {
		width: 32px;
		height: 32px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.artist-track-art.placeholder {
		background: var(--accent-soft);
		display: grid;
		place-items: center;
		font-size: var(--font-size-sm);
		color: var(--accent-strong);
	}

	.artist-track-meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.artist-track-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.artist-track-album {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.artist-track-dur {
		font-size: var(--font-size-xs);
		color: var(--text-muted);
		flex-shrink: 0;
	}
</style>
