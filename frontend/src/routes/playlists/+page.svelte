<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import type { Snapshot } from './$types';
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
	import {
		currentTrack,
		isPlaying,
		playTrackNow,
		shuffleMode,
		shufflePlaylist,
		startPlaylistRadio,
	} from '$lib/stores/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import TrackRow from '$lib/components/TrackRow.svelte';
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
	let expandedPlaylistIds = $state<Set<number>>(new Set());
	let playlistTracksById = $state<Record<number, Track[]>>({});
	let loadingById = $state<Record<number, boolean>>({});
	let errorById = $state<Record<number, string | null>>({});
	let isLoading = $state(true);
	let loadError = $state('');
	let playlistQuery = $state('');
	let playlistFilter = $state<PlaylistFilter>('all');
	let playlistSort = $state<PlaylistSort>('default');

	// Cover mosaics keyed by playlist id. Seeded from localStorage on mount,
	// kept in sync whenever a playlist's tracks load.
	let mosaicById = $state<Record<number, string[]>>({});

	// Per-playlist pre-computed search blob, populated on load. Avoids
	// JSON.parse(smart_rules) for every playlist on every keystroke.
	let searchTextById = $state<Record<number, string>>({});

	// Inline delete confirmation - the card flips into a confirm bar instead
	// of firing the destructive call on the first click.
	let pendingDeleteId = $state<number | null>(null);

	// Set of playlist ids whose expanded track list is in "show all" mode.
	// The first paint after expand caps at TRACKS_PREVIEW_LIMIT to keep big
	// playlists from rendering hundreds of rows synchronously.
	let showAllTracksFor = $state<Set<number>>(new Set());
	const TRACKS_PREVIEW_LIMIT = 75;

	// Phase 5B - back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{
		expandedIds: number[];
		scrollY: number;
		query?: string;
		filter?: PlaylistFilter;
		sort?: PlaylistSort;
		showAllTracksIds?: number[];
	}> = {
		capture: () => ({
			expandedIds: [...expandedPlaylistIds],
			scrollY: typeof window !== 'undefined' ? window.scrollY : 0,
			query: playlistQuery,
			filter: playlistFilter,
			sort: playlistSort,
			showAllTracksIds: [...showAllTracksFor]
		}),
		restore: (saved) => {
			if (Array.isArray(saved.expandedIds)) {
				expandedPlaylistIds = new Set(saved.expandedIds);
			}
			if (typeof saved.query === 'string') playlistQuery = saved.query;
			if (saved.filter) playlistFilter = saved.filter;
			if (saved.sort) playlistSort = saved.sort;
			if (Array.isArray(saved.showAllTracksIds)) {
				showAllTracksFor = new Set(saved.showAllTracksIds);
			}
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
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
		isLoading = true;
		loadError = '';
		try {
			const data = await api.getPlaylists();
			playlists = data.playlists;
			const next: Record<number, string> = {};
			for (const p of playlists) next[p.id] = buildSearchText(p);
			searchTextById = next;
		} catch (error) {
			loadError = `Failed to load playlists: ${error}`;
		} finally {
			isLoading = false;
		}
	}

	function recordMosaic(id: number, tracks: Track[], trackCount: number) {
		const urls = pickArtworkUrls(tracks);
		if (urls.length === 0) return;
		setCachedMosaic(id, urls, trackCount);
		mosaicById = { ...mosaicById, [id]: urls };
	}

	function isExpanded(id: number) {
		return expandedPlaylistIds.has(id);
	}

	async function expandPlaylist(id: number) {
		const playlist = playlists.find((p) => p.id === id);
		const next = new Set(expandedPlaylistIds);
		const opening = !next.has(id);
		if (opening) next.add(id);
		else { next.delete(id); expandedPlaylistIds = next; return; }
		expandedPlaylistIds = next;

		if (playlistTracksById[id] || loadingById[id]) return;
		loadingById = { ...loadingById, [id]: true };
		errorById = { ...errorById, [id]: null };
		try {
			const data = playlist?.is_smart
				? await api.evaluateSmartPlaylist(id)
				: await api.getPlaylistTracks(id);
			playlistTracksById = { ...playlistTracksById, [id]: data.tracks };
			if (playlist) recordMosaic(id, data.tracks, playlist.track_count);
		} catch (error) {
			errorById = { ...errorById, [id]: `Failed to load tracks: ${error}` };
			playlistTracksById = { ...playlistTracksById, [id]: [] };
		} finally {
			loadingById = { ...loadingById, [id]: false };
		}
	}

	async function playTrack(trackId: number) { await playTrackNow(trackId); }

	async function ensurePlaylistTracks(id: number) {
		if (!playlistTracksById[id] && !loadingById[id]) {
			await expandPlaylist(id);
		}
		return playlistTracksById[id] ?? [];
	}

	async function playPlaylistQuick(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		const tracks = await ensurePlaylistTracks(playlist.id);
		if (!tracks.length) return;
		const replaced = await api.replacePlaybackQueue(
			tracks.map((t) => t.id),
			undefined,
			undefined,
			get(shuffleMode)
		);
		await playTrackNow(replaced.queue[0]?.track.id ?? tracks[0].id);
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

	function activatePlaylist(playlistId: number, e: KeyboardEvent) {
		if (e.key !== 'Enter' && e.key !== ' ') return;
		if (e.target !== e.currentTarget) return;
		e.preventDefault();
		void expandPlaylist(playlistId);
	}

	async function togglePlaylistFavorite(playlist: Playlist, e: MouseEvent) {
		e.stopPropagation();
		// Optimistic flip - revert if the server disagrees.
		const optimistic = { ...playlist, is_favorite: !playlist.is_favorite };
		playlists = playlists.map((p) => (p.id === playlist.id ? optimistic : p));
		try {
			const updated = await api.togglePlaylistFavorite(playlist.id);
			playlists = playlists.map((p) => (p.id === playlist.id ? updated.playlist : p));
		} catch {
			playlists = playlists.map((p) => (p.id === playlist.id ? playlist : p));
		}
	}

	function toggleShowAllTracks(id: number, on: boolean) {
		const next = new Set(showAllTracksFor);
		if (on) next.add(id);
		else next.delete(id);
		showAllTracksFor = next;
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
		openContextMenu(point, buildPlaylistMenu(playlist), playlist.name);
	}

	function buildPlaylistMenu(playlist: Playlist): MenuItem[] {
		const items: MenuItem[] = [
			{ label: 'Play', icon: 'P', onSelect: () => void playPlaylistFromMenu(playlist) },
			{ label: 'Shuffle', icon: 'X', onSelect: () => void shufflePlaylistFromMenu(playlist) },
			{ label: 'Radio', icon: 'R', onSelect: () => void radioFromMenu(playlist) },
			{ separator: true, label: '' },
			{
				label: playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites',
				icon: playlist.is_favorite ? 'F' : 'f',
				onSelect: () => {
					// Synthesize a fake MouseEvent shape; togglePlaylistFavorite
					// only uses stopPropagation, which we no-op here.
					void togglePlaylistFavorite(playlist, { stopPropagation: () => {} } as MouseEvent);
				},
			},
		];
		if (playlist.is_smart) {
			items.push({ separator: true, label: '' });
			items.push({ label: 'Edit rules', icon: 'E', onSelect: () => openEdit(playlist) });
			items.push({ label: 'Duplicate', icon: 'D', onSelect: () => void duplicatePlaylist(playlist) });
			items.push({
				label: 'Delete',
				icon: 'x',
				danger: true,
				onSelect: () => requestDelete(playlist.id),
			});
		}
		return items;
	}

	async function playPlaylistFromMenu(playlist: Playlist) {
		const tracks = await ensurePlaylistTracks(playlist.id);
		if (!tracks.length) return;
		const replaced = await api.replacePlaybackQueue(
			tracks.map((t) => t.id),
			undefined,
			undefined,
			get(shuffleMode),
		);
		await playTrackNow(replaced.queue[0]?.track.id ?? tracks[0].id);
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
			const data = playlist.is_smart
				? await api.evaluateSmartPlaylist(id)
				: await api.getPlaylistTracks(id);
			recordMosaic(id, data.tracks, playlist.track_count);
			if (!playlistTracksById[id]) {
				playlistTracksById = { ...playlistTracksById, [id]: data.tracks };
			}
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
		const trigger = editorTriggerEl;
		queueMicrotask(() => trigger?.focus());
		editorTriggerEl = null;
	}

	async function loadGenreSuggestions() {
		if (genreSuggestionsLoaded) return;
		genreSuggestionsLoaded = true;
		try {
			const data = await api.getGenres();
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
		if (!value.includes(',')) return;
		const parts = value.split(',');
		const remainder = parts.pop() ?? '';
		const clause = draftClauses.find((c) => c.id === clauseId);
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
			await api.deleteSmartPlaylist(id);
			playlists = playlists.filter((p) => p.id !== id);
			expandedPlaylistIds = new Set([...expandedPlaylistIds].filter((x) => x !== id));
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
			return `${label}: ${lo} – ${hi}`;
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
				return [`${f}: ${clause.range?.start ?? '—'} → ${clause.range?.end ?? 'now'}`];
			}
			case 'play_count': return [`Play count: ${clause.op ?? '≥'} ${clause.value ?? 0}${clause.value_max != null ? ` – ${clause.value_max}` : ''}`];
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

	function playlistSubtitle(playlist: Playlist): string {
		const desc = playlist.description?.trim();
		if (desc) return desc;
		if (!playlist.is_smart) return 'Synced playlist.';
		return 'Smart playlist';
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
		gte: '≥ (at least)',
		lte: '≤ (at most)',
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
			<input
				class="playlist-search-input"
				type="search"
				placeholder="Search playlists, descriptions, or smart rules"
				bind:value={playlistQuery}
			/>
			{#if playlistQuery.trim()}
				<button class="clear-search" onclick={() => (playlistQuery = '')}>Clear</button>
			{/if}
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
	{:else if isLoading}
		<EmptyState title="Loading playlists" copy="Pulling synced and smart playlists." />
	{:else if playlists.length > 0 && filteredPlaylists.length === 0}
		<EmptyState title="No playlists match" copy="Try a different search, filter, or sort mode.">
			{#snippet actions()}
				<button class="btn btn-glass" onclick={clearPlaylistSearch}>Reset filters</button>
			{/snippet}
		</EmptyState>
	{:else if playlists.length > 0}
		<div class="playlist-grid">
			{#each filteredPlaylists as playlist (playlist.id)}
				{@const mosaic = mosaicById[playlist.id] ?? []}
				<section class="playlist-card glass-panel">
					<div
						class="playlist-card-top"
						role="button"
						tabindex="0"
						aria-expanded={isExpanded(playlist.id)}
						aria-controls={`pl-body-${playlist.id}`}
						onclick={() => void expandPlaylist(playlist.id)}
						onkeydown={(e) => activatePlaylist(playlist.id, e)}
						oncontextmenu={(e) => openPlaylistContextMenu(playlist, e)}
						use:registerCard={playlist.id}
					>
						<div
							class="playlist-cover"
							class:has-mosaic={mosaic.length >= 4}
							class:has-solo={mosaic.length > 0 && mosaic.length < 4}
							style:background={mosaic.length === 0 ? nameToGradient(playlist.name) : undefined}
						>
							{#if mosaic.length >= 4}
								{#each mosaic.slice(0, 4) as url}
									<img src={url} alt="" loading="lazy" />
								{/each}
							{:else if mosaic.length > 0}
								<img class="cover-solo" src={mosaic[0]} alt="" loading="lazy" />
							{:else}
								<span>{playlist.name.trim().slice(0, 1).toUpperCase() || 'P'}</span>
							{/if}
						</div>
						<div class="playlist-meta">
							<div class="playlist-chip-row">
								<StateBadge label={playlistSourceLabel(playlist)} tone={playlist.is_smart ? 'active' : 'muted'} compact={true} />
								{#if playlist.is_smart}
									{@const summary = smartRuleSummary(playlist)}
									{#if summary}
										<span class="rule-chip" title={smartSummaryLines(playlist).join('\n')}>{summary}</span>
									{/if}
								{/if}
							</div>
							<h3>{playlist.name}</h3>
							<p class="playlist-copy">{playlistSubtitle(playlist)}</p>
						</div>
					</div>

					<div class="playlist-card-foot">
						<div class="playlist-count">
							<strong>{playlist.track_count.toLocaleString()}</strong>
							<span>tracks</span>
						</div>
						{#if pendingDeleteId === playlist.id}
							<div class="confirm-strip" role="alertdialog" aria-label="Confirm delete">
								<span class="confirm-copy">Delete "{playlist.name}"?</span>
								<button class="btn btn-glass btn-sm" onclick={(e) => { e.stopPropagation(); cancelDelete(); }}>Cancel</button>
								<button
									class="btn btn-sm danger-solid"
									disabled={deletingId === playlist.id}
									onclick={(e) => { e.stopPropagation(); void confirmDelete(playlist.id); }}
								>{deletingId === playlist.id ? 'Deleting...' : 'Delete'}</button>
							</div>
						{:else}
							<div class="playlist-actions">
								<button class="btn btn-primary btn-sm" onclick={(e) => void playPlaylistQuick(playlist, e)}>Play</button>
								<button class="btn btn-glass btn-sm" onclick={(e) => void shufflePlaylistQuick(playlist, e)}>Shuffle</button>
								<button class="btn btn-glass btn-sm" onclick={(e) => void startPlaylistRadioQuick(playlist, e)}>Radio</button>
								<button
									class="icon-btn favorite-btn"
									class:active={playlist.is_favorite}
									onclick={(e) => void togglePlaylistFavorite(playlist, e)}
									aria-label={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
									title={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
								>
									<svg width="14" height="14" viewBox="0 0 24 24" fill={playlist.is_favorite ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
										<path d="M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" />
									</svg>
								</button>
								<button
									class="icon-btn more-btn"
									onclick={(e) => openPlaylistContextMenu(playlist, e, true)}
									aria-label="More actions"
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

					{#if isExpanded(playlist.id)}
						<div class="playlist-body" id={`pl-body-${playlist.id}`}>
							{#if loadingById[playlist.id]}
								<p class="playlist-copy">Loading tracks...</p>
							{:else if errorById[playlist.id]}
								<p class="playlist-copy">{errorById[playlist.id]}</p>
							{:else if (playlistTracksById[playlist.id]?.length ?? 0) > 0}
								{@const allTracks = playlistTracksById[playlist.id] ?? []}
								{@const showAll = showAllTracksFor.has(playlist.id)}
								{@const visibleTracks = showAll ? allTracks : allTracks.slice(0, TRACKS_PREVIEW_LIMIT)}
								<ol class="track-list">
									{#each visibleTracks as track, i (`${track.id}-${i}`)}
										<TrackRow
											{track}
											variant="art"
											index={i}
											isCurrent={$currentTrack?.id === track.id}
											isPlaying={$isPlaying}
											onRowClick={() => void playTrack(track.id)}
										/>
									{/each}
								</ol>
								{#if allTracks.length > TRACKS_PREVIEW_LIMIT}
									<div class="track-list-more">
										{#if showAll}
											<button class="btn btn-glass btn-sm" onclick={() => toggleShowAllTracks(playlist.id, false)}>Show less</button>
										{:else}
											<button class="btn btn-glass btn-sm" onclick={() => toggleShowAllTracks(playlist.id, true)}>
												Show all {allTracks.length.toLocaleString()}
											</button>
										{/if}
									</div>
								{/if}
							{:else}
								<p class="playlist-copy">No tracks resolved for this playlist.</p>
							{/if}
						</div>
					{/if}
				</section>
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
										list={clause.type === 'genre' ? 'noor-genre-suggestions' : undefined}
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

						<!-- Energy / Danceability (0–1) -->
						{#if clause.type === 'energy_range' || clause.type === 'danceability_range'}
							<div class="num-fields">
								<div class="field-group">
									<label class="field-label" for="r-min-{clause.id}">Min (0–1)</label>
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
									<label class="field-label" for="r-max-{clause.id}">Max (0–1)</label>
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
								<option value="">Select key…</option>
								{#each KEY_SIGNATURES as k}
									<option value={k}>{k}</option>
								{/each}
							</select>
						{/if}

						<!-- Camelot key -->
						{#if clause.type === 'camelot_key'}
							<select class="field-input" bind:value={clause.key_value}>
								<option value="">Select Camelot key…</option>
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
								<option value="acrcloud">ACRCloud</option>
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
			<span class="editor-hint">Ctrl+Enter to save</span>
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

	.playlist-search-input {
		width: 100%;
		background: var(--bg-raised);
		border: 1px solid var(--border-strong);
		border-radius: 20px;
		padding: 12px 76px 12px 18px;
		color: var(--text-primary);
	}

	.playlist-search-input:focus {
		border-color: var(--accent);
		background: var(--bg-elevated);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}

	.clear-search {
		position: absolute;
		right: 10px;
		top: 50%;
		transform: translateY(-50%);
		padding: 4px 10px;
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
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
		padding: 5px 13px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
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
		height: 34px;
		padding: 6px 10px;
		border-radius: 999px;
	}

	.playlist-result-copy {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.playlist-grid {
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.playlist-card {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: 14px 18px;
		padding: 14px 16px;
		border-radius: 10px;
	}

	.playlist-card-top {
		display: grid;
		grid-template-columns: 50px minmax(0, 1fr) auto;
		gap: 12px;
		align-items: center;
		min-width: 0;
		cursor: pointer;
		border-radius: 8px;
		padding: 4px;
		margin: -4px;
		transition: background var(--motion-fast);
	}

	.playlist-card-top:hover,
	.playlist-card-top:focus-visible {
		background: var(--bg-hover);
		outline: none;
	}

	.playlist-card-top[aria-expanded="true"] {
		background: var(--accent-soft);
	}

	.playlist-cover {
		width: 50px;
		height: 50px;
		border-radius: 7px;
		display: grid;
		place-items: center;
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08);
		flex-shrink: 0;
	}

	.playlist-cover span {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
		color: #fff;
		text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
	}

	.playlist-cover.has-mosaic {
		display: grid;
		grid-template-columns: 1fr 1fr;
		grid-template-rows: 1fr 1fr;
		gap: 0;
		place-items: stretch;
		background: var(--bg-raised);
	}

	.playlist-cover.has-mosaic img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.playlist-cover.has-solo .cover-solo {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.playlist-meta {
		display: flex;
		flex-direction: column;
		gap: 5px;
		min-width: 0;
	}

	.playlist-chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: 5px;
	}

	.playlist-meta h3 {
		margin: 0;
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.playlist-copy {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.icon-btn {
		min-width: 36px;
		height: 30px;
		padding: 0 10px;
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-bold);
	}

	.icon-btn:hover,
	.icon-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.playlist-card-foot {
		align-self: center;
		display: grid;
		grid-template-columns: auto minmax(260px, auto);
		align-items: center;
		gap: 14px;
	}

	.playlist-count {
		display: flex;
		flex-direction: column;
		min-width: 58px;
	}

	.playlist-count strong {
		font-size: var(--font-size-md);
		font-variant-numeric: tabular-nums;
	}

	.playlist-count span {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.07em;
	}

	.playlist-actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: flex-end;
		gap: 6px;
	}

	.rule-chip {
		display: inline-flex;
		align-items: center;
		padding: 2px 9px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: color-mix(in srgb, currentColor 6%, transparent);
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.05em;
		font-variant-numeric: tabular-nums;
		cursor: help;
	}

	.confirm-strip {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
		flex-wrap: wrap;
	}

	.confirm-copy {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.danger-solid {
		background: var(--state-error);
		border: 1px solid var(--state-error);
		color: #fff;
	}

	.danger-solid:hover:not(:disabled) {
		filter: brightness(1.08);
	}

	.more-btn,
	.favorite-btn {
		width: 30px;
		min-width: 30px;
		padding: 0;
		display: inline-flex;
		align-items: center;
		justify-content: center;
	}

	.track-list-more {
		display: flex;
		justify-content: center;
		padding-top: 6px;
	}

	.playlist-actions .btn-sm {
		min-height: 30px;
		padding: 5px 10px;
	}

	.btn-sm {
		padding: 5px 12px;
		font-size: var(--font-size-sm);
	}

	.playlist-body {
		grid-column: 1 / -1;
		padding-top: 12px;
		border-top: 1px solid var(--border-subtle);
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.track-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
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

	@media (max-width: 1120px) {
		.playlist-card {
			grid-template-columns: 1fr;
		}

		.playlist-card-foot {
			display: flex;
			justify-content: space-between;
			width: 100%;
		}
	}

	@media (max-width: 760px) {
		.playlist-card {
			display: flex;
			flex-direction: column;
		}

		.playlist-control-band {
			padding: 12px;
		}

		.playlist-toolbar,
		.playlist-card-foot {
			display: flex;
			flex-direction: column;
			align-items: flex-start;
		}

		.filter-pills,
		.playlist-sort,
		.playlist-actions {
			width: 100%;
		}

		.filter-pill {
			flex: 1;
			justify-content: center;
		}

		.playlist-card-top {
			grid-template-columns: 50px minmax(0, 1fr);
		}

		.favorite-btn {
			grid-column: 1 / -1;
			justify-self: flex-start;
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
