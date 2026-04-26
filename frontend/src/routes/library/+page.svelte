<script lang="ts">
	import { onMount } from 'svelte';
	import {
		tracks, albums, isLoading, isLoadingMore, totalTracks, totalAlbums,
		sortBy, sortDir, viewMode, searchQuery,
		loadTracks, loadAlbums,
		formatDuration, formatDateShort, getQualityClass,
		selectedTrackIds, selectedAlbumIds,
		lastSelectedTrackId, lastSelectedAlbumId,
		selectTrackIds, selectAlbumIds, clearSelection,
	} from '$lib/stores/library';
	import { api, type Album, type Artist, type Genre, type Playlist, type Track } from '$lib/api/client';
	import { currentTrack, isPlaying, playTrackNow, addTrackToQueue } from '$lib/stores/player';
	import SelectionBar from '$lib/components/ui/SelectionBar.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	const PAGE_SIZE = 100;
	const SEARCH_LIMIT = 200;

	let activeTab = $state<'tracks' | 'albums' | 'artists'>('albums');
	let playlists = $state<Playlist[]>([]);
	let genres = $state<Genre[]>([]);
	let selectedPlaylistId = $state('');
	let selectedGenreId = $state('');
	let batchMessage = $state<string | null>(null);
	let batchError = $state<string | null>(null);
	let batchBusy = $state<'playlist' | 'genre' | 'delete' | null>(null);
	let pendingUndoTrackIds = $state<number[]>([]);
	let albumActionBusyId = $state<number | null>(null);
	let activeAlbumMenuId = $state<number | null>(null);
	let activeTrackMenuId = $state<number | null>(null);
	let searchBusy = $state(false);
	let searchError = $state<string | null>(null);
	let searchResults = $state<{ tracks: Track[]; albums: Album[] }>({ tracks: [], albums: [] });
	let searchTimer: ReturnType<typeof setTimeout> | null = null;
	let infiniteSentinel = $state<HTMLDivElement | null>(null);
	let infiniteObserver: IntersectionObserver | null = null;
	let undoTimer: ReturnType<typeof setTimeout> | null = null;
	let artists = $state<Artist[]>([]);
	let artistsLoading = $state(false);
	let expandedArtistId = $state<number | null>(null);
	let artistTracksById = $state<Record<number, Track[]>>({});
	let artistTracksLoadingId = $state<number | null>(null);

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
	let showBpmColumn = $state(true);
	let showKeyColumn = $state(true);
	let showEnergyColumn = $state(true);
	let showDanceColumn = $state(true);

	// DSP filter panel
	let showDspFilters = $state(false);
	let filterBpmMin = $state<number | null>(null);
	let filterBpmMax = $state<number | null>(null);
	let filterEnergyMin = $state(0);
	let filterEnergyMax = $state(1);
	let filterKey = $state('');
	let filterInstrumental = $state(false);

	// Camelot keys for dropdown
	const CAMELOT_KEYS = [
		'', '1A', '2A', '3A', '4A', '5A', '6A', '7A', '8A', '9A', '10A', '11A', '12A',
		'1B', '2B', '3B', '4B', '5B', '6B', '7B', '8B', '9B', '10B', '11B', '12B'
	];

	onMount(() => {
		void loadAlbums();
		void loadTracks();
		void loadBatchMeta();
		return () => {
			if (searchTimer) clearTimeout(searchTimer);
			infiniteObserver?.disconnect();
			if (undoTimer) clearTimeout(undoTimer);
		};
	});

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
			loadTracks($sortBy, $sortDir);
		} else if (activeTab === 'albums') {
			loadAlbums($sortBy, $sortDir);
		}
		clearSelection();
	}

	async function applyDspFilters() {
		// Reload tracks with DSP filters applied via API params
		// For now, just toggle the filter state — actual API filter integration
		// requires backend endpoint support for bpm_min, bpm_max, etc.
		batchMessage = 'DSP filters applied (backend support required for full filtering).';
	}

	async function clearDspFilters() {
		filterBpmMin = null;
		filterBpmMax = null;
		filterEnergyMin = 0;
		filterEnergyMax = 1;
		filterKey = '';
		filterInstrumental = false;
		batchMessage = 'DSP filters cleared.';
	}

	function switchTab(tab: 'tracks' | 'albums' | 'artists') {
		activeTab = tab;
		expandedTrackId = null;
		expandedAlbumId = null;
		detailTrack = null;
		detailAlbum = null;
		if (!$searchQuery.trim()) {
			if (tab === 'tracks') loadTracks($sortBy, $sortDir);
			if (tab === 'albums') loadAlbums();
			if (tab === 'artists' && artists.length === 0) void loadArtists();
		}
		clearSelection();
	}

	async function loadArtists() {
		artistsLoading = true;
		try {
			const data = await api.getArtists('name', 'asc', 500);
			artists = data.artists;
		} catch (err) {
			console.error('Failed to load artists:', err);
		} finally {
			artistsLoading = false;
		}
	}

	async function toggleArtist(artist: Artist) {
		if (expandedArtistId === artist.id) {
			expandedArtistId = null;
			return;
		}
		expandedArtistId = artist.id;
		if (artistTracksById[artist.id]) return;
		artistTracksLoadingId = artist.id;
		try {
			const data = await api.getArtistTracks(artist.id);
			artistTracksById = { ...artistTracksById, [artist.id]: data.tracks };
		} catch (err) {
			console.error('Failed to load artist tracks:', err);
			artistTracksById = { ...artistTracksById, [artist.id]: [] };
		} finally {
			artistTracksLoadingId = null;
		}
	}

	async function playArtist(artist: Artist, event: MouseEvent) {
		event.stopPropagation();
		const tracks = artistTracksById[artist.id];
		if (!tracks || tracks.length === 0) return;
		await api.replacePlaybackQueue(tracks.map((t) => t.id));
		await playTrackNow(tracks[0].id);
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

	function handleAlbumCardClick(albumId: number, event: MouseEvent) {
		const additive = event.ctrlKey || event.metaKey;
		const range = event.shiftKey;
		updateAlbumSelection(albumId, additive, range);
	}

	function handleTrackRowKeydown(trackId: number, event: KeyboardEvent) {
		runOnActivation(event, () => updateTrackSelection(trackId));
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
			albums: searchResults.albums.filter((album) => !$selectedAlbumIds.has(album.id))
		};

		try {
			const result = await api.batchDelete(deletedTrackIds, [...$selectedAlbumIds]);
			batchMessage = `Removed ${result.removed_tracks} track favorites and ${result.removed_albums} album favorites from TIDAL.`;
			clearSelection();
			undoTimer = setTimeout(() => {
				pendingUndoTrackIds = [];
				void loadTracks($sortBy, $sortDir);
				void loadAlbums();
			}, 6000);
		} catch (error) {
			batchError = `Failed to delete selection: ${error}`;
			pendingUndoTrackIds = [];
			void loadTracks($sortBy, $sortDir);
			void loadAlbums();
		} finally {
			batchBusy = null;
		}
	}

	function undoDelete() {
		if (undoTimer) clearTimeout(undoTimer);
		pendingUndoTrackIds = [];
		batchMessage = 'Delete view reverted locally. Run sync to restore remote favorites if needed.';
		void loadTracks($sortBy, $sortDir);
		void loadAlbums();
	}

	function closeMenus() {
		activeAlbumMenuId = null;
		activeTrackMenuId = null;
	}

	function toggleAlbumMenu(albumId: number, event: MouseEvent) {
		event.stopPropagation();
		activeTrackMenuId = null;
		activeAlbumMenuId = activeAlbumMenuId === albumId ? null : albumId;
	}

	function toggleTrackMenu(trackId: number, event: MouseEvent) {
		event.stopPropagation();
		activeAlbumMenuId = null;
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

	function selectAlbumFromMenu(albumId: number, event: MouseEvent) {
		event.stopPropagation();
		closeMenus();
		updateAlbumSelection(albumId);
	}

	async function runLibrarySearch(query: string) {
		const trimmed = query.trim();
		if (!trimmed) {
			searchResults = { tracks: [], albums: [] };
			searchBusy = false;
			searchError = null;
			return;
		}

		searchBusy = true;
		searchError = null;
		try {
			const results = await api.search(trimmed, SEARCH_LIMIT);
			searchResults = {
				tracks: results.tracks,
				albums: results.albums
			};
			clearSelection();
		} catch (error) {
			searchError = `Search failed: ${error}`;
			searchResults = { tracks: [], albums: [] };
		} finally {
			searchBusy = false;
		}
	}

	async function loadMoreVisibleItems() {
		if ($isLoading || $isLoadingMore || $searchQuery.trim()) return;
		if (activeTab === 'tracks') {
			if ($tracks.length >= $totalTracks) return;
			await loadTracks($sortBy, $sortDir, PAGE_SIZE, $tracks.length);
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
				const source = activeTab === 'tracks'
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
	let libraryModeLabel = $derived(activeTab === 'albums' ? 'Album view' : activeTab === 'artists' ? 'Artist view' : 'Track view');
	let libraryModeCopy = $derived(
		activeTab === 'albums'
			? 'Artwork-first browse with quick album actions.'
			: activeTab === 'artists'
			? 'Browse your artists and explore their tracks.'
			: 'Dense track management with direct playback and batch work.'
	);
	let isSearchMode = $derived(Boolean($searchQuery.trim()));
	let visibleTracks = $derived($searchQuery.trim() ? searchResults.tracks : $tracks);
	let visibleAlbums = $derived($searchQuery.trim() ? searchResults.albums : $albums);
	let canLoadMore = $derived(
		!$searchQuery.trim() &&
		(activeTab === 'tracks' ? $tracks.length < $totalTracks : activeTab === 'albums' ? $albums.length < $totalAlbums : false)
	);
	let searchSummary = $derived(
		activeTab === 'tracks'
			? `${visibleTracks.length} track match${visibleTracks.length === 1 ? '' : 'es'}`
			: `${visibleAlbums.length} album match${visibleAlbums.length === 1 ? '' : 'es'}`
	);
	let loadedSummary = $derived(
		activeTab === 'albums'
			? `${$albums.length} of ${$totalAlbums} albums loaded`
			: `${$tracks.length} of ${$totalTracks} tracks loaded`
	);

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
</script>

<svelte:window onclick={closeMenus} onkeydown={(e) => {
	if (e.key === 'Escape') {
		if (expandedAlbumId !== null) { expandedAlbumId = null; detailAlbum = null; detailAlbumTracksList = []; }
		if (expandedTrackId !== null) { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }
	}
}} />

<div class="page-shell library animate-in">
	<section class="library-hero glass-panel">
		<div class="library-hero-main">
			<div class="library-hero-copy">
				<p class="eyebrow">Library</p>
				<div class="library-hero-heading">
					<h1>Library</h1>
					<span class="library-mode-pill">{libraryModeLabel}</span>
				</div>
				<p class="library-hero-subtitle">{libraryModeCopy}</p>
			</div>

			<div class="library-hero-actions">
				<div class="tab-bar">
					<button class="tab" class:active={activeTab === 'albums'} onclick={() => switchTab('albums')}>Albums</button>
					<button class="tab" class:active={activeTab === 'tracks'} onclick={() => switchTab('tracks')}>Tracks</button>
					<button class="tab" class:active={activeTab === 'artists'} onclick={() => switchTab('artists')}>Artists</button>
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
				<button class="btn btn-glass" onclick={() => void playRandomLibrary()}>
					Random play
				</button>
			</div>
		</div>

		<div class="library-hero-stats">
			<span class="library-stat-chip">{$totalAlbums.toLocaleString()} albums</span>
			<span class="library-stat-chip">{$totalTracks.toLocaleString()} tracks</span>
			<span class="library-stat-chip">{selectionCount.toLocaleString()} selected</span>
			<span class="library-stat-chip emphasis">{isSearchMode ? searchSummary : loadedSummary}</span>
		</div>
	</section>

	<div class="library-toolbar glass">
		<input
			bind:value={$searchQuery}
			type="search"
			placeholder={activeTab === 'albums' ? 'Search albums or artists' : 'Search tracks, albums, or artists'}
		/>
		<div class="toolbar-meta">
			{#if searchBusy}
				<span class="toolbar-note">Searching…</span>
			{:else if isSearchMode}
				<span class="toolbar-note">{searchSummary}</span>
				<button class="btn btn-glass" onclick={() => (searchQuery.set(''))}>Clear search</button>
			{:else}
				<span class="toolbar-note">{loadedSummary}</span>
			{/if}
		</div>
	</div>

	{#if activeTab === 'tracks'}
		<div class="dsp-filter-bar glass">
			<button class="btn btn-glass btn-sm" onclick={() => showDspFilters = !showDspFilters}>
				{showDspFilters ? 'Hide DSP Filters' : 'DSP Filters'}
			</button>
			{#if showDspFilters}
				<div class="dsp-filter-grid">
					<div class="filter-group">
						<label>BPM Range</label>
						<div class="filter-inputs">
							<input type="number" placeholder="Min" bind:value={filterBpmMin} min="40" max="300" />
							<span>–</span>
							<input type="number" placeholder="Max" bind:value={filterBpmMax} min="40" max="300" />
						</div>
					</div>
					<div class="filter-group">
						<label>Energy</label>
						<div class="filter-inputs">
							<input type="range" bind:value={filterEnergyMin} min="0" max="1" step="0.05" />
							<input type="range" bind:value={filterEnergyMax} min="0" max="1" step="0.05" />
						</div>
					</div>
					<div class="filter-group">
						<label>Key</label>
						<select bind:value={filterKey}>
							{#each CAMELOT_KEYS as key}
								<option value={key}>{key || 'All Keys'}</option>
							{/each}
						</select>
					</div>
					<div class="filter-group">
						<label>Instrumental</label>
						<label class="toggle-switch-small">
							<input type="checkbox" bind:checked={filterInstrumental} />
							<span>Only</span>
						</label>
					</div>
					<div class="filter-actions">
						<button class="btn btn-primary btn-sm" onclick={applyDspFilters}>Apply</button>
						<button class="btn btn-glass btn-sm" onclick={clearDspFilters}>Clear</button>
					</div>
				</div>
			{/if}
		</div>
	{/if}

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
				{#if activeTab === 'tracks'}
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

	{:else if activeTab === 'albums'}
		<!-- Album Grid -->
		<div class="album-grid" class:album-list={$viewMode === 'list'}>
			{#each visibleAlbums as album (album.id)}
				<div
					class="album-card"
					class:selected={$selectedAlbumIds.has(album.id)}
					role="button"
					tabindex="0"
					aria-pressed={$selectedAlbumIds.has(album.id)}
					onclick={(event) => handleAlbumCardClick(album.id, event)}
					onkeydown={(event) => handleAlbumCardKeydown(album.id, event)}
				>
					<div class="album-art">
						{#if album.artwork_url}
							<img src={album.artwork_url} alt={album.title} loading="lazy" />
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
						<button class="menu-trigger" aria-label="Album actions" onclick={(event) => toggleAlbumMenu(album.id, event)}>
							⋯
						</button>
						{#if activeAlbumMenuId === album.id}
							<div class="item-menu" role="menu" tabindex="-1" onmousedown={(event) => event.stopPropagation()}>
								<button class="menu-item" onclick={(event) => void playAlbum(album.id, event)}>
									{albumActionBusyId === album.id ? 'Working...' : 'Play Album'}
								</button>
								<button class="menu-item" onclick={(event) => void queueAlbum(album.id, event)}>
									Queue Album
								</button>
								<button class="menu-item" onclick={(event) => { event.stopPropagation(); void openAlbumDetail(album); closeMenus(); }}>
									View Details
								</button>
								<button class="menu-item secondary" onclick={(event) => selectAlbumFromMenu(album.id, event)}>
									Select Album
								</button>
							</div>
						{/if}
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
		{#if artistsLoading}
			<div class="loading">Loading artists…</div>
		{:else if artists.length === 0}
			<EmptyState title="No artists yet" copy="Sync your TIDAL library in Settings to populate artists." />
		{:else}
			<div class="artist-grid">
				{#each artists as artist (artist.id)}
					<button
						class="artist-card"
						class:expanded={expandedArtistId === artist.id}
						onclick={() => void toggleArtist(artist)}
						title="Expand {artist.name}"
					>
						<div class="artist-photo">
							{#if artist.photo_url}
								<img src={artist.photo_url} alt={artist.name} loading="lazy" />
							{:else}
								<span class="artist-initial">{artist.name.charAt(0).toUpperCase()}</span>
							{/if}
						</div>
						<span class="artist-name">{artist.name}</span>
					</button>
				{/each}
			</div>

			{#if expandedArtistId !== null}
				{@const expandedArtist = artists.find(a => a.id === expandedArtistId)}
				{#if expandedArtist}
					<div class="artist-panel glass-panel">
						<div class="artist-panel-header">
							<div class="artist-panel-identity">
								{#if expandedArtist.photo_url}
									<img class="artist-panel-photo" src={expandedArtist.photo_url} alt={expandedArtist.name} />
								{:else}
									<div class="artist-panel-photo placeholder">{expandedArtist.name.charAt(0).toUpperCase()}</div>
								{/if}
								<div>
									<h3>{expandedArtist.name}</h3>
									{#if artistTracksById[expandedArtist.id]}
										<span class="artist-panel-count">{artistTracksById[expandedArtist.id].length} tracks</span>
									{/if}
								</div>
							</div>
							<div class="artist-panel-actions">
								{#if artistTracksById[expandedArtist.id]?.length}
									<button class="btn btn-primary btn-sm" onclick={(e) => void playArtist(expandedArtist, e)}>Play all</button>
								{/if}
								<button class="btn btn-glass btn-sm" onclick={() => expandedArtistId = null}>Close</button>
							</div>
						</div>

						{#if artistTracksLoadingId === expandedArtist.id}
							<p class="artist-panel-loading">Loading tracks…</p>
						{:else if (artistTracksById[expandedArtist.id]?.length ?? 0) === 0}
							<p class="artist-panel-loading">No tracks found.</p>
						{:else}
							<div class="artist-track-list">
								{#each artistTracksById[expandedArtist.id] as track (track.id)}
									<div
										class="artist-track-row"
										role="button"
										tabindex="0"
										onclick={() => void playTrackNow(track.id)}
										onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
									>
										{#if track.artwork_url}
											<img class="artist-track-art" src={track.artwork_url} alt="" loading="lazy" />
										{:else}
											<div class="artist-track-art placeholder">♫</div>
										{/if}
										<div class="artist-track-meta">
											<span class="artist-track-title">{track.title}</span>
											{#if track.album_title}
												<span class="artist-track-album">{track.album_title}</span>
											{/if}
										</div>
										{#if track.best_quality}
											<span class="quality-badge {getQualityClass(track.best_quality)}">{track.best_quality.replace(/_/g, ' ')}</span>
										{/if}
										<span class="artist-track-dur">{formatDuration(track.duration_ms)}</span>
										<button class="queue-btn" onclick={(e) => { e.stopPropagation(); void addTrackToQueue(track.id); }}>+</button>
									</div>
								{/each}
							</div>
						{/if}
					</div>
				{/if}
			{/if}
		{/if}

	{:else if activeTab === 'tracks'}
		<!-- Track List -->
		<div class="track-list">
			<div class="track-header">
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
				<span class="col-album">Album</span>
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
					role="button"
					tabindex="0"
					aria-pressed={$selectedTrackIds.has(track.id)}
					ondblclick={() => void playTrack(track)}
					onclick={(event) => handleTrackRowClick(track.id, event)}
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
					<span class="col-duration">{formatDuration(track.duration_ms)}</span>
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
				<span class="load-more-count">{$tracks.length} of {$totalTracks} tracks</span>
				<button
					class="btn btn-glass"
					disabled={$isLoadingMore}
					onclick={() => loadTracks($sortBy, $sortDir, PAGE_SIZE, $tracks.length)}
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
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div
		class="modal-backdrop"
		onclick={() => { expandedAlbumId = null; detailAlbum = null; detailAlbumTracksList = []; }}
	>
		<div class="modal-panel glass-panel" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={detailAlbum.title}>
			<div class="modal-topbar">
				<button class="modal-close" aria-label="Close" onclick={() => { expandedAlbumId = null; detailAlbum = null; detailAlbumTracksList = []; }}>✕</button>
			</div>

			<div class="detail-album-hero">
				{#if detailAlbum.artwork_url}
					<img class="detail-album-art" src={detailAlbum.artwork_url} alt={detailAlbum.title} />
				{:else}
					<div class="detail-album-art placeholder">♫</div>
				{/if}
				<div class="detail-album-info">
					<h2>{detailAlbum.title}</h2>
					<p class="detail-artist">{detailAlbum.artist_name ?? 'Unknown Artist'}</p>
					<div class="detail-meta-row">
						{#if detailAlbum.year}<span class="detail-chip">{detailAlbum.year}</span>{/if}
						{#if detailAlbum.release_type}<span class="detail-chip">{detailAlbum.release_type}</span>{/if}
						{#if detailAlbum.track_count}<span class="detail-chip">{detailAlbum.track_count} tracks</span>{/if}
						<span class="detail-chip">{detailAlbum.source}</span>
					</div>
					<div class="detail-actions">
						<button class="btn btn-primary" onclick={(e) => void playAlbum(detailAlbum!.id, e)}>▶ Play All</button>
						<button class="btn btn-glass" onclick={(e) => void queueAlbum(detailAlbum!.id, e)}>+ Queue All</button>
					</div>
				</div>
			</div>

			{#if detailAlbumLoading}
				<div class="detail-loading"><div class="spinner spinner-sm"></div><span>Loading tracks…</span></div>
			{:else if detailAlbumTracksList.length === 0}
				<EmptyState title="No tracks synced yet" copy="Run a TIDAL sync to populate this album's tracks." />
			{:else}
				<div class="detail-track-list">
					{#each detailAlbumTracksList as track, i (track.id)}
						<div
							class="detail-track-row"
							class:playing={$currentTrack?.id === track.id}
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
							<span class="detail-track-duration">{formatDuration(track.duration_ms)}</span>
							<button class="detail-track-queue" onclick={(e) => { e.stopPropagation(); void addTrackToQueue(track.id); }}>+</button>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</div>
{/if}

<!-- ─── Track Detail Modal ─────────────────────── -->
{#if expandedTrackId !== null && detailTrack}
	<!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
	<div
		class="modal-backdrop"
		onclick={() => { expandedTrackId = null; detailTrack = null; detailAlbumTracks = []; }}
	>
		<div class="modal-panel glass-panel" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={detailTrack.title}>
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
					<span class="meta-value">{formatDuration(detailTrack.duration_ms)}</span>
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
								<span class="detail-track-duration">{formatDuration(track.duration_ms)}</span>
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

	/* ─── Loading ───────────────────────── */

	.loading {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 12px;
		padding: 48px 0;
		color: var(--text-secondary);
		font-size: 0.9rem;
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

	.library-hero-heading h1 {
		font-size: clamp(1.55rem, 2.4vw, 2.15rem);
		line-height: 1;
	}

	.library-mode-pill,
	.library-stat-chip {
		display: inline-flex;
		align-items: center;
		padding: 6px 10px;
		border-radius: 999px;
		border: 1px solid rgba(255, 255, 255, 0.08);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-secondary);
		font-size: 0.78rem;
		font-weight: 600;
	}

	.library-mode-pill {
		color: var(--text-primary);
		background: rgba(124, 128, 255, 0.12);
		border-color: rgba(124, 128, 255, 0.22);
	}

	.library-hero-subtitle {
		max-width: 48ch;
		color: var(--text-secondary);
		font-size: 0.9rem;
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
		font-size: 0.84rem;
	}

	/* ─── DSP Filter Bar ───────────────────────── */

	.dsp-filter-bar {
		padding: 10px 14px;
		margin-bottom: var(--gap);
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.dsp-filter-grid {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
		align-items: flex-end;
	}

	.filter-group {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 140px;
		flex: 1;
	}

	.filter-group label {
		font-size: 0.72rem;
		font-weight: 600;
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-tertiary);
	}

	.filter-inputs {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.filter-inputs input[type="number"] {
		width: 70px;
		padding: 5px 8px;
		font-size: 0.82rem;
	}

	.filter-inputs input[type="range"] {
		flex: 1;
	}

	.filter-inputs span {
		color: var(--text-muted);
		font-size: 0.82rem;
	}

	.filter-actions {
		display: flex;
		gap: 6px;
		align-items: flex-end;
	}

	.toggle-switch-small {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: 0.82rem;
		color: var(--text-secondary);
		cursor: pointer;
	}

	.toggle-switch-small input {
		width: 16px;
		height: 16px;
		accent-color: var(--accent);
	}

	/* ─── New DSP Columns ───────────────────────── */

	.col-bpm, .col-key, .col-energy, .col-dance {
		font-size: 0.78rem;
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
		font-size: 0.72rem;
		font-weight: 700;
		font-family: var(--font-mono);
	}

	.camelot-badge-inline {
		display: inline-block;
		padding: 1px 5px;
		margin-left: 4px;
		border-radius: 4px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: 0.68rem;
		font-weight: 700;
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
		font-size: 0.68rem;
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
		font-size: 0.72rem;
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
		vertical-align: middle;
	}

	.tab-bar {
		display: flex;
		gap: 4px;
		padding: 2px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.tab {
		padding: 8px 14px;
		border-radius: 999px;
		font-size: 0.82rem;
		font-weight: 600;
		color: var(--text-secondary);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.tab.active {
		background: rgba(124, 128, 255, 0.14);
		color: var(--text-primary);
	}

	.view-toggle {
		display: inline-flex;
		gap: 2px;
		padding: 2px;
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.06);
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
		font-size: 0.82rem;
		font-weight: 600;
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
		font-size: 2.5rem;
		color: var(--accent-strong);
	}

	.detail-album-art.placeholder {
		background: var(--accent-soft);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 2.5rem;
		color: var(--accent-strong);
	}

	.detail-album-info,
	.detail-track-info {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.detail-album-info h2,
	.detail-track-info h2 {
		font-size: 1.5rem;
		line-height: 1.2;
	}

	.detail-artist {
		color: var(--text-secondary);
		font-size: 0.95rem;
	}

	.detail-album-name {
		color: var(--text-tertiary);
		font-size: 0.85rem;
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
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
		font-size: 0.72rem;
		font-weight: 600;
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
		font-size: 0.68rem;
		font-weight: 700;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-muted);
	}

	.meta-value {
		font-size: 0.88rem;
		color: var(--text-primary);
		word-break: break-all;
	}

	.detail-album-tracks h3 {
		font-size: 0.9rem;
		font-weight: 600;
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
		font-size: 0.78rem;
	}

	.detail-track-art {
		width: 40px;
		height: 40px;
		border-radius: 6px;
		object-fit: cover;
		flex-shrink: 0;
	}

	.detail-track-title {
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.detail-track-artist {
		color: var(--text-secondary);
		font-size: 0.82rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 180px;
	}

	.detail-track-duration {
		color: var(--text-tertiary);
		font-size: 0.82rem;
	}

	.detail-track-queue {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
		font-size: 1rem;
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

		.tab-bar,
		.view-toggle {
			width: 100%;
		}

		.tab {
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
		font-size: 0.9rem;
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
			var(--bg-glass);
		border: 1px solid rgba(255, 255, 255, 0.08);
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
		outline: 2px solid rgba(155, 111, 255, 0.85);
		outline-offset: 2px;
	}

	.album-card:focus-visible,
	.track-row:focus-visible,
	.header-sort:focus-visible {
		outline: 2px solid rgba(155, 111, 255, 0.9);
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
		font-size: 2rem;
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
		font-size: 1.1rem;
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
		font-weight: 600;
		font-size: 0.9rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		transition: color 240ms ease;
	}

	.album-artist {
		font-size: 0.8125rem;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.album-year {
		font-size: 0.75rem;
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
		border: 1px solid rgba(255, 255, 255, 0.06);
		color: var(--text-muted);
		font-size: 0.64rem;
		font-weight: 600;
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
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-primary);
		font-size: 1rem;
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
		border: 1px solid rgba(255, 255, 255, 0.06);
		color: var(--text-tertiary);
		font-size: 0.85rem;
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
		backdrop-filter: blur(20px);
		-webkit-backdrop-filter: blur(20px);
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
		font-size: 0.8125rem;
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
		grid-template-columns: 40px minmax(0, 2fr) minmax(0, 1.5fr) minmax(0, 1.5fr) auto auto auto auto 60px 50px 55px 55px auto 40px;
		gap: var(--gap-sm);
		align-items: center;
		padding: 8px var(--gap-sm);
		border-bottom: 1px solid var(--border-glass);
		font-size: 0.75rem;
		font-weight: 500;
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
		font-size: 0.65rem;
		color: var(--text-muted);
		transition: color var(--motion-fast);
	}

	.header-sort.sorted .sort-arrow {
		color: var(--accent-strong);
	}

	.track-row {
		display: grid;
		grid-template-columns: 40px minmax(0, 2fr) minmax(0, 1.5fr) minmax(0, 1.5fr) auto auto auto auto 60px 50px 55px 55px auto 40px;
		gap: var(--gap-sm);
		align-items: center;
		padding: 6px var(--gap-sm);
		border-radius: var(--radius-xs);
		font-size: 0.875rem;
		text-align: left;
		width: 100%;
		transition: background var(--motion-fast);
	}

	.track-row:hover {
		background: var(--bg-glass-hover);
	}

	.track-row.selected {
		background: var(--accent-soft);
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
		font-size: 0.8125rem;
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
		font-size: 0.78rem;
		min-width: 50px;
	}

	.plays-count {
		font-variant-numeric: tabular-nums;
	}

	.col-date {
		text-align: right;
		color: var(--text-tertiary);
		font-size: 0.75rem;
		min-width: 70px;
	}

	.date-added {
		font-variant-numeric: tabular-nums;
	}

	.col-duration {
		text-align: right;
		color: var(--text-tertiary);
		font-size: 0.8125rem;
		min-width: 55px;
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
		font-size: 0.7rem;
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
		font-size: 0.8125rem;
	}

	.track-num-play {
		display: none;
		color: var(--accent);
		font-size: 0.7rem;
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
		font-size: 0.8125rem;
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
			border: 1px solid rgba(255, 255, 255, 0.06);
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
			font-size: 0.78rem;
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
		font-family: var(--font-display);
		font-size: 1.6rem;
		color: var(--accent-strong);
		line-height: 1;
	}

	.artist-name {
		font-size: 0.82rem;
		font-weight: 600;
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
		font-family: var(--font-display);
		font-size: 1.4rem;
		color: var(--accent-strong);
	}

	.artist-panel-identity h3 {
		font-size: 1rem;
		font-weight: 700;
		margin: 0;
	}

	.artist-panel-count {
		font-size: 0.78rem;
		color: var(--text-muted);
	}

	.artist-panel-actions {
		display: flex;
		gap: 8px;
		flex-shrink: 0;
	}

	.artist-panel-loading {
		color: var(--text-muted);
		font-size: 0.85rem;
		padding: 8px 0;
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
		font-size: 0.8rem;
		color: var(--accent-strong);
	}

	.artist-track-meta {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.artist-track-title {
		font-size: 0.85rem;
		font-weight: 500;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.artist-track-album {
		font-size: 0.75rem;
		color: var(--text-muted);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.artist-track-dur {
		font-size: 0.78rem;
		color: var(--text-muted);
		flex-shrink: 0;
	}
</style>
