<script lang="ts">
	import { onMount } from 'svelte';
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
	import { formatDuration, getQualityClass } from '$lib/stores/library';
	import { addTrackToQueue, playTrackNow, shufflePlaylist, startPlaylistRadio } from '$lib/stores/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';

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

	// Phase 5B — back/forward state via SvelteKit snapshot.
	export const snapshot: Snapshot<{
		expandedIds: number[];
		scrollY: number;
	}> = {
		capture: () => ({
			expandedIds: [...expandedPlaylistIds],
			scrollY: typeof window !== 'undefined' ? window.scrollY : 0
		}),
		restore: (saved) => {
			if (Array.isArray(saved.expandedIds)) {
				expandedPlaylistIds = new Set(saved.expandedIds);
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

	// ─── Delete confirm ───────────────────────────────────────────────────────
	let deletingId = $state<number | null>(null);
	let deleteError = $state('');

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
		void loadPlaylists();
	});

	// ─── Data loading ─────────────────────────────────────────────────────────
	async function loadPlaylists() {
		isLoading = true;
		loadError = '';
		try {
			const data = await api.getPlaylists();
			playlists = data.playlists;
		} catch (error) {
			loadError = `Failed to load playlists: ${error}`;
		} finally {
			isLoading = false;
		}
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
		} catch (error) {
			errorById = { ...errorById, [id]: `Failed to load tracks: ${error}` };
			playlistTracksById = { ...playlistTracksById, [id]: [] };
		} finally {
			loadingById = { ...loadingById, [id]: false };
		}
	}

	async function playTrack(trackId: number) { await playTrackNow(trackId); }
	async function queueTrack(trackId: number, e: MouseEvent) {
		e.stopPropagation();
		await addTrackToQueue(trackId);
	}

	async function playPlaylist(id: number, e: MouseEvent) {
		e.stopPropagation();
		const tracks = playlistTracksById[id];
		if (!tracks?.length) return;
		await api.replacePlaybackQueue(tracks.map((t) => t.id));
		await playTrackNow(tracks[0].id);
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
	}

	function closeEditor() {
		editorOpen = false;
		const trigger = editorTriggerEl;
		queueMicrotask(() => trigger?.focus());
		editorTriggerEl = null;
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
			} else {
				const result = await api.updateSmartPlaylist(editingPlaylistId, name, desc, rootClause);
				playlists = playlists.map((p) => (p.id === editingPlaylistId ? result.playlist : p));
				// Invalidate cached tracks so re-expand re-evaluates
				const { [editingPlaylistId]: _removed, ...rest } = playlistTracksById;
				playlistTracksById = rest;
			}
			closeEditor();
		} catch (e) {
			editorError = String(e);
		} finally {
			editorSaving = false;
		}
	}

	async function confirmDelete(id: number) {
		deletingId = id;
		deleteError = '';
		try {
			await api.deleteSmartPlaylist(id);
			playlists = playlists.filter((p) => p.id !== id);
			expandedPlaylistIds = new Set([...expandedPlaylistIds].filter((x) => x !== id));
		} catch (e) {
			deleteError = String(e);
		} finally {
			deletingId = null;
		}
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
		if (!playlist.is_smart) return playlist.description?.trim() || 'Synced playlist.';
		const def = parseSmartDef(playlist.smart_rules);
		if (!def?.root) return 'Smart playlist';
		return describeClause(def.root)[0] ?? 'Smart playlist';
	}

	function smartSummaryLines(playlist: Playlist): string[] {
		const def = parseSmartDef(playlist.smart_rules);
		if (!def?.root) return [];
		const lines: string[] = [];
		if (def.description?.trim()) lines.push(def.description.trim());
		lines.push(...describeClause(def.root));
		return lines.slice(0, 4);
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
</script>

<svelte:head>
	<title>Playlists | NOOR</title>
</svelte:head>

<svelte:window onkeydown={(e) => { if (e.key === 'Escape' && editorOpen) closeEditor(); }} />

<div class="page-shell playlists-page animate-in">
	<PageHeader
		eyebrow="Playlists"
		title="Saved sets and smart curation."
		subtitle="Regular playlists and rules-based smart lists. Create smart playlists to auto-populate from your library using any combination of rules."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={loadPlaylists}>Refresh</button>
			<button class="btn btn-primary" onclick={openNew}>New smart playlist</button>
		{/snippet}
	</PageHeader>

	<section class="stat-grid">
		<MetricPair label="Total" value={playlists.length} copy="All synced and local playlists." />
		<MetricPair label="Smart" value={smartCount()} copy="Rules-based, always up to date." />
		<MetricPair label="Regular" value={regularCount()} copy="Standard curated playlists." />
	</section>

	{#if deleteError}
		<div class="feedback-bar error glass">{deleteError}</div>
	{/if}

	{#if loadError}
		<EmptyState title="Playlists could not load" copy={loadError} />
	{:else if isLoading}
		<EmptyState title="Loading playlists" copy="Pulling synced and smart playlists." />
	{:else if playlists.length > 0}
		<div class="playlist-list">
			{#each playlists as playlist (playlist.id)}
				<section class="playlist-card glass-panel">
					<button class="playlist-header" onclick={() => void expandPlaylist(playlist.id)}>
						<div class="playlist-meta">
							<div class="title-row">
								<h3>{playlist.name}</h3>
								<StateBadge label={playlist.is_smart ? 'Smart' : 'Playlist'} tone={playlist.is_smart ? 'active' : 'muted'} compact={true} />
							</div>
							<p class="playlist-copy">{playlistSubtitle(playlist)}</p>
							{#if playlist.is_smart}
								<div class="smart-summary">
									{#each smartSummaryLines(playlist).slice(0, 3) as line}
										<p>{line}</p>
									{/each}
								</div>
							{/if}
						</div>
						<div class="playlist-side">
							<span>{playlist.track_count} tracks</span>
							<svg
								class="chevron"
								class:open={isExpanded(playlist.id)}
								width="16"
								height="16"
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
								aria-hidden="true"
							>
								<path d="M6 9l6 6 6-6" />
							</svg>
						</div>
					</button>

					<div class="card-actions">
						<button
							class="action-btn fav-btn"
							class:active={playlist.is_favorite}
							onclick={async (e) => {
								e.stopPropagation();
								try {
									const updated = await api.togglePlaylistFavorite(playlist.id);
									playlists = playlists.map(p => p.id === playlist.id ? updated.playlist : p);
								} catch {
									// silently ignore — button state reverts on next data load
								}
							}}
							title={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
							aria-label={playlist.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
						>♥</button>
					</div>

					{#if playlist.is_smart}
						<div class="smart-actions">
							<button class="btn btn-glass btn-sm" onclick={(e) => { e.stopPropagation(); openEdit(playlist); }}>
								Edit rules
							</button>
							<button
								class="btn btn-glass btn-sm danger"
								disabled={deletingId === playlist.id}
								onclick={(e) => { e.stopPropagation(); void confirmDelete(playlist.id); }}
							>
								{deletingId === playlist.id ? 'Deleting…' : 'Delete'}
							</button>
						</div>
					{/if}

					{#if isExpanded(playlist.id)}
						<div class="playlist-body">
							{#if loadingById[playlist.id]}
								<p class="playlist-copy">Loading tracks…</p>
							{:else if errorById[playlist.id]}
								<p class="playlist-copy">{errorById[playlist.id]}</p>
							{:else if (playlistTracksById[playlist.id]?.length ?? 0) > 0}
								<div class="playlist-body-actions">
									<button class="btn btn-primary btn-sm" onclick={(e) => void playPlaylist(playlist.id, e)}>Play all</button>
									<button
										class="action-btn"
										onclick={(e) => {
											e.stopPropagation();
											void shufflePlaylist(playlistTracksById[playlist.id]);
										}}
										title="Shuffle playlist"
										aria-label="Shuffle playlist"
									>⤮ Shuffle</button>
									<button
										class="action-btn"
										onclick={(e) => {
											e.stopPropagation();
											void startPlaylistRadio(playlistTracksById[playlist.id]);
										}}
										title="Playlist radio"
										aria-label="Playlist radio"
									>◉ Radio</button>
								</div>
								<div class="track-list">
									{#each playlistTracksById[playlist.id] as track, i (`${track.id}-${i}`)}
										<div class="track-row">
											<button
												type="button"
												class="track-row-main"
												onclick={() => void playTrack(track.id)}
											>
												<span class="track-main">
													<h4>{track.title}</h4>
													<p>{track.artist_name ?? 'Unknown artist'}</p>
												</span>
											</button>
											<div class="track-side">
												{#if track.best_quality}
													<span class={`quality-badge ${getQualityClass(track.best_quality)}`}>
														{track.best_quality.replace(/_/g, ' ')}
													</span>
												{/if}
												<span>{formatDuration(track.duration_ms)}</span>
												<button
													type="button"
													class="queue-btn"
													aria-label="Add to queue"
													onclick={(e) => void queueTrack(track.id, e)}
												>
													<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
														<path d="M12 5v14M5 12h14" />
													</svg>
												</button>
											</div>
										</div>
									{/each}
								</div>
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
		onclick={closeEditor}
	></button>
	<div
		class="editor-drawer glass-panel"
		role="dialog"
		aria-modal="true"
		aria-labelledby="editor-title"
		tabindex="-1"
		use:trapFocus
	>
		<div class="editor-head">
			<h2 id="editor-title">
				{editingPlaylistId === null ? 'New smart playlist' : 'Edit smart playlist'}
			</h2>
			<button class="close-btn" onclick={closeEditor} aria-label="Close editor">
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
				<input
					id="draft-desc"
					class="field-input"
					type="text"
					placeholder="Short note about this playlist"
					bind:value={draftDescription}
				/>
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
										placeholder={clause.type === 'genre' ? 'Add genre (e.g. Electronic)' : 'Add artist name'}
										bind:value={tagInputs[clause.id]}
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
					</div>
				{/each}
			</div>

			<button class="btn btn-glass add-rule-btn" onclick={addClause}>+ Add rule</button>

			{#if editorError}
				<p class="editor-error">{editorError}</p>
			{/if}
		</div>

		<div class="editor-foot">
			<button class="btn btn-glass" onclick={closeEditor}>Cancel</button>
			<button class="btn btn-primary" onclick={saveEditor} disabled={editorSaving}>
				{editorSaving ? 'Saving…' : editingPlaylistId === null ? 'Create playlist' : 'Save changes'}
			</button>
		</div>
	</div>
{/if}

<style>
	.playlist-list {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.playlist-card {
		padding: 20px;
	}

	.playlist-header {
		width: 100%;
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		text-align: left;
	}

	.playlist-meta {
		display: flex;
		flex-direction: column;
		gap: 8px;
		min-width: 0;
	}

	.title-row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.playlist-copy,
	.smart-summary p,
	.track-main p,
	.track-side span {
		color: var(--text-secondary);
		font-size: 0.875rem;
	}

	.smart-summary {
		display: flex;
		flex-direction: column;
		gap: 3px;
	}

	.smart-summary p {
		font-size: 0.8125rem;
		font-family: var(--font-mono);
		color: var(--text-tertiary);
	}

	.playlist-side {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 6px;
		color: var(--text-tertiary);
		white-space: nowrap;
		font-size: 0.875rem;
	}

	.chevron {
		transition: transform 180ms ease;
	}

	.chevron.open {
		transform: rotate(180deg);
	}

	.smart-actions {
		display: flex;
		gap: 8px;
		margin-top: 14px;
		padding-top: 14px;
		border-top: 1px solid var(--border-subtle);
	}

	.btn-sm {
		padding: 5px 12px;
		font-size: 0.8125rem;
	}

	.danger {
		color: var(--state-error);
		border-color: color-mix(in srgb, var(--state-error) 28%, transparent);
	}

	.danger:hover:not(:disabled) {
		background: color-mix(in srgb, var(--state-error) 14%, transparent);
		border-color: color-mix(in srgb, var(--state-error) 45%, transparent);
	}

	.playlist-body {
		margin-top: 18px;
		padding-top: 18px;
		border-top: 1px solid var(--border-subtle);
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.playlist-body-actions {
		display: flex;
		gap: 8px;
	}

	.track-list {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
	}

	.track-row:last-child { border-bottom: none; }

	.track-row-main {
		flex: 1;
		min-width: 0;
		text-align: left;
		padding: 10px 0;
		background: transparent;
		border: none;
		color: inherit;
		cursor: pointer;
		border-radius: var(--radius-xs);
	}

	.track-row-main:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	.track-main {
		display: block;
		min-width: 0;
	}

	.track-side {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-shrink: 0;
		padding: 10px 0;
	}

	.queue-btn {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		background: var(--accent-soft);
		color: var(--accent-strong);
		border: 1px solid transparent;
		cursor: pointer;
		transition: background 150ms ease, border-color 150ms ease;
	}

	.queue-btn:hover {
		background: color-mix(in srgb, var(--accent) 22%, transparent);
		border-color: var(--accent-line);
	}

	.queue-btn:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: 2px;
	}

	.feedback-bar {
		padding: 10px 14px;
		margin-bottom: var(--gap);
		font-size: 0.875rem;
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

	.editor-head h2 { font-size: 1.0625rem; font-weight: 600; }

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
		font-size: 0.8125rem;
		font-weight: 500;
		color: var(--text-secondary);
	}

	.optional { font-weight: 400; color: var(--text-tertiary); }

	.field-input {
		background: color-mix(in srgb, currentColor 6%, transparent);
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		padding: 9px 12px;
		font-size: 0.875rem;
		color: var(--text-primary);
		width: 100%;
		transition: border-color 150ms ease, box-shadow 150ms ease;
	}

	.field-input:focus-visible {
		outline: none;
		border-color: var(--accent-line);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}

	/* ─── Logic toggle ────────────────────────────────────────── */

	.logic-bar {
		display: flex;
		align-items: center;
		gap: 10px;
		font-size: 0.875rem;
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
		font-size: 0.8125rem;
		font-weight: 600;
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
		font-size: 0.875rem;
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
		font-size: 0.8125rem;
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
		font-size: 0.8125rem;
		color: var(--text-secondary);
		cursor: pointer;
	}

	.check-row input { accent-color: var(--accent); }

	/* ─── Date / number fields ────────────────────────────────── */

	.date-fields { display: flex; flex-direction: column; gap: 10px; }

	.date-range-row { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }

	.num-fields { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }

	.num-input { max-width: 90px; }

	.and-label { color: var(--text-tertiary); font-size: 0.875rem; }

	/* ─── Playlist checklist ──────────────────────────────────── */

	.playlist-checklist { display: flex; flex-direction: column; gap: 6px; }

	.field-hint { font-size: 0.8125rem; color: var(--text-tertiary); }

	/* ─── Add rule button ─────────────────────────────────────── */

	.add-rule-btn { align-self: flex-start; }

	.editor-error { font-size: 0.875rem; color: var(--state-error); }

	/* ─── Card action buttons ────────────────────────────────── */

	.card-actions {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-top: 10px;
	}

	.action-btn {
		background: var(--surface-2, #2a2a2a);
		border: none;
		color: var(--text-secondary, #ccc);
		cursor: pointer;
		font-size: 12px;
		padding: 5px 10px;
		border-radius: 5px;
		display: flex;
		align-items: center;
		gap: 4px;
		white-space: nowrap;
	}
	.action-btn:hover {
		background: var(--surface-3, #333);
		color: var(--text-primary, #fff);
	}
	.action-btn.fav-btn {
		background: none;
		font-size: 16px;
		padding: 5px;
		color: var(--text-tertiary, #666);
	}
	.action-btn.fav-btn.active {
		color: var(--accent, #e00055);
	}

	/* ─── Responsive ──────────────────────────────────────────── */

	@media (max-width: 760px) {
		.playlist-header,
		.track-row {
			flex-direction: column;
			align-items: flex-start;
		}

		.playlist-side { align-items: flex-start; }

		.editor-drawer {
			width: 100vw;
			border-radius: var(--radius-lg) var(--radius-lg) 0 0;
			top: auto;
			height: 90dvh;
		}

		.date-range-row { grid-template-columns: 1fr; }
	}
</style>
