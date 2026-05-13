<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import type { Snapshot } from './$types';
	import {
		tracks, albums, artists as artistsStore, isLoading, isLoadingMore, totalTracks, totalAlbums,
		sortBy, sortDir, viewMode, searchQuery,
		loadTracks, loadAlbums,
		selectedTrackIds, selectedAlbumIds,
		lastSelectedTrackId, lastSelectedAlbumId,
		selectTrackIds, selectAlbumIds, clearSelection,
	} from '$lib/stores/library';
	import { formatTrackDuration, formatDateShort, getQualityClass } from '$lib/utils/format';
	import { api, type Album, type Artist, type Genre, type Playlist, type Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playTrackNow, addTrackToQueue, playTrackNext } from '$lib/stores/player';
	import SelectionBar from '$lib/components/ui/SelectionBar.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import LibraryHero from '$lib/components/LibraryHero.svelte';
	import ArtistCarousel from '$lib/components/ArtistCarousel.svelte';
	import AlbumCarousel from '$lib/components/AlbumCarousel.svelte';
	import AlbumDetailPopup from '$lib/components/AlbumDetailPopup.svelte';
	import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
	import { openContextMenu, openMenuAtElement, type MenuItem } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { parseQuery } from '$lib/search/query_parser';
	import { buildAudioParams, hasAnyFilter } from '$lib/search/audio_params';
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
				const { tracks: t } = await api.getArtistTracks(artistId);
				return t.map(tr => tr.id);
			}),
		}), artist.name);
	}

	function handleHomeAlbumContextMenu(e: MouseEvent, albumId: number) {
		const card = recentAlbums.find(a => a.id === albumId);
		const album = card ?? { id: albumId, title: '' };
		openContextMenu(e, buildAlbumMenu(album, {
			isLocal: true,
			addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
				const { tracks: t } = await api.getAlbumTracks(albumId);
				return t.map(tr => tr.id);
			}),
		}), album.title);
	}

	const PAGE_SIZE = 100;
	const RECENT_TRACK_LIMIT = 10;
	const ALL_SEARCH_ARTIST_PREVIEW_LIMIT = 12;
	const ALL_SEARCH_ALBUM_PREVIEW_LIMIT = 12;
	const ALL_SEARCH_TRACK_PREVIEW_LIMIT = 10;

	let activeTab = $state<'all' | 'tracks' | 'liked' | 'albums' | 'artists'>('all');
	let playlists = $state<Playlist[]>([]);
	let genres = $state<Genre[]>([]);
	let selectedPlaylistId = $state('');
	let selectedGenreId = $state('');
	let batchMessage = $state<string | null>(null);
	let batchError = $state<string | null>(null);
	let batchBusy = $state<'playlist' | 'genre' | 'delete' | null>(null);
	let pendingUndoTrackIds = $state<number[]>([]);
	let albumActionBusyId = $state<number | null>(null);
	let activeTrackMenuId = $state<number | null>(null);
	let searchBusy = $state(false);
	let searchError = $state<string | null>(null);
	let searchResults = $state<{ tracks: Track[]; albums: Album[]; artists: Artist[] }>({ tracks: [], albums: [], artists: [] });
	let searchTimer: ReturnType<typeof setTimeout> | null = null;
	let infiniteSentinel = $state<HTMLDivElement | null>(null);
	let infiniteObserver: IntersectionObserver | null = null;
	let undoTimer: ReturnType<typeof setTimeout> | null = null;
	let artists = $state<Artist[]>([]);
	let artistsLoading = $state(false);
	let failedArtistImages = $state(new Set<string>());
	let recentTracks = $state<Track[]>([]);

	// Keyboard cursor for track list
	let cursorIndex = $state(-1);

	// Decade filter for albums tab
	let activeDecade = $state<number | null>(null);

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

	// Reactive grid template — must match the order of cells in .track-header and .track-row.
	// Cells that get conditionally removed via {#if showXColumn} drop their column track here too,
	// so header and row stay aligned.
	// All non-fr columns must be explicit px — 'auto' sizes independently per row-grid,
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
		void loadAlbums();
		void loadTracks();
		void loadBatchMeta();
		void loadRecentTracks();
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
			const data = await api.getTracks('last_played_at', 'desc', RECENT_TRACK_LIMIT, 0, true, false);
			recentTracks = data.tracks
				.filter((track) => track.last_played_at)
				.slice(0, RECENT_TRACK_LIMIT);
		} catch (error) {
			console.error('Failed to load recent tracks:', error);
		}
	}

	async function loadBatchMeta() {
		try {
			const [playlistData, genreData] = await Promise.all([
				api.getPlaylists(),
				api.getGenres(),
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
		if (activeTab === 'tracks') {
			loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, false);
		} else if (activeTab === 'liked') {
			loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, true);
		} else if (activeTab === 'albums') {
			loadAlbums($sortBy, $sortDir);
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
			// so always refetch from offset 0 when entering either — never reuse stale rows.
			if (tab === 'tracks') loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, false);
			if (tab === 'liked') loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, true);
			if (tab === 'albums') loadAlbums();
		}
		if (tab === 'artists' && artists.length === 0) void loadArtists();
		clearSelection();
	}

	async function loadArtists() {
		artistsLoading = true;
		try {
			// Default browse view — top 200 alphabetically. When the user types
			// a query, the search effect calls api.search() server-side and
			// shows searchResults.artists (FTS). No more upfront 10k load.
			const data = await api.getArtists('name', 'asc', 200);
			artists = data.artists;
		} catch (err) {
			console.error('Failed to load artists:', err);
		} finally {
			artistsLoading = false;
		}
	}

	async function playTrack(track: typeof $tracks[0]) {
		await playTrackNow(track.id);
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
			const data = await api.getAlbumTracks(albumId);
			const trackIds = data.tracks.map((track) => track.id);
			if (trackIds.length === 0) {
				throw new Error('No synced tracks found for this album yet.');
			}
			await api.replacePlaybackQueue(trackIds);
			await playTrackNow(trackIds[0]);
			batchMessage = `Playing album from track 1 of ${trackIds.length}.`;
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
			const data = await api.getAlbumTracks(albumId);
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
				const data = await api.getAlbumTracks(track.album_id);
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
			const data = await api.getAlbumTracks(album.id);
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

	async function removeAlbumFromLibrary(albumId: number) {
		albums.update((list) => list.filter((a) => a.id !== albumId));
		searchResults = {
			tracks: searchResults.tracks,
			albums: searchResults.albums.filter((a) => a.id !== albumId),
			artists: searchResults.artists,
		};
		try {
			await api.batchDelete([], [albumId]);
			batchMessage = `Removed album from your library.`;
		} catch (error) {
			batchError = `Failed to remove album: ${error}`;
			void loadAlbums();
		}
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
				void playTrackNow(track.id);
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

		const deletedTrackIds = [...$selectedTrackIds];
		pendingUndoTrackIds = deletedTrackIds;
		tracks.update((list) => list.filter((track) => !deletedTrackIds.includes(track.id)));
		albums.update((list) => list.filter((album) => !$selectedAlbumIds.has(album.id)));
		searchResults = {
			tracks: searchResults.tracks.filter((track) => !deletedTrackIds.includes(track.id)),
			albums: searchResults.albums.filter((album) => !$selectedAlbumIds.has(album.id)),
			artists: searchResults.artists
		};

		try {
			const result = await api.batchDelete(deletedTrackIds, [...$selectedAlbumIds]);
			batchMessage = `Removed ${result.removed_tracks} track favorites and ${result.removed_albums} album favorites from TIDAL.`;
			clearSelection();
			undoTimer = setTimeout(() => {
				pendingUndoTrackIds = [];
				void loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, activeTab === 'liked');
				void loadAlbums();
			}, 6000);
		} catch (error) {
			batchError = `Failed to delete selection: ${error}`;
			pendingUndoTrackIds = [];
			void loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, activeTab === 'liked');
			void loadAlbums();
		} finally {
			batchBusy = null;
		}
	}

	function undoDelete() {
		if (undoTimer) clearTimeout(undoTimer);
		pendingUndoTrackIds = [];
		batchMessage = 'Delete view reverted locally. Run sync to restore remote favorites if needed.';
		void loadTracks($sortBy, $sortDir, PAGE_SIZE, 0, activeTab === 'liked');
		void loadAlbums();
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

	async function runLibrarySearch(query: string) {
		const trimmed = query.trim();
		if (!trimmed) {
			searchResults = { tracks: [], albums: [], artists: [] };
			searchBusy = false;
			searchError = null;
			return;
		}

		searchBusy = true;
		searchError = null;
		try {
			const parsed = parseQuery(trimmed);
			if (hasAnyFilter(parsed)) {
				// DSP/filter syntax (bpm:138, key:Am, energy:>0.7, genre:dnb, etc.) — route to audio search.
				const params = buildAudioParams(parsed, genres);
				const audio = await api.searchAudio(params);
				const adaptedTracks: Track[] = audio.tracks.map((r) => ({
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
				searchResults = { tracks: adaptedTracks, albums: [], artists: [] };
			} else {
				// Plain text — server-side FTS. No more preloading the full library.
				const r = await api.search(trimmed, 100);
				searchResults = {
					tracks: r.tracks,
					albums: r.albums,
					artists: r.artists,
				};
			}
			clearSelection();
		} catch (error) {
			searchError = `Search failed: ${error}`;
			searchResults = { tracks: [], albums: [], artists: [] };
		} finally {
			searchBusy = false;
		}
	}

	async function loadMoreVisibleItems() {
		if ($isLoading || $isLoadingMore || $searchQuery.trim()) return;
		if (activeTab === 'tracks' || activeTab === 'liked') {
			if ($tracks.length >= $totalTracks) return;
			await loadTracks($sortBy, $sortDir, PAGE_SIZE, $tracks.length, activeTab === 'liked');
			return;
		}
		if ($albums.length >= $totalAlbums) return;
		await loadAlbums($sortBy, $sortDir, PAGE_SIZE, $albums.length);
	}

	async function playRandomLibrary() {
		batchError = null;
		batchMessage = null;
		try {
			if ($searchQuery.trim()) {
				const source = (activeTab === 'tracks' || activeTab === 'liked')
					? visibleTracks
					: searchResults.tracks;
				if (source.length === 0) {
					throw new Error('No searchable tracks are available in the current library view.');
				}
				const randomTrack = source[Math.floor(Math.random() * source.length)];
				await playTrackNow(randomTrack.id);
				batchMessage = `Playing a random pick: ${randomTrack.title}.`;
				return;
			}

			if ($totalTracks === 0) {
				throw new Error('No tracks are loaded in the library yet.');
			}

			const randomOffset = Math.floor(Math.random() * $totalTracks);
			const data = await api.getTracks('date_added', 'desc', 1, randomOffset);
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
	let decadeBuckets = $derived.by(() => {
		const seen = new Set<number>();
		for (const a of $albums) {
			if (a.year != null) seen.add(Math.floor(a.year / 10) * 10);
		}
		return [...seen].sort((a, b) => a - b);
	});
	let visibleAlbums = $derived.by(() => {
		let base = $searchQuery.trim() ? searchResults.albums : $albums;
		if ($searchQuery.trim() && $sortBy && $sortBy !== 'relevance') {
			const dir = $sortDir === 'desc' ? -1 : 1;
			base = [...base].sort((a, b) => {
				let av: string | number | null | undefined;
				let bv: string | number | null | undefined;
				switch ($sortBy) {
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
			: false)
	);
	let searchSummary = $derived(
		activeTab === 'all'
			? formatSearchSummary(allSearchArtists.length, visibleAlbums.length, visibleTracks.length)
			: (activeTab === 'tracks' || activeTab === 'liked')
			? `${visibleTracks.length} track match${visibleTracks.length === 1 ? '' : 'es'}`
			: `${visibleAlbums.length} album match${visibleAlbums.length === 1 ? '' : 'es'}`
	);
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
		const top: HeroArtist[] = played.slice(0, 5).map(a => ({ ...a, kind: 'top' }));

		// Forgotten favourite: an artist in the local DB (which only contains
		// Tidal-favourited artists) that the user has barely listened to.
		const topIds = new Set(top.map(a => a.id));
		const candidates = all
			.filter(a => !topIds.has(a.id) && !!(a.photo_url ?? a.fallback_art_url) && a.playCount === 0);
		const fallback = candidates.length === 0
			? all.filter(a => !topIds.has(a.id) && !!(a.photo_url ?? a.fallback_art_url) && a.playCount < 2)
			: candidates;
		if (fallback.length > 0) {
			const pick = fallback[Math.floor(Math.random() * fallback.length)];
			top.push({ ...pick, kind: 'forgotten_favorite' as const });
		}

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

	// Per-tile lazy artwork. Keyed by domain-prefixed id so we never collide
	// (track 5 and album 5 are independent entries). Populated by lazyTidalArt
	// when a tile without baked artwork scrolls into view.
	let lazyArt = $state<Record<string, string>>({});
	let artistLazyArt = $state<Record<number, string>>({});

	// ── Home view handlers ─────────────────────────────────────────────────

	function playAllForArtist(artistId: number) {
		const artistTracks = $tracks.filter(t => t.artist_id === artistId);
		if (!artistTracks.length) return;
		void playTrackNow(artistTracks[0].id);
		for (const t of artistTracks.slice(1)) void addTrackToQueue(t.id);
	}

	function shuffleArtist(artistId: number) {
		const artistTracks = [...$tracks.filter(t => t.artist_id === artistId)];
		artistTracks.sort(() => Math.random() - 0.5);
		if (!artistTracks.length) return;
		void playTrackNow(artistTracks[0].id);
		for (const t of artistTracks.slice(1)) void addTrackToQueue(t.id);
	}

	function handleHomeArtistClick(artistId: number) {
		void goto(`/artists/${artistId}`);
	}

	function handleHomeAlbumClick(albumId: number) {
		const found = $albums.find(a => a.id === albumId);
		if (found) {
			void openAlbumDetail(found);
		} else {
			// Album not in current loaded page — build a stub from recentAlbums card
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

	// ─── Position memory (Phase 5B — SvelteKit snapshot) ─────────────────────
	// Snapshot binds state to the browser's history entry. Multi-selection is
	// active-task state and is intentionally not captured (resets on nav).
	let pendingRestoreScroll = $state<number | null>(null)

	$effect(() => {
		if (pendingRestoreScroll !== null) {
			const target = pendingRestoreScroll
			pendingRestoreScroll = null
			requestAnimationFrame(() => window.scrollTo({ top: target, behavior: 'auto' }))
		}
	})

	type LibrarySnapshot = {
		activeTab: typeof activeTab
		searchQuery: string
		sortBy: string
		sortDir: 'asc' | 'desc'
		viewMode: 'grid' | 'list'
		activeDecade: number | null
		scrollY: number
	}
	export const snapshot: Snapshot<LibrarySnapshot> = {
		capture: () => ({
			activeTab,
			searchQuery: get(searchQuery),
			sortBy: get(sortBy),
			sortDir: get(sortDir),
			viewMode: get(viewMode),
			activeDecade,
			scrollY: typeof window !== 'undefined' ? window.scrollY : 0
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
			activeDecade = saved.activeDecade
			if (typeof saved.scrollY === 'number') pendingRestoreScroll = saved.scrollY
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
		el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
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
		<input
			class="library-search-input"
			bind:value={$searchQuery}
			type="search"
			placeholder={activeTab === 'albums' ? 'Search albums or artists' : 'Search tracks, albums, or artists'}
		/>
		<div class="kbd-hint">
			<kbd>/</kbd> focus &nbsp;·&nbsp;
			<kbd>↑↓</kbd> move &nbsp;·&nbsp;
			<kbd>Enter</kbd> play &nbsp;·&nbsp;
			<kbd>Shift</kbd>+<kbd>Enter</kbd> queue &nbsp;·&nbsp;
			<kbd>Ctrl</kbd>+<kbd>Enter</kbd> next &nbsp;·&nbsp;
			<span class="hint-filters">bpm:138 &nbsp;·&nbsp; key:Am &nbsp;·&nbsp; energy:&gt;0.7 &nbsp;·&nbsp; genre:dnb &nbsp;·&nbsp; instrumental:true</span>
		</div>

		<div class="filter-pills">
			<div class="filter-pill-group filter-pill-group--primary">
				<button class="filter-pill" class:active={activeTab === 'all'}     onclick={() => switchTab('all')}>All</button>
				<button class="filter-pill" class:active={activeTab === 'tracks'}  onclick={() => switchTab('tracks')}>Tracks</button>
				<button class="filter-pill" class:active={activeTab === 'liked'}   onclick={() => switchTab('liked')}>Liked</button>
				<button class="filter-pill" class:active={activeTab === 'albums'}  onclick={() => switchTab('albums')}>Albums</button>
				<button class="filter-pill" class:active={activeTab === 'artists'} onclick={() => switchTab('artists')}>Artists</button>
				<button class="filter-pill filter-pill--ghost" onclick={() => void playRandomLibrary()} title="Random play">
					⤮ Random
				</button>
			</div>

			<div class="filter-pill-actions">
				{#if activeTab === 'albums'}
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

				{#if searchBusy}
					<span class="library-status">Searching…</span>
				{:else if isSearchMode}
					<span class="library-status">{searchSummary}</span>
					<button class="filter-pill filter-pill--ghost" onclick={() => (searchQuery.set(''))}>Clear</button>
				{/if}
			</div>
		</div>
	</div>


	{#if searchError}
		<div class="batch-feedback error glass">{searchError}</div>
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
			{#if pendingUndoTrackIds.length > 0}
				<button class="btn btn-glass" onclick={undoDelete}>Undo</button>
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
							{@const photoSrc = artist.photo_url && !failedArtistImages.has(artist.photo_url) ? artist.photo_url : null}
							{@const lazyArtistImg = artistLazyArt[artist.id] && !failedArtistImages.has(artistLazyArt[artist.id]) ? artistLazyArt[artist.id] : null}
							{@const fallbackSrc = artistArtworkById.get(artist.id)}
							{@const fallbackArtistImg = fallbackSrc && !failedArtistImages.has(fallbackSrc) ? fallbackSrc : null}
							{@const artistImg = photoSrc ?? lazyArtistImg ?? fallbackArtistImg}
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
									enabled: !photoSrc && !artistLazyArt[artist.id],
									query: { artist: artist.name },
									onResolve: (url) => (artistLazyArt[artist.id] = url),
								}}
							>
								<div class="artist-photo">
									{#if artistImg}
										<img src={artistImg} alt={artist.name} loading="lazy" onerror={() => { failedArtistImages = new Set([...failedArtistImages, artistImg]); }} />
									{:else}
										<span class="artist-initial">{artist.name.charAt(0).toUpperCase()}</span>
									{/if}
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
											const { tracks: t } = await api.getAlbumTracks(album.id);
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
									{#if albumArt}
										<img src={albumArt} alt={album.title} loading="lazy" />
									{:else}
										<div class="art-placeholder" aria-hidden="true"></div>
									{/if}
									<div class="album-art-overlay">
										<button
											class="art-play-btn"
											aria-label="Play {album.title}"
											onclick={(event) => void playAlbum(album.id, event)}
										>
											<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
												<polygon points="5,3 13,8 5,13" fill="currentColor" />
											</svg>
										</button>
										<button
											class="art-info-btn"
											aria-label="View {album.title} details"
											onclick={(event) => { event.stopPropagation(); void openAlbumDetail(album); }}
										>
											i
										</button>
									</div>
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
									{#if trackArt}
										<img class="ht-art-img" src={trackArt} alt="" loading="lazy" />
									{/if}
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
						<button class="view-all-link" onclick={() => switchTab('tracks')}>View all →</button>
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
									{#if trackArt}
										<img class="ht-art-img" src={trackArt} alt="" loading="lazy" />
									{/if}
								</div>
								<div class="ht-meta">
									<span class="ht-title">{track.title}</span>
									<span class="ht-sub">{track.artist_name ?? ''}{track.album_title ? ` — ${track.album_title}` : ''}</span>
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
		{#if decadeBuckets.length > 1}
			<div class="decade-strip">
				<button class="decade-chip" class:active={activeDecade === null} onclick={() => activeDecade = null}>All</button>
				{#each decadeBuckets as decade}
					<button
						class="decade-chip"
						class:active={activeDecade === decade}
						onclick={() => activeDecade = activeDecade === decade ? null : decade}
					>{decade}s</button>
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
								const { tracks: t } = await api.getAlbumTracks(album.id);
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
						{#if albumArt}
							<img src={albumArt} alt={album.title} loading="lazy" />
						{:else}
							<div class="art-placeholder">♫</div>
						{/if}
						<div class="album-art-overlay">
							<button
								class="art-play-btn"
								aria-label="Play {album.title}"
								onclick={(event) => void playAlbum(album.id, event)}
							>
								▶
							</button>
							<button
								class="art-info-btn"
								aria-label="View {album.title} details"
								onclick={(event) => { event.stopPropagation(); void openAlbumDetail(album); }}
							>
								ℹ
							</button>
						</div>
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
									onRemove: () => void removeAlbumFromLibrary(album.id),
									addToPlaylistSubmenu: buildAddToPlaylistSubmenu(async () => {
										const { tracks: t } = await api.getAlbumTracks(album.id);
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

		{#if visibleAlbums.length === 0}
			<EmptyState title={isSearchMode ? 'No albums match this search' : 'No albums yet'} copy={isSearchMode ? 'Try a broader search term or switch to tracks.' : 'Connect TIDAL in Settings and run a sync to populate the library.'} />
		{:else if !isSearchMode && $albums.length < $totalAlbums}
			<div class="load-more-row">
				<span class="load-more-count">{$albums.length} of {$totalAlbums} albums</span>
				<button
					class="btn btn-glass"
					disabled={$isLoadingMore}
					onclick={() => loadAlbums($sortBy, $sortDir, PAGE_SIZE, $albums.length)}
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
					{@const photoSrc = artist.photo_url && !failedArtistImages.has(artist.photo_url) ? artist.photo_url : null}
					{@const lazyArtistImg = artistLazyArt[artist.id] && !failedArtistImages.has(artistLazyArt[artist.id]) ? artistLazyArt[artist.id] : null}
					{@const fallbackSrc = artistArtworkById.get(artist.id)}
					{@const fallbackArtistImg = fallbackSrc && !failedArtistImages.has(fallbackSrc) ? fallbackSrc : null}
					{@const artistImg = photoSrc ?? lazyArtistImg ?? fallbackArtistImg}
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
							enabled: !photoSrc && !artistLazyArt[artist.id],
							query: { artist: artist.name },
							onResolve: (url) => (artistLazyArt[artist.id] = url),
						}}
					>
						<div class="artist-photo">
							{#if artistImg}
								<img src={artistImg} alt={artist.name} loading="lazy" onerror={() => { failedArtistImages = new Set([...failedArtistImages, artistImg]); }} />
							{:else}
								<span class="artist-initial">{artist.name.charAt(0).toUpperCase()}</span>
							{/if}
						</div>
						<span class="artist-name">{artist.name}</span>
					</button>
				{/each}
			</div>

		{/if}

	{:else if activeTab === 'tracks' || activeTab === 'liked'}
		<!-- Track List (shared between Tracks and Liked tabs — server filters via likedOnly) -->
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

			{#each visibleTracks as track, i (track.id)}
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
					<span class="col-artist">{track.artist_name ?? 'Unknown'}</span>
					<span class="col-album">{track.album_title ?? ''}</span>
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
							<span class="plays-count">{track.play_count > 0 ? track.play_count.toLocaleString() : '—'}</span>
						</span>
					{/if}
					{#if showDateColumn}
						<span class="col-date">
							<span class="date-added">{track.date_added ? formatDateShort(track.date_added) : '—'}</span>
						</span>
						<span class="col-date">
							<span class="last-played">{track.last_played_at ? formatDateShort(track.last_played_at) : '—'}</span>
						</span>
					{/if}
					{#if showBpmColumn}
						<span class="col-bpm">
							<span class="bpm-value">{track.bpm ? Math.round(track.bpm) : '—'}</span>
						</span>
					{/if}
					{#if showKeyColumn}
						<span class="col-key">
							{#if track.camelot_key}
								<span class="camelot-badge">{track.camelot_key}</span>
							{:else}
								<span>—</span>
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
								<span>—</span>
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
								<span>—</span>
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
	>
		<div class="modal-panel glass-panel" onclick={(e) => e.stopPropagation()} role="dialog" tabindex="-1" aria-modal="true" aria-label={detailTrack.title}>
			<div class="modal-topbar">
				<button class="modal-close" aria-label="Close" onclick={() => { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }}>✕</button>
			</div>

			<div class="detail-track-hero">
				{#if detailTrack.artwork_url}
					<img class="detail-track-art-large" src={detailTrack.artwork_url} alt="" />
				{:else}
					<div class="detail-track-art-large placeholder">♫</div>
				{/if}
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
								{#if track.artwork_url}
									<img class="detail-track-art" src={track.artwork_url} alt="" loading="lazy" />
								{/if}
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

	.ht-art-img {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.ht-art--fallback {
		background: var(--bg-hover);
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
	.decade-chip {
		padding: 4px 13px;
		border-radius: var(--radius-md);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		border: 1px solid var(--panel-border);
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		transition: border-color 0.15s, background 0.15s, color 0.15s;
	}
	.decade-chip:hover {
		border-color: var(--accent-line);
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

	.kbd-hint {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 2px;
		font-size: var(--font-size-xs);
		color: var(--text-muted, rgba(255,255,255,0.35));
		padding: 4px 0 0 2px;
		user-select: none;
		max-width: 640px;
		width: 100%;
		margin: 0 auto;
	}

	.kbd-hint kbd {
		display: inline-block;
		padding: 1px 5px;
		border: 1px solid var(--border-subtle, rgba(255,255,255,0.15));
		border-radius: 4px;
		font-family: inherit;
		font-size: var(--font-size-2xs);
		color: var(--text-secondary, rgba(255,255,255,0.5));
		background: var(--bg-hover);
	}

	.hint-filters {
		opacity: 0.7;
		font-family: var(--font-mono);
		letter-spacing: 0.02em;
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

	.library-search-input {
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
		padding: 14px 22px;
		border-radius: var(--radius-lg);
		border: 1px solid var(--border-subtle);
		background: var(--panel-bg);
		color: var(--text-primary);
		font-size: var(--font-size-md);
		outline: none;
		transition: border-color var(--motion-fast), background var(--motion-fast);
	}

	.library-search-input:focus {
		border-color: var(--accent);
		background: var(--input-focus);
	}

	.library-status {
		font-size: var(--font-size-xs);
		color: var(--text-muted, rgba(255,255,255,0.4));
		margin-left: 4px;
	}

	.filter-pills {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
		gap: 6px;
		align-items: center;
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
	}

	.filter-pill-group,
	.filter-pill-actions {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-wrap: wrap;
	}

	.filter-pill-group--primary {
		grid-column: 2;
		justify-content: center;
	}

	.filter-pill-actions {
		grid-column: 3;
		justify-self: start;
	}

	.filter-pill--ghost {
		opacity: 0.75;
	}

	.filter-pill {
		padding: 5px 14px;
		border-radius: 20px;
		border: 1px solid var(--border-subtle, rgba(255,255,255,0.1));
		background: transparent;
		color: var(--text-secondary, rgba(255,255,255,0.6));
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background 0.15s, color 0.15s, border-color 0.15s;
		white-space: nowrap;
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

	.view-toggle {
		display: inline-flex;
		gap: 2px;
		padding: 2px;
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--border-subtle);
	}

	.view-toggle-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 28px;
		height: 28px;
		padding: 0;
		border: 0;
		border-radius: 6px;
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
		background: rgba(124, 128, 255, 0.22);
		color: var(--text-primary);
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
	.detail-track-art-large {
		width: 140px;
		height: 140px;
		border-radius: var(--radius);
		object-fit: cover;
		flex-shrink: 0;
		box-shadow: 0 4px 20px rgba(0, 0, 0, 0.3);
	}

	.detail-track-art-large.placeholder {
		background: var(--accent-soft);
		display: flex;
		align-items: center;
		justify-content: center;
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

	.detail-track-art {
		width: 40px;
		height: 40px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
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
		.detail-track-art-large {
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
		.view-toggle {
			width: 100%;
		}

		.filter-pills {
			grid-template-columns: 1fr;
		}

		.filter-pill-group--primary,
		.filter-pill-actions {
			grid-column: 1;
			width: 100%;
			justify-content: center;
		}

		.filter-pill {
			flex: 1;
			text-align: center;
		}

		.view-toggle :global(.btn) {
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

		.detail-track-art,
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
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: var(--gap);
		align-items: start;
	}

	@media (min-width: 1600px) {
		.album-grid {
			grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
		}
	}

	/* ─── Album List Mode ────────────────── */

	.album-grid.album-list {
		display: flex;
		flex-direction: column;
		gap: 0;
		border-radius: var(--radius-md);
		overflow: hidden;
		border: 1px solid rgba(255, 255, 255, 0.05);
		background: rgba(255, 255, 255, 0.015);
	}

	.album-grid.album-list .album-card {
		display: grid;
		grid-template-columns: 40px 1fr auto;
		gap: 12px;
		align-items: center;
		padding: 6px 12px;
		border-radius: 0;
		background: transparent;
		border: 0;
		border-top: 1px solid rgba(255, 255, 255, 0.04);
		box-shadow: none;
	}

	.album-grid.album-list .album-card:first-child {
		border-top: 0;
	}

	.album-grid.album-list .album-card:hover {
		transform: none;
		box-shadow: none;
		background: rgba(255, 255, 255, 0.04);
		border-color: rgba(255, 255, 255, 0.04);
	}

	.album-grid.album-list .album-card:hover .album-art {
		filter: none;
	}

	.album-grid.album-list .album-art {
		width: 40px;
		height: 40px;
		aspect-ratio: unset;
		margin-bottom: 0;
		border-radius: 4px;
		flex-shrink: 0;
		box-shadow: none;
	}

	.album-grid.album-list .album-art::after {
		display: none;
	}

	.album-grid.album-list .album-art-overlay {
		display: none;
	}

	.album-grid.album-list .album-meta {
		padding: 0;
		min-width: 0;
	}

	.album-grid.album-list .album-chips {
		margin-top: 3px;
	}

	.album-grid.album-list .album-actions {
		margin-top: 0;
		padding: 0;
	}

	.album-card {
		position: relative;
		padding: var(--gap-sm);
		border-radius: var(--radius-lg);
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.06), rgba(255, 255, 255, 0.02)),
			var(--bg-surface);
		border: 1px solid var(--panel-border);
		box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
		cursor: pointer;
		transition:
			transform 260ms cubic-bezier(0.22, 1, 0.36, 1),
			box-shadow 220ms ease,
			border-color 220ms ease;
	}

	.album-card:hover {
		transform: translateY(-5px) scale(1.015);
		border-color: rgba(255, 255, 255, 0.16);
		box-shadow:
			0 20px 40px rgba(0, 0, 0, 0.32),
			0 0 20px var(--accent-glow);
	}

	.album-card.selected {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
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
		border-radius: calc(var(--radius-lg) - 6px);
		overflow: hidden;
		margin-bottom: 10px;
		background: var(--bg-surface);
		box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
		transition: filter 220ms ease;
	}

	.album-art::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(180deg, transparent 50%, rgba(0, 0, 0, 0.2));
		pointer-events: none;
	}

	.album-card:hover .album-art {
		filter: saturate(1.06) brightness(1.03);
	}

	.album-art img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.art-placeholder {
		width: 100%;
		height: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-2xl);
		color: var(--text-tertiary);
	}

	.album-art-overlay {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		background: rgba(0, 0, 0, 0);
		transition: background 200ms ease;
		pointer-events: none;
	}

	.album-card:hover .album-art-overlay {
		background: rgba(0, 0, 0, 0.35);
	}

	.art-play-btn,
	.art-info-btn {
		width: 42px;
		height: 42px;
		border-radius: 50%;
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-md);
		background: rgba(0, 0, 0, 0.45);
		color: white;
		opacity: 0;
		transform: translateY(6px);
		transition: opacity 200ms ease, transform 200ms ease, background 150ms ease;
		backdrop-filter: blur(4px);
		pointer-events: auto;
		cursor: pointer;
		border: none;
	}

	.art-play-btn:hover {
		background: var(--accent);
		transform: translateY(0) scale(1.05);
	}

	.art-info-btn:hover {
		background: rgba(255, 255, 255, 0.25);
		transform: translateY(0) scale(1.05);
	}

	.album-card:hover .art-play-btn,
	.album-card:hover .art-info-btn {
		opacity: 1;
		transform: translateY(0);
	}

	.album-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		padding: 0 4px 4px;
		min-width: 0;
	}

	.album-actions {
		position: relative;
		display: flex;
		justify-content: flex-end;
		padding: 0 4px 4px;
		margin-top: 10px;
	}

	.album-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-md);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		transition: color 240ms ease;
	}

	.album-artist {
		font-size: var(--font-size-sm);
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
		display: flex;
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

	.album-card:hover .album-title {
		color: var(--text-primary);
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
		display: flex;
		align-items: center;
		justify-content: center;
		width: 100%;
		height: 100%;
		background: none;
		border: none;
		color: inherit;
		cursor: pointer;
		padding: 0;
	}

	.track-num-label {
		display: block;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.track-num-play {
		display: none;
		color: var(--accent);
		font-size: var(--font-size-xs);
	}

	.track-row:hover .track-num-label {
		display: none;
	}

	.track-row:hover .track-num-play {
		display: block;
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
			padding: 10px;
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

	.artist-photo img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.artist-initial {
		font-family: var(--font-body);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		color: var(--accent-strong);
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
