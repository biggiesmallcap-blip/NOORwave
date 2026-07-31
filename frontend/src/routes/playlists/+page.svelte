<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import type { Snapshot } from './$types';
	import { captureScroll, restoreScroll } from '$lib/navigation/scroll';
	import {
		api,
		type Playlist,
		type Track,
		type RuleClause,
		type LogicOp,
		type NumberOp,
		type QualityTier,
		type DateField,
		type SampleDataSource,
	} from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { invalidatePlaylistCaches } from '$lib/cache/ws_events';
	import { wsMessages } from '$lib/api/ws';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { createPersistedStore, oneOf } from '$lib/stores/persisted';
	import { buildPlaylistMenu } from '$lib/player/playlist_menu';
	import { downloadPlaylist } from '$lib/stores/downloads';
	import {
		playTracksInContext,
		shufflePlaylist,
		startPlaylistRadio,
	} from '$lib/stores/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import type { MenuItem } from '$lib/stores/context_menu';
	import {
		getCachedMosaic,
		setCachedMosaic,
		snapshotCache,
		pickArtworkUrls,
		nameToGradient,
	} from '$lib/stores/playlist_artwork_cache';

	// ─── Types used only in the editor ────────────────────────────────────────
	// A flat "draft clause" that the editor mutates before converting to RuleClause.
	type DraftClause = {
		id: number; // local key for #each
		type: RuleClause['type'];
		// genre / artist
		names: string[];
		match_descendants: boolean;
		// date_range
		date_field: DateField;
		date_start: string;
		date_end: string;
		// play_count
		play_count_op: NumberOp;
		play_count_value: number;
		play_count_max: number;
		// quality
		quality_minimum: QualityTier;
		// not_in_playlist
		exclude_playlist_ids: number[];
		// bpm_range / energy_range / danceability_range
		range_min: number | null;
		range_max: number | null;
		// key_signature / camelot_key
		key_value: string;
		// instrumental_only
		is_instrumental: boolean;
		// has_sample_data
		sample_source: SampleDataSource | '';
	};

	type PlaylistFilter = 'all' | 'favorites' | 'smart' | 'regular';
	type PlaylistSort = 'default' | 'name' | 'tracks' | 'type' | 'recent_update' | 'recent_create';

	const PLAYLIST_FILTERS = ['all', 'favorites', 'smart', 'regular'] as const;
	const PLAYLIST_SORTS = [
		'default',
		'name',
		'tracks',
		'type',
		'recent_update',
		'recent_create',
	] as const;

	const persistedFilter = createPersistedStore<PlaylistFilter>('playlists.filter', 'all', {
		parse: oneOf(PLAYLIST_FILTERS),
	});
	const persistedSort = createPersistedStore<PlaylistSort>('playlists.sort', 'recent_update', {
		parse: oneOf(PLAYLIST_SORTS),
	});

	let _draftIdCounter = 0;
	function newDraftId() {
		return ++_draftIdCounter;
	}

	function defaultDraft(): DraftClause {
		return {
			id: newDraftId(),
			type: 'genre',
			names: [],
			match_descendants: true,
			date_field: 'date_added',
			date_start: '',
			date_end: '',
			play_count_op: 'gte',
			play_count_value: 5,
			play_count_max: 20,
			quality_minimum: 'lossless',
			exclude_playlist_ids: [],
			range_min: null,
			range_max: null,
			key_value: '',
			is_instrumental: true,
			sample_source: '',
		};
	}

	function draftFromClause(clause: RuleClause): DraftClause {
		const d = defaultDraft();
		d.type = clause.type;
		if (clause.type === 'genre') {
			d.names = [...clause.names];
			d.match_descendants = clause.match_descendants;
		} else if (clause.type === 'artist') {
			d.names = [...clause.names];
		} else if (clause.type === 'date_range') {
			d.date_field = clause.field;
			d.date_start = clause.range.start ?? '';
			d.date_end = clause.range.end ?? '';
		} else if (clause.type === 'play_count') {
			d.play_count_op = clause.op;
			d.play_count_value = clause.value;
			d.play_count_max = clause.value_max ?? 20;
		} else if (clause.type === 'quality') {
			d.quality_minimum = clause.minimum;
		} else if (clause.type === 'not_in_playlist') {
			d.exclude_playlist_ids = [...clause.playlist_ids];
		} else if (
			clause.type === 'bpm_range' ||
			clause.type === 'energy_range' ||
			clause.type === 'danceability_range'
		) {
			d.range_min = clause.min;
			d.range_max = clause.max;
		} else if (clause.type === 'key_signature' || clause.type === 'camelot_key') {
			d.key_value = clause.key;
		} else if (clause.type === 'instrumental_only') {
			d.is_instrumental = clause.is_instrumental;
		} else if (clause.type === 'has_sample_data') {
			d.sample_source = clause.source ?? '';
		}
		return d;
	}

	function draftToClause(d: DraftClause): RuleClause {
		switch (d.type) {
			case 'genre':
				return { type: 'genre', names: d.names, match_descendants: d.match_descendants };
			case 'artist':
				return { type: 'artist', names: d.names };
			case 'date_range':
				return {
					type: 'date_range',
					field: d.date_field,
					range: { start: d.date_start || null, end: d.date_end || null },
				};
			case 'play_count':
				return {
					type: 'play_count',
					op: d.play_count_op,
					value: d.play_count_value,
					value_max: d.play_count_op === 'between_inclusive' ? d.play_count_max : null,
				};
			case 'quality':
				return { type: 'quality', minimum: d.quality_minimum };
			case 'not_in_playlist':
				return { type: 'not_in_playlist', playlist_ids: d.exclude_playlist_ids };
			case 'bpm_range':
				return { type: 'bpm_range', min: d.range_min, max: d.range_max };
			case 'energy_range':
				return { type: 'energy_range', min: d.range_min, max: d.range_max };
			case 'danceability_range':
				return { type: 'danceability_range', min: d.range_min, max: d.range_max };
			case 'key_signature':
				return { type: 'key_signature', key: d.key_value.trim() };
			case 'camelot_key':
				return { type: 'camelot_key', key: d.key_value.trim() };
			case 'instrumental_only':
				return { type: 'instrumental_only', is_instrumental: d.is_instrumental };
			case 'has_sample_data':
				return {
					type: 'has_sample_data',
					source: d.sample_source === '' ? null : d.sample_source,
				};
			default:
				return { type: 'genre', names: [], match_descendants: false };
		}
	}

	// ─── Page state ───────────────────────────────────────────────────────────
	let playlists = $state<Playlist[]>([]);
	let playlistTracksById = $state<Record<number, Track[]>>({});
	let isLoading = $state(true);
	let loadError = $state('');
	let playlistQuery = $state('');
	// Sort and filter persist across sessions, not just per history entry: a
	// SvelteKit snapshot only restores on back/forward, so the user's choice
	// reset every time they arrived here fresh. Defaults to "Recently updated"
	// because that is the order that answers "what changed?", which is what a
	// playlist list is usually being opened for.
	let playlistFilter = $state<PlaylistFilter>(get(persistedFilter));
	let playlistSort = $state<PlaylistSort>(get(persistedSort));
	$effect(() => persistedFilter.set(playlistFilter));
	$effect(() => persistedSort.set(playlistSort));
	let playlistLoadSeq = 0;
	let destroyed = false;

	// Cover mosaics keyed by playlist id. Seeded from localStorage on mount,
	// kept in sync whenever a playlist's tracks load.
	let mosaicById = $state<Record<number, string[]>>({});

	// Per-playlist pre-computed search blob, populated on load. Avoids
	// JSON.parse(smart_rules) for every playlist on every keystroke.
	let searchTextById = $state<Record<number, string>>({});

	// Inline delete confirmation - the card flips into a confirm bar instead
	// of firing the destructive call on the first click.
	let pendingDeleteId = $state<number | null>(null);

	// Phase 5B - back/forward state via SvelteKit snapshot. Sort and filter stay
	// here as well as in localStorage: the snapshot restores what was on screen
	// for this specific history entry, localStorage is the fresh-load default.
	export const snapshot: Snapshot<{
		scrollY: number;
		query?: string;
		filter?: PlaylistFilter;
		sort?: PlaylistSort;
	}> = {
		capture: () => ({
			scrollY: captureScroll(),
			query: playlistQuery,
			filter: playlistFilter,
			sort: playlistSort,
		}),
		restore: (saved) => {
			if (typeof saved.query === 'string') playlistQuery = saved.query;
			if (saved.filter) playlistFilter = saved.filter;
			if (saved.sort) playlistSort = saved.sort;
			restoreScroll(saved.scrollY);
		}
	};

	// ─── Editor state ─────────────────────────────────────────────────────────
	let editorOpen = $state(false);
	let editingPlaylistId = $state<number | null>(null); // null = creating new
	let draftName = $state('');
	let draftDescription = $state('');
	let draftLogicOp = $state<LogicOp>('AND');
	let draftClauses = $state<DraftClause[]>([]);
	let editorSaving = $state(false);
	let editorError = $state('');
	let nameInput = $state<string>(''); // tag input buffer for genre/artist
	// Per-clause tag input buffers keyed by draft id
	let tagInputs = $state<Record<number, string>>({});

	// Delete confirm
	let deletingId = $state<number | null>(null);
	let deleteError = $state('');

	// Editor dirty-tracking - the serialized snapshot of the draft at the
	// moment the editor opened. Compared against the live draft to decide
	// whether to prompt before closing.
	let editorInitialSig = $state('');

	// Genre suggestions for the tag input datalist. Lazily fetched the first
	// time the editor opens, cached for the session.
	let genreSuggestions = $state<string[]>([]);
	let genreSuggestionsLoaded = false;

	// Artist suggestions for artist-clause datalists. Fetched per query as
	// the user types, with a small in-session cache. Library artist tables
	// can run into the thousands so we never fetch the full list.
	let artistSuggestions = $state<string[]>([]);
	let artistQuery = $state('');
	let artistFetchAbort: AbortController | null = null;
	const artistCache = new Map<string, string[]>();

	// Live "matches N tracks" preview for the editor. Recomputed on a
	// debounce when the draft changes; null = idle, undefined-style.
	let previewCount = $state<number | null>(null);
	let previewError = $state('');
	let previewLoading = $state(false);
	let previewAbort: AbortController | null = null;
	let previewDebounce: ReturnType<typeof setTimeout> | null = null;

	// Drawer focus management
	let editorTriggerEl: HTMLElement | null = null;

	function trapFocus(node: HTMLElement) {
		const selector =
			'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';
		function focusable() {
			return Array.from(node.querySelectorAll<HTMLElement>(selector)).filter(
				(el) => !el.hasAttribute('disabled') && el.tabIndex !== -1,
			);
		}
		function handleKey(e: KeyboardEvent) {
			if (e.key !== 'Tab') return;
			const list = focusable();
			if (list.length === 0) return;
			const first = list[0];
			const last = list[list.length - 1];
			const active = document.activeElement as HTMLElement | null;
			if (e.shiftKey && (active === first || !node.contains(active))) {
				e.preventDefault();
				last.focus();
			} else if (!e.shiftKey && (active === last || !node.contains(active))) {
				e.preventDefault();
				first.focus();
			}
		}
		node.addEventListener('keydown', handleKey);
		// Defer initial focus so the input is mounted
		queueMicrotask(() => {
			const target = node.querySelector<HTMLInputElement>('#draft-name');
			target?.focus();
		});
		return {
			destroy() {
				node.removeEventListener('keydown', handleKey);
			},
		};
	}

	onMount(() => {
		mosaicById = { ...mosaicById, ...mapMosaicsFromCache(snapshotCache()) };
		void loadPlaylists();
		// A playlist can be created while this page is already open - saving the
		// queue from the layout's queue panel is the common case. Cache
		// invalidation alone would not repaint, because loadPlaylists() only runs
		// on mount, so react to the server event too.
		const unsubscribeWs = wsMessages.subscribe((messages) => {
			if (messages.at(-1)?.type === 'playlists_changed') void loadPlaylists();
		});
		return unsubscribeWs;
	});

	onDestroy(() => {
		destroyed = true;
		playlistLoadSeq += 1;
		artistFetchAbort?.abort();
		previewAbort?.abort();
		if (previewDebounce) {
			clearTimeout(previewDebounce);
			previewDebounce = null;
		}
		mosaicObserver?.disconnect();
		mosaicObserver = null;
	});

	function mapMosaicsFromCache(snapshot: Record<number, { urls: string[] }>): Record<number, string[]> {
		const out: Record<number, string[]> = {};
		for (const [idStr, entry] of Object.entries(snapshot)) {
			out[Number(idStr)] = entry.urls;
		}
		return out;
	}

	// ─── Data loading ─────────────────────────────────────────────────────────
	async function loadPlaylists() {
		const seq = ++playlistLoadSeq;
		// Only the first load is allowed to show a loading state. Every later call
		// is a background refresh driven by a `playlists_changed` event, and
		// blanking a populated list to "Loading playlists" on each one made a
		// simple rename flash the whole page away.
		if (playlists.length === 0) isLoading = true;
		loadError = '';
		try {
			const data = await cachedApi.getPlaylists();
			if (!isCurrentPlaylistLoad(seq)) return;
			playlists = data.playlists;
			const next: Record<number, string> = {};
			for (const p of playlists) next[p.id] = buildSearchText(p);
			searchTextById = next;
		} catch (error) {
			if (!isCurrentPlaylistLoad(seq)) return;
			loadError = `Failed to load playlists: ${error}`;
		} finally {
			if (isCurrentPlaylistLoad(seq)) isLoading = false;
		}
	}

	function isCurrentPlaylistLoad(seq: number): boolean {
		return !destroyed && seq === playlistLoadSeq;
	}

	function recordMosaic(id: number, tracks: Track[], trackCount: number) {
		const urls = pickArtworkUrls(tracks);
		if (urls.length === 0) return;
		setCachedMosaic(id, urls, trackCount);
		mosaicById = { ...mosaicById, [id]: urls };
	}

	/**
	 * Tracks for a playlist, fetched once and kept for the session.
	 *
	 * Only the row's quick actions (play / shuffle / radio) and the cover mosaic
	 * need these now - browsing a playlist's contents happens on
	 * `/playlists/[id]`. Smart playlists resolve through the evaluate endpoint
	 * because their contents are computed, not stored.
	 */
	async function ensurePlaylistTracks(id: number): Promise<Track[]> {
		const cached = playlistTracksById[id];
		if (cached) return cached;
		const playlist = playlists.find((p) => p.id === id);
		try {
			const data = playlist?.is_smart
				? await cachedApi.evaluateSmartPlaylist(id)
				: await cachedApi.getPlaylistTracks(id);
			if (destroyed) return [];
			playlistTracksById = { ...playlistTracksById, [id]: data.tracks };
			if (playlist) recordMosaic(id, data.tracks, playlist.track_count);
			return data.tracks;
		} catch (error) {
			if (destroyed) return [];
			deleteError = `Failed to load tracks: ${error}`;
			return [];
		}
	}

	async function playPlaylistQuick(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		const tracks = await ensurePlaylistTracks(playlist.id);
		if (!tracks.length) return;
		await playTracksInContext(tracks.map((t) => t.id));
	}

	async function shufflePlaylistQuick(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		const tracks = await ensurePlaylistTracks(playlist.id);
		await shufflePlaylist(tracks);
	}

	async function startPlaylistRadioQuick(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		const tracks = await ensurePlaylistTracks(playlist.id);
		await startPlaylistRadio(tracks);
	}

	async function togglePlaylistFavorite(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		// Optimistic flip - revert if the server disagrees.
		const optimistic = { ...playlist, is_favorite: !playlist.is_favorite };
		playlists = playlists.map((p) => (p.id === playlist.id ? optimistic : p));
		try {
			const updated = await api.togglePlaylistFavorite(playlist.id);
			invalidatePlaylistCaches();
			playlists = playlists.map((p) => (p.id === playlist.id ? updated.playlist : p));
		} catch {
			playlists = playlists.map((p) => (p.id === playlist.id ? playlist : p));
		}
	}

	function openPlaylistContextMenu(
		playlist: Playlist,
		event: MouseEvent | { clientX: number; clientY: number },
		anchorToButton = false,
	) {
		if ('preventDefault' in event && typeof event.preventDefault === 'function') {
			event.preventDefault();
		}
		if ('stopPropagation' in event && typeof event.stopPropagation === 'function') {
			event.stopPropagation();
		}
		// When triggered by the more-actions button, anchor the menu to the
		// button's position so it lines up with the click target rather than
		// the cursor coordinates from the synthetic MouseEvent.
		const anchor =
			anchorToButton && event instanceof MouseEvent
				? (event.currentTarget as HTMLElement | null)
				: null;
		const rect = anchor?.getBoundingClientRect();
		const point = rect
			? { clientX: rect.right, clientY: rect.bottom + 4 }
			: { clientX: event.clientX, clientY: event.clientY };
		openContextMenu(point, playlistMenuItems(playlist), playlist.name);
	}

	/**
	 * Built from the shared builder so this menu, the detail page's, and
	 * search's stay in step. The old inline version offered Delete only for
	 * smart playlists, because a regular one had no delete route to call.
	 */
	function playlistMenuItems(playlist: Playlist): MenuItem[] {
		return buildPlaylistMenu(playlist, {
			onPlay: () => void playPlaylistFromMenu(playlist),
			onShuffle: () => void shufflePlaylistFromMenu(playlist),
			onRadio: () => void radioFromMenu(playlist),
			onOpen: () => void goto(`/playlists/${playlist.id}`),
			onToggleFavorite: () => {
				// Synthesize a MouseEvent shape; togglePlaylistFavorite only calls
				// stopPropagation, which we no-op here.
				void togglePlaylistFavorite(playlist, { stopPropagation: () => {} } as MouseEvent);
			},
			onDownload: () => void downloadPlaylist(playlist.id),
			onRefreshFromTidal: () => void refreshPlaylistFromTidal(playlist),
			onEditRules: playlist.is_smart ? () => openEdit(playlist) : undefined,
			onDuplicate: playlist.is_smart ? () => void duplicatePlaylist(playlist) : undefined,
			onDelete: () => requestDelete(playlist.id),
		});
	}

	async function refreshPlaylistFromTidal(playlist: Playlist) {
		try {
			await api.refreshPlaylistFromTidal(playlist.id);
			invalidatePlaylistCaches();
			// Drop the cached tracks too so the quick actions and mosaic re-read.
			const { [playlist.id]: _stale, ...rest } = playlistTracksById;
			playlistTracksById = rest;
			await loadPlaylists();
		} catch (error) {
			deleteError = `Could not refresh from TIDAL: ${error}`;
		}
	}

	async function playPlaylistFromMenu(playlist: Playlist) {
		const tracks = await ensurePlaylistTracks(playlist.id);
		if (!tracks.length) return;
		await playTracksInContext(tracks.map((t) => t.id));
	}

	async function shufflePlaylistFromMenu(playlist: Playlist) {
		const tracks = await ensurePlaylistTracks(playlist.id);
		await shufflePlaylist(tracks);
	}

	async function radioFromMenu(playlist: Playlist) {
		const tracks = await ensurePlaylistTracks(playlist.id);
		await startPlaylistRadio(tracks);
	}

	// IntersectionObserver-backed mosaic loader - only fetches artwork for
	// cards the user can actually see, instead of hammering the API for
	// every smart playlist on mount.
	let mosaicObserver: IntersectionObserver | null = null;
	const fetchedMosaicIds = new Set<number>();

	function ensureMosaicObserver(): IntersectionObserver | null {
		if (typeof IntersectionObserver === 'undefined') return null;
		if (mosaicObserver) return mosaicObserver;
		mosaicObserver = new IntersectionObserver(
			(entries) => {
				for (const entry of entries) {
					if (!entry.isIntersecting) continue;
					const target = entry.target as HTMLElement;
					const id = Number(target.dataset.playlistId);
					if (!Number.isFinite(id) || fetchedMosaicIds.has(id)) continue;
					mosaicObserver?.unobserve(target);
					void hydrateMosaicFor(id);
				}
			},
			{ rootMargin: '200px 0px', threshold: 0.05 },
		);
		return mosaicObserver;
	}

	function registerCard(node: HTMLElement, playlistId: number) {
		node.dataset.playlistId = String(playlistId);
		const observer = ensureMosaicObserver();
		observer?.observe(node);
		return {
			destroy() {
				observer?.unobserve(node);
			},
		};
	}

	async function hydrateMosaicFor(id: number) {
		const playlist = playlists.find((p) => p.id === id);
		if (!playlist || playlist.track_count <= 0) return;
		if (mosaicById[id]?.length) return;
		const cached = getCachedMosaic(id, playlist.track_count);
		if (cached) {
			mosaicById = { ...mosaicById, [id]: cached };
			return;
		}
		if (fetchedMosaicIds.has(id)) return;
		fetchedMosaicIds.add(id);
		try {
			const { urls } = await cachedApi.getPlaylistCoverSample(id);
			if (destroyed) return;
			if (!urls.length) return;
			setCachedMosaic(id, urls, playlist.track_count);
			mosaicById = { ...mosaicById, [id]: urls };
		} catch {
			// Background task - leave the gradient fallback in place.
		}
	}

	// ─── Editor helpers ───────────────────────────────────────────────────────
	function openNew() {
		editorTriggerEl = document.activeElement as HTMLElement | null;
		editingPlaylistId = null;
		draftName = '';
		draftDescription = '';
		draftLogicOp = 'AND';
		draftClauses = [defaultDraft()];
		tagInputs = {};
		editorError = '';
		editorOpen = true;
		editorInitialSig = currentDraftSig();
		void loadGenreSuggestions();
	}

	function openEdit(playlist: Playlist) {
		editorTriggerEl = document.activeElement as HTMLElement | null;
		editingPlaylistId = playlist.id;
		draftName = playlist.name;
		draftDescription = playlist.description ?? '';
		editorError = '';

		try {
			const def = playlist.smart_rules ? JSON.parse(playlist.smart_rules) : null;
			if (def?.root?.type === 'group') {
				draftLogicOp = def.root.op as LogicOp;
				draftClauses = (def.root.clauses ?? []).map((c: RuleClause) => draftFromClause(c));
			} else if (def?.root) {
				draftLogicOp = 'AND';
				draftClauses = [draftFromClause(def.root as RuleClause)];
			} else {
				draftLogicOp = 'AND';
				draftClauses = [defaultDraft()];
			}
		} catch {
			draftLogicOp = 'AND';
			draftClauses = [defaultDraft()];
		}
		tagInputs = {};
		editorOpen = true;
		editorInitialSig = currentDraftSig();
		void loadGenreSuggestions();
	}

	function currentDraftSig(): string {
		// Stable serialization so reorder-free draft edits flip the dirty flag.
		return JSON.stringify({
			name: draftName.trim(),
			description: draftDescription.trim(),
			logic: draftLogicOp,
			clauses: draftClauses.map(draftToClause),
		});
	}

	function isEditorDirty(): boolean {
		return currentDraftSig() !== editorInitialSig;
	}

	function closeEditor(force = false) {
		if (!force && isEditorDirty()) {
			const ok = window.confirm('Discard unsaved changes to this playlist?');
			if (!ok) return;
		}
		editorOpen = false;
		resetPreview();
		const trigger = editorTriggerEl;
		queueMicrotask(() => trigger?.focus());
		editorTriggerEl = null;
	}

	async function loadGenreSuggestions() {
		if (genreSuggestionsLoaded) return;
		genreSuggestionsLoaded = true;
		try {
			const data = await cachedApi.getGenres();
			const names: string[] = [];
			const walk = (nodes: { name: string; children?: unknown[] }[]) => {
				for (const node of nodes) {
					if (node?.name) names.push(node.name);
					if (Array.isArray(node?.children)) {
						walk(node.children as { name: string; children?: unknown[] }[]);
					}
				}
			};
			walk(data.genres as unknown as { name: string; children?: unknown[] }[]);
			genreSuggestions = Array.from(new Set(names)).sort((a, b) => a.localeCompare(b));
		} catch {
			genreSuggestionsLoaded = false; // allow retry on next open
		}
	}

	async function refreshArtistSuggestions(q: string) {
		const query = q.trim();
		if (!query) {
			artistSuggestions = [];
			return;
		}
		const cached = artistCache.get(query.toLowerCase());
		if (cached) {
			artistSuggestions = cached;
			return;
		}
		artistFetchAbort?.abort();
		const ctrl = new AbortController();
		artistFetchAbort = ctrl;
		try {
			const data = await api.searchLibraryArtistNames(query, ctrl.signal, 20);
			if (ctrl.signal.aborted) return;
			const names = data.artists.map((a) => a.name);
			artistCache.set(query.toLowerCase(), names);
			artistSuggestions = names;
		} catch {
			// Soft fail - leave the previous suggestion list visible.
		}
	}

	function schedulePreview() {
		if (!editorOpen) return;
		if (previewDebounce) clearTimeout(previewDebounce);
		previewDebounce = setTimeout(() => {
			previewDebounce = null;
			void runPreview();
		}, 350);
	}

	async function runPreview() {
		if (!editorOpen) return;
		// Skip preview for drafts that won't pass server validation - it
		// would just round-trip a 400 every keystroke.
		if (draftClauses.length === 0) {
			previewCount = null;
			previewError = '';
			return;
		}
		if (validateDraft()) {
			previewCount = null;
			previewError = '';
			return;
		}
		const rootClause: RuleClause = {
			type: 'group',
			op: draftLogicOp,
			clauses: draftClauses.map(draftToClause),
		};
		previewAbort?.abort();
		const ctrl = new AbortController();
		previewAbort = ctrl;
		previewLoading = true;
		previewError = '';
		try {
			const data = await api.previewSmartPlaylist(rootClause, ctrl.signal);
			if (ctrl.signal.aborted) return;
			previewCount = data.count;
		} catch (e) {
			if (ctrl.signal.aborted) return;
			previewError = String(e);
			previewCount = null;
		} finally {
			if (!ctrl.signal.aborted) previewLoading = false;
		}
	}

	function resetPreview() {
		previewAbort?.abort();
		if (previewDebounce) {
			clearTimeout(previewDebounce);
			previewDebounce = null;
		}
		previewCount = null;
		previewError = '';
		previewLoading = false;
	}

	$effect(() => {
		if (!editorOpen) return;
		// Touch the reactive bits we care about so the effect re-runs.
		void draftClauses;
		void draftLogicOp;
		schedulePreview();
	});

	function addClause() {
		draftClauses = [...draftClauses, defaultDraft()];
	}

	function removeClause(id: number) {
		draftClauses = draftClauses.filter((c) => c.id !== id);
	}

	function addTag(clauseId: number) {
		const raw = (tagInputs[clauseId] ?? '').trim();
		if (!raw) return;
		const clause = draftClauses.find((c) => c.id === clauseId);
		if (!clause) return;
		const tags = raw.split(',').map((t) => t.trim()).filter(Boolean);
		clause.names = [...new Set([...clause.names, ...tags])];
		draftClauses = [...draftClauses]; // trigger reactivity
		tagInputs = { ...tagInputs, [clauseId]: '' };
	}

	function onTagInput(clauseId: number, e: Event) {
		// Auto-tokenize on comma so "rock, pop, soul" splits as the user types,
		// rather than requiring Enter / Add.
		const target = e.target as HTMLInputElement;
		const value = target.value;
		const clause = draftClauses.find((c) => c.id === clauseId);
		if (clause?.type === 'artist') {
			const q = value.replace(/,.*$/, '').trim();
			if (q !== artistQuery) {
				artistQuery = q;
				void refreshArtistSuggestions(q);
			}
		}
		if (!value.includes(',')) return;
		const parts = value.split(',');
		const remainder = parts.pop() ?? '';
		if (!clause) return;
		const tags = parts.map((t) => t.trim()).filter(Boolean);
		if (tags.length) {
			clause.names = [...new Set([...clause.names, ...tags])];
			draftClauses = [...draftClauses];
		}
		tagInputs = { ...tagInputs, [clauseId]: remainder };
	}

	function removeTag(clauseId: number, tag: string) {
		const clause = draftClauses.find((c) => c.id === clauseId);
		if (!clause) return;
		clause.names = clause.names.filter((n) => n !== tag);
		draftClauses = [...draftClauses];
	}

	function toggleExcludePlaylist(clauseId: number, playlistId: number) {
		const clause = draftClauses.find((c) => c.id === clauseId);
		if (!clause) return;
		const ids = clause.exclude_playlist_ids;
		clause.exclude_playlist_ids = ids.includes(playlistId)
			? ids.filter((id) => id !== playlistId)
			: [...ids, playlistId];
		draftClauses = [...draftClauses];
	}

	async function saveEditor() {
		const name = draftName.trim();
		if (!name) { editorError = 'Name is required.'; return; }
		if (draftClauses.length === 0) { editorError = 'Add at least one rule.'; return; }
		const validationError = validateDraft();
		if (validationError) { editorError = validationError; return; }

		const rootClause: RuleClause = {
			type: 'group',
			op: draftLogicOp,
			clauses: draftClauses.map(draftToClause),
		};

		editorSaving = true;
		editorError = '';
		try {
			const desc = draftDescription.trim() || null;
			if (editingPlaylistId === null) {
				const result = await api.createSmartPlaylist(name, desc, rootClause);
				playlists = [...playlists, result.playlist];
				indexPlaylistSearch(result.playlist);
			} else {
				const result = await api.updateSmartPlaylist(editingPlaylistId, name, desc, rootClause);
				playlists = playlists.map((p) => (p.id === editingPlaylistId ? result.playlist : p));
				indexPlaylistSearch(result.playlist);
				// Invalidate cached tracks so re-expand re-evaluates
				const { [editingPlaylistId]: _removed, ...rest } = playlistTracksById;
				playlistTracksById = rest;
			}
			invalidatePlaylistCaches();
			closeEditor(true);
		} catch (e) {
			editorError = String(e);
		} finally {
			editorSaving = false;
		}
	}

	function requestDelete(id: number) {
		pendingDeleteId = id;
		deleteError = '';
	}

	function cancelDelete() {
		pendingDeleteId = null;
	}

	async function confirmDelete(id: number) {
		deletingId = id;
		deleteError = '';
		try {
			// The generic route, not deleteSmartPlaylist: this now deletes regular
			// and TIDAL-mirrored playlists too, and the server pushes the delete
			// to TIDAL first so the next sync does not bring it back.
			await api.deletePlaylist(id);
			invalidatePlaylistCaches();
			playlists = playlists.filter((p) => p.id !== id);
			pendingDeleteId = null;
		} catch (e) {
			deleteError = String(e);
		} finally {
			deletingId = null;
		}
	}

	async function duplicatePlaylist(playlist: Playlist) {
		if (!playlist.is_smart) return;
		const def = parseSmartDef(playlist.smart_rules);
		const root = def?.root as RuleClause | undefined;
		if (!root) return;
		try {
			const result = await api.createSmartPlaylist(
				`${playlist.name} (copy)`,
				playlist.description ?? null,
				root
			);
			invalidatePlaylistCaches();
			playlists = [...playlists, result.playlist];
			indexPlaylistSearch(result.playlist);
		} catch {
			// Surface via the deleteError bar - it's the existing inline error channel.
			deleteError = 'Could not duplicate that playlist.';
		}
	}

	function indexPlaylistSearch(playlist: Playlist) {
		searchTextById = { ...searchTextById, [playlist.id]: buildSearchText(playlist) };
	}

	function buildSearchText(playlist: Playlist): string {
		const def = parseSmartDef(playlist.smart_rules);
		const rules = def?.root ? describeClause(def.root).join(' ') : '';
		return [
			playlist.name,
			playlist.description ?? '',
			playlist.is_smart ? 'smart rules' : 'regular synced playlist',
			rules,
		]
			.join(' ')
			.toLowerCase();
	}

	// ─── Derived display helpers ──────────────────────────────────────────────
	function smartCount() { return playlists.filter((p) => p.is_smart).length; }
	function regularCount() { return playlists.length - smartCount(); }

	function parseSmartDef(raw: string | null | undefined) {
		if (!raw) return null;
		try { return JSON.parse(raw); } catch { return null; }
	}

	function describeClause(clause: {
		type: string;
		op?: string;
		clauses?: unknown[];
		names?: string[];
		match_descendants?: boolean;
		field?: string;
		range?: { start?: string | null; end?: string | null };
		value?: number;
		value_max?: number | null;
		op_label?: string;
		minimum?: string;
		playlist_ids?: number[];
		min?: number | null;
		max?: number | null;
		key?: string;
		is_instrumental?: boolean;
		source?: string | null;
	}): string[] {
		const fmtRange = (label: string, min: number | null | undefined, max: number | null | undefined, digits = 0) => {
			const lo = min == null ? 'any' : min.toFixed(digits);
			const hi = max == null ? 'any' : max.toFixed(digits);
			return `${label}: ${lo} - ${hi}`;
		};
		switch (clause.type) {
			case 'group': {
				const label = (clause.op ?? 'AND').toUpperCase() === 'OR' ? 'Any of' : 'All of';
				const children = (clause.clauses ?? []).flatMap((c) => describeClause(c as typeof clause).map((l) => `  ${l}`));
				return [`${label}:`, ...children];
			}
			case 'genre': return [`Genre: ${(clause.names ?? []).join(', ') || 'any'}${clause.match_descendants ? ' (+ descendants)' : ''}`];
			case 'artist': return [`Artist: ${(clause.names ?? []).join(', ') || 'any'}`];
			case 'date_range': {
				const f = clause.field === 'last_played_at' ? 'Last played' : 'Date added';
				return [`${f}: ${clause.range?.start ?? '-'} -> ${clause.range?.end ?? 'now'}`];
			}
			case 'play_count': return [`Play count: ${clause.op ?? '>='} ${clause.value ?? 0}${clause.value_max != null ? ` - ${clause.value_max}` : ''}`];
			case 'quality': return [`Min quality: ${clause.minimum ?? '?'}`];
			case 'not_in_playlist': return [`Exclude from playlists: ${(clause.playlist_ids ?? []).join(', ') || 'none'}`];
			case 'bpm_range': return [fmtRange('BPM', clause.min, clause.max, 0)];
			case 'energy_range': return [fmtRange('Energy', clause.min, clause.max, 2)];
			case 'danceability_range': return [fmtRange('Danceability', clause.min, clause.max, 2)];
			case 'key_signature': return [`Key: ${clause.key ?? '?'}`];
			case 'camelot_key': return [`Camelot: ${clause.key ?? '?'}`];
			case 'instrumental_only': return [clause.is_instrumental ? 'Instrumentals only' : 'Vocals only'];
			case 'has_sample_data': return [`Sample data: ${clause.source ?? 'any source'}`];
			default: return [`Unknown rule: ${clause.type}`];
		}
	}

	function smartSummaryLines(playlist: Playlist): string[] {
		const def = parseSmartDef(playlist.smart_rules);
		if (!def?.root) return [];
		const lines: string[] = [];
		if (def.description?.trim()) lines.push(def.description.trim());
		lines.push(...describeClause(def.root));
		return lines.slice(0, 4);
	}

	function smartRuleSummary(playlist: Playlist): string | null {
		if (!playlist.is_smart) return null;
		const def = parseSmartDef(playlist.smart_rules);
		const root = def?.root;
		if (!root) return 'Smart playlist';
		if (root.type === 'group') {
			const op = (root.op ?? 'AND').toUpperCase() === 'OR' ? 'ANY' : 'ALL';
			const count = Array.isArray(root.clauses) ? root.clauses.length : 0;
			return `${count} rule${count === 1 ? '' : 's'} - ${op}`;
		}
		return '1 rule';
	}

	const RULE_TYPE_LABELS: Record<string, string> = {
		genre: 'Genre',
		artist: 'Artist',
		date_range: 'Date range',
		play_count: 'Play count',
		quality: 'Quality',
		not_in_playlist: 'Not in playlist',
		bpm_range: 'BPM range',
		key_signature: 'Key signature',
		camelot_key: 'Camelot key',
		energy_range: 'Energy',
		danceability_range: 'Danceability',
		instrumental_only: 'Instrumental',
		has_sample_data: 'Sample data',
	};

	const CAMELOT_KEYS: string[] = Array.from({ length: 12 }, (_, i) => `${i + 1}A`).concat(
		Array.from({ length: 12 }, (_, i) => `${i + 1}B`),
	);

	const KEY_SIGNATURES: string[] = [
		'C', 'C#', 'D', 'D#', 'E', 'F', 'F#', 'G', 'G#', 'A', 'A#', 'B',
		'Cm', 'C#m', 'Dm', 'D#m', 'Em', 'Fm', 'F#m', 'Gm', 'G#m', 'Am', 'A#m', 'Bm',
	];

	const NUMBER_OP_LABELS: Record<string, string> = {
		eq: '= (exactly)',
		gte: '>= (at least)',
		lte: '<= (at most)',
		gt: '> (more than)',
		lt: '< (fewer than)',
		between_inclusive: 'between',
	};

	let otherPlaylists = $derived(playlists.filter((p) => !p.is_smart));
	let activeFilterLabel = $derived(
		playlistFilter === 'all'
			? 'All'
			: playlistFilter === 'favorites'
				? 'Favorites'
				: playlistFilter === 'smart'
					? 'Smart'
					: 'Regular',
	);
	let filteredPlaylists = $derived.by(() => {
		const q = playlistQuery.trim().toLowerCase();
		return [...playlists]
			.filter((playlist) => {
				if (playlistFilter === 'favorites' && !playlist.is_favorite) return false;
				if (playlistFilter === 'smart' && !playlist.is_smart) return false;
				if (playlistFilter === 'regular' && playlist.is_smart) return false;
				if (!q) return true;
				return playlistSearchText(playlist).includes(q);
			})
			.sort(comparePlaylists);
	});
	let visibleTrackTotal = $derived(
		filteredPlaylists.reduce((sum, playlist) => sum + Math.max(playlist.track_count ?? 0, 0), 0),
	);

	function playlistSearchText(playlist: Playlist): string {
		return searchTextById[playlist.id] ?? buildSearchText(playlist);
	}

	function comparePlaylists(a: Playlist, b: Playlist): number {
		if (playlistSort === 'name') return a.name.localeCompare(b.name);
		if (playlistSort === 'tracks') return b.track_count - a.track_count || a.name.localeCompare(b.name);
		if (playlistSort === 'type') {
			const typeDiff = Number(b.is_smart) - Number(a.is_smart);
			return typeDiff || a.name.localeCompare(b.name);
		}
		if (playlistSort === 'recent_update') {
			return (b.updated_at ?? '').localeCompare(a.updated_at ?? '') || a.name.localeCompare(b.name);
		}
		if (playlistSort === 'recent_create') {
			return (b.created_at ?? '').localeCompare(a.created_at ?? '') || a.name.localeCompare(b.name);
		}
		const favoriteDiff = Number(b.is_favorite) - Number(a.is_favorite);
		return favoriteDiff || a.name.localeCompare(b.name);
	}

	function filterCount(filter: PlaylistFilter): number {
		if (filter === 'favorites') return playlists.filter((p) => p.is_favorite).length;
		if (filter === 'smart') return smartCount();
		if (filter === 'regular') return regularCount();
		return playlists.length;
	}

	function playlistSourceLabel(playlist: Playlist): string {
		if (playlist.is_smart) return 'Smart';
		if (playlist.tidal_uuid) return 'TIDAL';
		return 'Local';
	}

	function clearPlaylistSearch() {
		playlistQuery = '';
		playlistFilter = 'all';
	}

	function clauseValidation(clause: DraftClause): string | null {
		if ((clause.type === 'genre' || clause.type === 'artist') && clause.names.length === 0) {
			return `Add at least one ${clause.type}.`;
		}
		if (clause.type === 'date_range' && clause.date_start && clause.date_end && clause.date_start > clause.date_end) {
			return 'The start date must be before the end date.';
		}
		if (clause.type === 'play_count') {
			if (clause.play_count_value < 0 || clause.play_count_max < 0) return 'Play counts cannot be negative.';
			if (clause.play_count_op === 'between_inclusive' && clause.play_count_max < clause.play_count_value) {
				return 'The upper play-count value must be greater than the lower value.';
			}
		}
		if (clause.type === 'not_in_playlist' && clause.exclude_playlist_ids.length === 0) {
			return 'Choose at least one playlist to exclude.';
		}
		if (
			(clause.type === 'bpm_range' || clause.type === 'energy_range' || clause.type === 'danceability_range') &&
			clause.range_min != null &&
			clause.range_max != null &&
			clause.range_min > clause.range_max
		) {
			return 'The minimum value must be less than the maximum value.';
		}
		if (
			(clause.type === 'energy_range' || clause.type === 'danceability_range') &&
			[clause.range_min, clause.range_max].some((value) => value != null && (value < 0 || value > 1))
		) {
			return 'Use values between 0 and 1.';
		}
		if ((clause.type === 'key_signature' || clause.type === 'camelot_key') && !clause.key_value.trim()) {
			return 'Choose a key.';
		}
		return null;
	}

	function validateDraft(): string | null {
		const invalid = draftClauses.map(clauseValidation).find(Boolean);
		return invalid ?? null;
	}
</script>

<svelte:head>
	<title>Playlists | NOOR</title>
</svelte:head>

<svelte:window onkeydown={(e) => {
	if (e.key !== 'Escape') return;
	if (editorOpen) { closeEditor(); return; }
	if (pendingDeleteId !== null) { cancelDelete(); }
}} />

<div class="page-shell playlists-page animate-in">
	<PageHeader
		eyebrow="Playlists"
		title="Playlists"
		subtitle="Synced lists and rules-based smart sets."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={loadPlaylists}>Refresh</button>
			<button class="btn btn-primary" onclick={openNew}>New smart playlist</button>
		{/snippet}
	</PageHeader>

	<section class="playlist-control-band glass">
		<div class="playlist-search-wrap">
			<SearchField
				bind:value={playlistQuery}
				variant="page"
				fill
				placeholder="Search playlists, descriptions, or smart rules"
			>
				{#snippet trailing()}
					{#if playlistQuery.trim()}
						<button class="clear-search" onclick={() => (playlistQuery = '')}>Clear</button>
					{/if}
				{/snippet}
			</SearchField>
		</div>

		<div class="playlist-toolbar">
			<div class="filter-pills" aria-label="Playlist filters">
				{#each [
					{ value: 'all', label: 'All' },
					{ value: 'favorites', label: 'Favorites' },
					{ value: 'smart', label: 'Smart' },
					{ value: 'regular', label: 'Regular' },
				] as filter}
					<button
						class="filter-pill"
						class:active={playlistFilter === filter.value}
						onclick={() => (playlistFilter = filter.value as PlaylistFilter)}
					>
						{filter.label}
						<span>{filterCount(filter.value as PlaylistFilter)}</span>
					</button>
				{/each}
			</div>

			<div class="playlist-sort">
				<label class="field-label" for="playlist-sort">Sort</label>
				<select id="playlist-sort" bind:value={playlistSort}>
					<option value="default">Favorites, then name</option>
					<option value="recent_update">Recently updated</option>
					<option value="recent_create">Recently created</option>
					<option value="name">Name</option>
					<option value="tracks">Track count</option>
					<option value="type">Type</option>
				</select>
			</div>
		</div>

		<p class="playlist-result-copy">
			{#if playlistFilter === 'all' && !playlistQuery.trim()}
				{filteredPlaylists.length} playlist{filteredPlaylists.length === 1 ? '' : 's'} - {visibleTrackTotal.toLocaleString()} track{visibleTrackTotal === 1 ? '' : 's'}
			{:else}
				{filteredPlaylists.length} of {playlists.length} - {visibleTrackTotal.toLocaleString()} track{visibleTrackTotal === 1 ? '' : 's'}
			{/if}
		</p>
	</section>

	{#if deleteError}
		<div class="feedback-bar error glass">{deleteError}</div>
	{/if}

	{#if loadError}
		<EmptyState title="Playlists could not load" copy={loadError} />
	{:else if isLoading && playlists.length === 0}
		<EmptyState title="Loading playlists" copy="Pulling synced and smart playlists." />
	{:else if playlists.length > 0 && filteredPlaylists.length === 0}
		<EmptyState title="No playlists match" copy="Try a different search, filter, or sort mode.">
			{#snippet actions()}
				<button class="btn btn-glass" onclick={clearPlaylistSearch}>Reset filters</button>
			{/snippet}
		</EmptyState>
	{:else if playlists.length > 0}
		<div class="playlist-list">
			{#each filteredPlaylists as playlist (playlist.id)}
				{@const mosaic = mosaicById[playlist.id] ?? []}
				<div class="playlist-row" use:registerCard={playlist.id}>
					<a
						class="playlist-hit"
						href={`/playlists/${playlist.id}`}
						oncontextmenu={(e) => openPlaylistContextMenu(playlist, e)}
					>
						<div
							class="playlist-cover"
							class:has-mosaic={mosaic.length >= 4}
							style:background={mosaic.length === 0 ? nameToGradient(playlist.name) : undefined}
						>
							{#if mosaic.length >= 4}
								{#each mosaic.slice(0, 4) as url}
									<ArtworkImage className="playlist-cover-art" src={url} size={320} fallbackText="PL" decorative={true} />
								{/each}
							{:else if mosaic.length > 0}
								<ArtworkImage className="playlist-cover-art" src={mosaic[0]} size={320} fallbackText="PL" decorative={true} />
							{:else}
								<span class="playlist-initial">{playlist.name.trim().slice(0, 1).toUpperCase() || 'P'}</span>
							{/if}
						</div>
						<div class="playlist-meta">
							<h3>{playlist.name}</h3>
							<p class="playlist-copy">
								<span>{playlistSourceLabel(playlist)}</span>
								<span aria-hidden="true">&middot;</span>
								<span>{playlist.track_count.toLocaleString()} {playlist.track_count === 1 ? 'track' : 'tracks'}</span>
								{#if playlist.is_smart}
									{@const summary = smartRuleSummary(playlist)}
									{#if summary}
										<span aria-hidden="true">&middot;</span>
										<span title={smartSummaryLines(playlist).join('\n')}>{summary}</span>
									{/if}
								{/if}
							</p>
						</div>
					</a>

					{#if pendingDeleteId === playlist.id}
						<div class="confirm-strip" role="alertdialog" aria-label="Confirm delete">
							<span class="confirm-copy">Delete "{playlist.name}"?</span>
							<button class="btn btn-glass btn-sm" onclick={cancelDelete}>Cancel</button>
							<button
								class="btn btn-sm danger-solid"
								disabled={deletingId === playlist.id}
								onclick={() => void confirmDelete(playlist.id)}
							>{deletingId === playlist.id ? 'Deleting...' : 'Delete'}</button>
						</div>
					{:else}
						<div class="playlist-actions">
							<button class="row-btn" onclick={(e) => void playPlaylistQuick(playlist, e)} aria-label="Play {playlist.name}" title="Play">&#9654;</button>
							<button class="row-btn" onclick={(e) => void shufflePlaylistQuick(playlist, e)} aria-label="Shuffle {playlist.name}" title="Shuffle">&#10728;</button>
							<button class="row-btn" onclick={(e) => void startPlaylistRadioQuick(playlist, e)} aria-label="Start radio from {playlist.name}" title="Start radio">&#9673;</button>
							<button
								class="row-btn favorite-btn"
								class:active={playlist.is_favorite}
								onclick={(e) => void togglePlaylistFavorite(playlist, e)}
								aria-label={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
								aria-pressed={playlist.is_favorite}
								title={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
							>
								<svg width="14" height="14" viewBox="0 0 24 24" fill={playlist.is_favorite ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
									<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
								</svg>
							</button>
							<button
								class="row-btn more-btn"
								onclick={(e) => openPlaylistContextMenu(playlist, e, true)}
								aria-label="More actions for {playlist.name}"
								title="More actions"
							>
								<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
									<circle cx="5" cy="12" r="1.6" />
									<circle cx="12" cy="12" r="1.6" />
									<circle cx="19" cy="12" r="1.6" />
								</svg>
							</button>
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{:else}
		<EmptyState title="No playlists yet" copy="Connect TIDAL and sync to pull playlists, or create a smart playlist above." />
	{/if}
</div>

<!-- ─── Rule Editor Drawer ──────────────────────────────────────────────────── -->
{#if editorOpen}
	<button
		type="button"
		class="drawer-backdrop"
		aria-label="Close editor"
		onclick={() => closeEditor()}
	></button>
	<div
		class="editor-drawer glass-panel"
		role="dialog"
		aria-modal="true"
		aria-labelledby="editor-title"
		tabindex="-1"
		use:trapFocus
		onkeydown={(e) => {
			if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
				e.preventDefault();
				void saveEditor();
			}
		}}
	>
		<datalist id="noor-genre-suggestions">
			{#each genreSuggestions as g}
				<option value={g}></option>
			{/each}
		</datalist>
		<datalist id="noor-artist-suggestions">
			{#each artistSuggestions as a}
				<option value={a}></option>
			{/each}
		</datalist>
		<div class="editor-head">
			<h2 id="editor-title">
				{editingPlaylistId === null ? 'New smart playlist' : 'Edit smart playlist'}
			</h2>
			<button class="close-btn" onclick={() => closeEditor()} aria-label="Close editor">
				<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
					<path d="M18 6L6 18M6 6l12 12" />
				</svg>
			</button>
		</div>

		<div class="editor-body">
			<!-- Name + description -->
			<div class="field-group">
				<label class="field-label" for="draft-name">Name</label>
				<input
					id="draft-name"
					class="field-input"
					type="text"
					placeholder="e.g. Late Night Electronic"
					bind:value={draftName}
				/>
			</div>
			<div class="field-group">
				<label class="field-label" for="draft-desc">Description <span class="optional">(optional)</span></label>
				<textarea
					id="draft-desc"
					class="field-input field-textarea"
					rows="2"
					placeholder="Short note about this playlist"
					bind:value={draftDescription}
				></textarea>
			</div>

			<!-- Logic operator -->
			<div class="logic-bar">
				<span>Match</span>
				<div class="logic-toggle">
					<button
						class="logic-btn {draftLogicOp === 'AND' ? 'active' : ''}"
						onclick={() => (draftLogicOp = 'AND')}
					>All</button>
					<button
						class="logic-btn {draftLogicOp === 'OR' ? 'active' : ''}"
						onclick={() => (draftLogicOp = 'OR')}
					>Any</button>
				</div>
				<span>of the following rules:</span>
			</div>

			<!-- Rule clauses -->
			<div class="clause-list">
				{#each draftClauses as clause (clause.id)}
					<div class="clause-card glass">
						<div class="clause-head">
							<select class="type-select" bind:value={clause.type}>
								{#each Object.entries(RULE_TYPE_LABELS) as [value, label]}
									<option {value}>{label}</option>
								{/each}
							</select>
							<button class="remove-btn" onclick={() => removeClause(clause.id)} aria-label="Remove rule">
								<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
									<path d="M18 6L6 18M6 6l12 12" />
								</svg>
							</button>
						</div>

						<!-- Genre fields -->
						{#if clause.type === 'genre' || clause.type === 'artist'}
							<div class="tag-field">
								<div class="tag-list">
									{#each clause.names as tag}
										<span class="tag">
											{tag}
											<button class="tag-remove" aria-label="Remove tag" onclick={() => removeTag(clause.id, tag)}>
												<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
													<path d="M18 6L6 18M6 6l12 12" />
												</svg>
											</button>
										</span>
									{/each}
								</div>
								<div class="tag-input-row">
									<input
										class="field-input"
										type="text"
										list={clause.type === 'genre'
											? 'noor-genre-suggestions'
											: 'noor-artist-suggestions'}
										placeholder={clause.type === 'genre' ? 'Add genre (e.g. Electronic)' : 'Add artist name'}
										bind:value={tagInputs[clause.id]}
										oninput={(e) => onTagInput(clause.id, e)}
										onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addTag(clause.id); } }}
									/>
									<button class="btn btn-glass btn-sm" onclick={() => addTag(clause.id)}>Add</button>
								</div>
								{#if clause.type === 'genre'}
									<label class="check-row">
										<input type="checkbox" bind:checked={clause.match_descendants} />
										<span>Include descendant genres</span>
									</label>
								{/if}
							</div>
						{/if}

						<!-- Date range fields -->
						{#if clause.type === 'date_range'}
							<div class="date-fields">
								<select class="field-input" bind:value={clause.date_field}>
									<option value="date_added">Date added</option>
									<option value="last_played_at">Last played</option>
								</select>
								<div class="date-range-row">
									<div class="field-group">
										<label class="field-label" for="ds-{clause.id}">From</label>
										<input id="ds-{clause.id}" class="field-input" type="date" bind:value={clause.date_start} />
									</div>
									<div class="field-group">
										<label class="field-label" for="de-{clause.id}">To</label>
										<input id="de-{clause.id}" class="field-input" type="date" bind:value={clause.date_end} />
									</div>
								</div>
							</div>
						{/if}

						<!-- Play count fields -->
						{#if clause.type === 'play_count'}
							<div class="num-fields">
								<select class="field-input" bind:value={clause.play_count_op}>
									{#each Object.entries(NUMBER_OP_LABELS) as [value, label]}
										<option {value}>{label}</option>
									{/each}
								</select>
								<input class="field-input num-input" type="number" min="0" bind:value={clause.play_count_value} />
								{#if clause.play_count_op === 'between_inclusive'}
									<span class="and-label">and</span>
									<input class="field-input num-input" type="number" min="0" bind:value={clause.play_count_max} />
								{/if}
							</div>
						{/if}

						<!-- Quality field -->
						{#if clause.type === 'quality'}
							<select class="field-input" bind:value={clause.quality_minimum}>
								<option value="lossy">Lossy (any)</option>
								<option value="lossless">Lossless+</option>
								<option value="hi_res">Hi-Res only</option>
							</select>
						{/if}

						<!-- Not in playlist field -->
						{#if clause.type === 'not_in_playlist'}
							{#if otherPlaylists.length === 0}
								<p class="field-hint">No regular playlists found.</p>
							{:else}
								<div class="playlist-checklist">
									{#each otherPlaylists as pl}
										<label class="check-row">
											<input
												type="checkbox"
												checked={clause.exclude_playlist_ids.includes(pl.id)}
												onchange={() => toggleExcludePlaylist(clause.id, pl.id)}
											/>
											<span>{pl.name}</span>
										</label>
									{/each}
								</div>
							{/if}
						{/if}

						<!-- BPM range -->
						{#if clause.type === 'bpm_range'}
							<div class="num-fields">
								<div class="field-group">
									<label class="field-label" for="bpm-min-{clause.id}">Min BPM</label>
									<input
										id="bpm-min-{clause.id}"
										class="field-input num-input"
										type="number"
										min="0"
										step="1"
										placeholder="any"
										bind:value={clause.range_min}
									/>
								</div>
								<span class="and-label">to</span>
								<div class="field-group">
									<label class="field-label" for="bpm-max-{clause.id}">Max BPM</label>
									<input
										id="bpm-max-{clause.id}"
										class="field-input num-input"
										type="number"
										min="0"
										step="1"
										placeholder="any"
										bind:value={clause.range_max}
									/>
								</div>
							</div>
						{/if}

						<!-- Energy / Danceability (0-1) -->
						{#if clause.type === 'energy_range' || clause.type === 'danceability_range'}
							<div class="num-fields">
								<div class="field-group">
									<label class="field-label" for="r-min-{clause.id}">Min (0-1)</label>
									<input
										id="r-min-{clause.id}"
										class="field-input num-input"
										type="number"
										min="0"
										max="1"
										step="0.05"
										placeholder="any"
										bind:value={clause.range_min}
									/>
								</div>
								<span class="and-label">to</span>
								<div class="field-group">
									<label class="field-label" for="r-max-{clause.id}">Max (0-1)</label>
									<input
										id="r-max-{clause.id}"
										class="field-input num-input"
										type="number"
										min="0"
										max="1"
										step="0.05"
										placeholder="any"
										bind:value={clause.range_max}
									/>
								</div>
							</div>
						{/if}

						<!-- Key signature (musical letter notation) -->
						{#if clause.type === 'key_signature'}
							<select class="field-input" bind:value={clause.key_value}>
								<option value="">Select key...</option>
								{#each KEY_SIGNATURES as k}
									<option value={k}>{k}</option>
								{/each}
							</select>
						{/if}

						<!-- Camelot key -->
						{#if clause.type === 'camelot_key'}
							<select class="field-input" bind:value={clause.key_value}>
								<option value="">Select Camelot key...</option>
								{#each CAMELOT_KEYS as k}
									<option value={k}>{k}</option>
								{/each}
							</select>
						{/if}

						<!-- Instrumental only -->
						{#if clause.type === 'instrumental_only'}
							<label class="check-row">
								<input type="checkbox" bind:checked={clause.is_instrumental} />
								<span>{clause.is_instrumental ? 'Instrumentals only' : 'Vocals only'}</span>
							</label>
						{/if}

						<!-- Has sample data -->
						{#if clause.type === 'has_sample_data'}
							<select class="field-input" bind:value={clause.sample_source}>
								<option value="">Any source</option>
								<option value="fingerprint">Fingerprint</option>
							</select>
						{/if}

						{#if clauseValidation(clause)}
							<p class="clause-error">{clauseValidation(clause)}</p>
						{/if}
					</div>
				{/each}
			</div>

			<button class="btn btn-glass add-rule-btn" onclick={addClause}>+ Add rule</button>

			{#if editorError}
				<p class="editor-error">{editorError}</p>
			{/if}
		</div>

		<div class="editor-foot">
			<span class="editor-hint">
				{#if previewError}
					Preview unavailable
				{:else if previewLoading}
					Counting matches...
				{:else if previewCount !== null}
					Matches {previewCount.toLocaleString()} track{previewCount === 1 ? '' : 's'}
				{:else}
					Ctrl+Enter to save
				{/if}
			</span>
			<button class="btn btn-glass" onclick={() => closeEditor()}>Cancel</button>
			<button class="btn btn-primary" onclick={saveEditor} disabled={editorSaving}>
				{editorSaving ? 'Saving...' : editingPlaylistId === null ? 'Create playlist' : 'Save changes'}
			</button>
		</div>
	</div>
{/if}

<style>
	.playlist-control-band {
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding: 16px;
	}

	.playlist-search-wrap {
		position: relative;
		width: 100%;
	}

	.clear-search {
		flex-shrink: 0;
		padding: 4px 10px;
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}

	.clear-search:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.playlist-toolbar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
	}

	.filter-pills {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-wrap: wrap;
	}

	.filter-pill {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		height: var(--control-h);
		padding: 0 14px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		transition: background var(--motion-fast), border-color var(--motion-fast), color var(--motion-fast);
	}

	.filter-pill span {
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
	}

	.filter-pill:hover {
		border-color: var(--accent-line);
		color: var(--text-primary);
	}

	.filter-pill.active {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.filter-pill.active span { color: rgba(255,255,255,0.78); }

	.playlist-sort {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 240px;
	}

	.playlist-sort select {
		height: var(--control-h);
		padding: 0 10px;
		border-radius: 999px;
	}

	.playlist-result-copy {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	/* Borderless browse rows, matching /search. The old treatment wrapped each
	   playlist in .glass-panel (backdrop blur + 1px border + a 40px drop shadow)
	   around a bordered cover and five bordered pill buttons, which read as five
	   nested boxes per row. Here the artwork carries the weight, hover fills the
	   row, and the actions only appear when they are reachable. */
	.playlist-list {
		display: flex;
		flex-direction: column;
	}

	.playlist-row {
		display: grid;
		align-items: center;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: var(--space-3);
		padding: var(--space-2);
		border-radius: var(--radius-sm);
		transition: background var(--motion-base);
	}

	.playlist-row:hover,
	.playlist-row:focus-within {
		background: var(--bg-hover);
	}

	.playlist-hit {
		display: grid;
		align-items: center;
		grid-template-columns: auto minmax(0, 1fr);
		gap: var(--space-3);
		min-width: 0;
		border-radius: var(--radius-sm);
		color: inherit;
		text-decoration: none;
	}

	.playlist-hit:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 3px;
	}

	.playlist-cover {
		display: grid;
		width: clamp(2.75rem, 4vw, 3.5rem);
		aspect-ratio: 1 / 1;
		place-items: center;
		overflow: hidden;
		border-radius: var(--radius-sm);
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: box-shadow var(--motion-base);
	}

	.playlist-row:hover .playlist-cover {
		box-shadow: 0 8px 18px -4px rgba(0, 0, 0, 0.45);
	}

	.playlist-cover.has-mosaic {
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
	}

	.playlist-cover :global(.playlist-cover-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.playlist-cover :global(.playlist-cover-art.fallback) {
		display: grid;
		place-items: center;
		background: var(--bg-raised);
	}

	.playlist-initial {
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
	}

	.playlist-meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.playlist-meta h3 {
		margin: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-snug);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.playlist-copy {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	.playlist-actions {
		display: flex;
		align-items: center;
		gap: 2px;
	}

	/* .row-btn itself is the global utility in app.css; the row only has to say
	   when to reveal them. */
	.playlist-row:hover :global(.row-btn),
	.playlist-row:focus-within :global(.row-btn) {
		opacity: 1;
	}

	/* A favourited playlist has to read at a glance without hovering. */
	.playlist-row :global(.row-btn.favorite-btn.active) {
		opacity: 1;
		color: var(--state-favorite);
	}


	.clause-error {
		color: var(--state-error);
		font-size: var(--font-size-xs);
	}

	.feedback-bar {
		padding: 10px 14px;
		margin-bottom: var(--gap);
		font-size: var(--font-size-sm);
	}

	.feedback-bar.error {
		color: var(--state-error);
		border-color: color-mix(in srgb, var(--state-error) 28%, transparent);
	}

	/* ─── Drawer ──────────────────────────────────────────────── */

	.drawer-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		z-index: 300;
		backdrop-filter: blur(2px);
		border: none;
		padding: 0;
		cursor: pointer;
	}

	.drawer-backdrop:focus-visible {
		outline: none;
	}

	.editor-drawer {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		width: min(540px, 100vw);
		z-index: 400;
		display: flex;
		flex-direction: column;
		border-radius: var(--radius-lg) 0 0 var(--radius-lg);
		background: rgba(14, 14, 20, 0.98);
		border: 1px solid var(--border-strong);
		box-shadow: -8px 0 40px rgba(0, 0, 0, 0.5);
	}

	:global([data-theme="light"]) .editor-drawer {
		background: rgba(252, 252, 255, 0.98);
	}

	.editor-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 20px 24px 16px;
		border-bottom: 1px solid var(--border-subtle);
		flex-shrink: 0;
	}

	.editor-head h2 { font-size: var(--font-size-md); font-weight: var(--font-weight-semibold); }

	.close-btn {
		width: 32px;
		height: 32px;
		border-radius: 999px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: color-mix(in srgb, currentColor 8%, transparent);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		cursor: pointer;
		transition: background 150ms ease, color 150ms ease;
	}

	.close-btn:hover {
		background: color-mix(in srgb, currentColor 14%, transparent);
		color: var(--text-primary);
	}

	.close-btn:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	.editor-body {
		flex: 1;
		overflow-y: auto;
		padding: 20px 24px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}

	.editor-foot {
		padding: 16px 24px max(20px, var(--safe-bottom));
		border-top: 1px solid var(--border-subtle);
		display: flex;
		justify-content: flex-end;
		gap: 10px;
		flex-shrink: 0;
	}

	/* ─── Form elements ───────────────────────────────────────── */

	.field-group {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.field-label {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		color: var(--text-secondary);
	}

	.optional { font-weight: 400; color: var(--text-tertiary); }

	.field-input {
		background: color-mix(in srgb, currentColor 6%, transparent);
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		padding: 9px 12px;
		font-size: var(--font-size-sm);
		color: var(--text-primary);
		width: 100%;
		transition: border-color 150ms ease, box-shadow 150ms ease;
	}

	.field-input:focus-visible {
		outline: none;
		border-color: var(--accent-line);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}

	.field-textarea {
		min-height: 56px;
		resize: vertical;
		font-family: inherit;
		line-height: var(--line-height-snug);
	}

	.editor-hint {
		flex: 1;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	/* ─── Logic toggle ────────────────────────────────────────── */

	.logic-bar {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}

	.logic-toggle {
		display: flex;
		gap: 2px;
		padding: 2px;
		border-radius: 8px;
		background: color-mix(in srgb, currentColor 4%, transparent);
		border: 1px solid var(--border-subtle);
	}

	.logic-btn {
		padding: 5px 14px;
		border-radius: 6px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		background: transparent;
		border: none;
		transition: background 150ms ease, color 150ms ease;
		cursor: pointer;
	}

	.logic-btn:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	.logic-btn.active {
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	/* ─── Clause cards ────────────────────────────────────────── */

	.clause-list {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.clause-card {
		padding: 14px;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.clause-head {
		display: flex;
		align-items: center;
		gap: 10px;
	}

	.type-select {
		flex: 1;
		background: color-mix(in srgb, currentColor 6%, transparent);
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		padding: 7px 10px;
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.remove-btn {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: color-mix(in srgb, currentColor 6%, transparent);
		border: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		cursor: pointer;
		flex-shrink: 0;
		transition: background 150ms ease, color 150ms ease, border-color 150ms ease;
	}

	.remove-btn:hover {
		background: color-mix(in srgb, var(--state-error) 14%, transparent);
		border-color: color-mix(in srgb, var(--state-error) 30%, transparent);
		color: var(--state-error);
	}

	.remove-btn:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	/* ─── Tag input ───────────────────────────────────────────── */

	.tag-field { display: flex; flex-direction: column; gap: 8px; }

	.tag-list {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		min-height: 0;
	}

	.tag {
		display: flex;
		align-items: center;
		gap: 4px;
		padding: 4px 10px 4px 12px;
		border-radius: 999px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		font-size: var(--font-size-sm);
		color: var(--accent-strong);
	}

	.tag-remove {
		width: 16px;
		height: 16px;
		border-radius: 999px;
		background: color-mix(in srgb, currentColor 18%, transparent);
		border: none;
		color: inherit;
		cursor: pointer;
		flex-shrink: 0;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		transition: background 150ms ease;
	}

	.tag-remove:hover {
		background: color-mix(in srgb, currentColor 32%, transparent);
	}

	.tag-remove:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	.tag-input-row { display: flex; gap: 8px; }
	.tag-input-row .field-input { flex: 1; }

	.check-row {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		cursor: pointer;
	}

	.check-row input { accent-color: var(--accent); }

	/* ─── Date / number fields ────────────────────────────────── */

	.date-fields { display: flex; flex-direction: column; gap: 10px; }

	.date-range-row { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }

	.num-fields { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }

	.num-input { max-width: 90px; }

	.and-label { color: var(--text-tertiary); font-size: var(--font-size-sm); }

	/* ─── Playlist checklist ──────────────────────────────────── */

	.playlist-checklist { display: flex; flex-direction: column; gap: 6px; }

	.field-hint { font-size: var(--font-size-sm); color: var(--text-tertiary); }

	/* ─── Add rule button ─────────────────────────────────────── */

	.add-rule-btn { align-self: flex-start; }

	.editor-error { font-size: var(--font-size-sm); color: var(--state-error); }

	@media (max-width: 760px) {
		.playlist-control-band {
			padding: 12px;
		}

		.playlist-toolbar {
			display: flex;
			flex-direction: column;
			align-items: flex-start;
		}

		.filter-pills,
		.playlist-sort {
			width: 100%;
		}

		.filter-pill {
			flex: 1;
			justify-content: center;
		}

		.editor-drawer {
			width: 100vw;
			border-radius: var(--radius-lg) var(--radius-lg) 0 0;
			top: auto;
			height: 90dvh;
		}

		.date-range-row { grid-template-columns: 1fr; }
	}
</style>
