<script lang="ts">
	import { onMount } from 'svelte';
	import {
		api,
		type Playlist,
		type Track,
		type RuleClause,
		type LogicOp,
		type NumberOp,
		type QualityTier,
		type DateField,
	} from '$lib/api/client';
	import { formatDuration, getQualityClass } from '$lib/stores/library';
	import { addTrackToQueue, playTrackNow } from '$lib/stores/player';
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

	function describeClause(clause: { type: string; op?: string; clauses?: unknown[]; names?: string[]; match_descendants?: boolean; field?: string; range?: { start?: string | null; end?: string | null }; value?: number; value_max?: number | null; op_label?: string; minimum?: string; playlist_ids?: number[] }): string[] {
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
	};

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
							<span>{isExpanded(playlist.id) ? '−' : '+'}</span>
						</div>
					</button>

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
								</div>
								<div class="track-list">
									{#each playlistTracksById[playlist.id] as track (track.id)}
										<div
											class="track-row"
											role="button"
											tabindex="0"
											onclick={() => void playTrack(track.id)}
											onkeydown={(e) => { if (e.key === 'Enter') void playTrack(track.id); }}
										>
											<div class="track-main">
												<h4>{track.title}</h4>
												<p>{track.artist_name ?? 'Unknown artist'}</p>
											</div>
											<div class="track-side">
												{#if track.best_quality}
													<span class={`quality-badge ${getQualityClass(track.best_quality)}`}>
														{track.best_quality.replace(/_/g, ' ')}
													</span>
												{/if}
												<span>{formatDuration(track.duration_ms)}</span>
												<button class="queue-btn" onclick={(e) => void queueTrack(track.id, e)}>+</button>
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
	<div class="drawer-backdrop" role="presentation" onclick={closeEditor} onkeydown={(e) => e.key === 'Escape' && closeEditor()}></div>
	<aside class="editor-drawer glass-panel">
		<div class="editor-head">
			<h2>{editingPlaylistId === null ? 'New smart playlist' : 'Edit smart playlist'}</h2>
			<button class="close-btn" onclick={closeEditor} aria-label="Close editor">×</button>
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
							<button class="remove-btn" onclick={() => removeClause(clause.id)} aria-label="Remove rule">×</button>
						</div>

						<!-- Genre fields -->
						{#if clause.type === 'genre' || clause.type === 'artist'}
							<div class="tag-field">
								<div class="tag-list">
									{#each clause.names as tag}
										<span class="tag">
											{tag}
											<button class="tag-remove" onclick={() => removeTag(clause.id, tag)}>×</button>
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
	</aside>
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
		font-family: monospace;
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

	.smart-actions {
		display: flex;
		gap: 8px;
		margin-top: 14px;
		padding-top: 14px;
		border-top: 1px solid rgba(255, 255, 255, 0.05);
	}

	.btn-sm {
		padding: 5px 12px;
		font-size: 0.8125rem;
	}

	.danger {
		color: #ff8080;
		border-color: rgba(255, 80, 80, 0.2);
	}

	.danger:hover {
		background: rgba(255, 60, 60, 0.1);
		border-color: rgba(255, 80, 80, 0.35);
	}

	.playlist-body {
		margin-top: 18px;
		padding-top: 18px;
		border-top: 1px solid rgba(255, 255, 255, 0.06);
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
		padding: 10px 0;
		border-bottom: 1px solid rgba(255, 255, 255, 0.05);
		cursor: pointer;
	}

	.track-row:last-child { border-bottom: none; padding-bottom: 0; }

	.track-main { min-width: 0; }

	.track-side {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-shrink: 0;
	}

	.queue-btn {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		background: rgba(124, 128, 255, 0.12);
		color: var(--accent-strong);
	}

	.feedback-bar {
		padding: 10px 14px;
		margin-bottom: var(--gap);
		font-size: 0.875rem;
	}

	.feedback-bar.error { color: #ff8080; border-color: rgba(255, 80, 80, 0.2); }

	/* ─── Drawer ──────────────────────────────────────────────── */

	.drawer-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		z-index: 300;
		backdrop-filter: blur(2px);
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
		border: 1px solid rgba(255, 255, 255, 0.1);
		box-shadow: -8px 0 40px rgba(0, 0, 0, 0.5);
	}

	.editor-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 20px 24px 16px;
		border-bottom: 1px solid rgba(255, 255, 255, 0.07);
		flex-shrink: 0;
	}

	.editor-head h2 { font-size: 1.0625rem; font-weight: 600; }

	.close-btn {
		width: 32px;
		height: 32px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
		font-size: 1.2rem;
		line-height: 1;
		cursor: pointer;
		transition: background 0.12s;
	}

	.close-btn:hover { background: rgba(255, 255, 255, 0.12); }

	.editor-body {
		flex: 1;
		overflow-y: auto;
		padding: 20px 24px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}

	.editor-foot {
		padding: 16px 24px 20px;
		border-top: 1px solid rgba(255, 255, 255, 0.07);
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
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 8px;
		padding: 9px 12px;
		font-size: 0.875rem;
		color: var(--text-primary);
		width: 100%;
		transition: border-color 0.12s;
	}

	.field-input:focus {
		outline: none;
		border-color: rgba(124, 128, 255, 0.5);
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
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
	}

	.logic-btn {
		padding: 5px 14px;
		border-radius: 6px;
		font-size: 0.8125rem;
		font-weight: 600;
		color: var(--text-secondary);
		transition: background 0.12s, color 0.12s;
		cursor: pointer;
	}

	.logic-btn.active {
		background: rgba(124, 128, 255, 0.2);
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
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.1);
		border-radius: 8px;
		padding: 7px 10px;
		font-size: 0.875rem;
		color: var(--text-primary);
	}

	.remove-btn {
		width: 28px;
		height: 28px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-tertiary);
		font-size: 1rem;
		line-height: 1;
		cursor: pointer;
		flex-shrink: 0;
		transition: background 0.12s, color 0.12s;
	}

	.remove-btn:hover { background: rgba(255, 60, 60, 0.12); color: #ff8080; }

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
		background: rgba(124, 128, 255, 0.14);
		border: 1px solid rgba(124, 128, 255, 0.3);
		font-size: 0.8125rem;
		color: var(--accent-strong);
	}

	.tag-remove {
		width: 16px;
		height: 16px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
		font-size: 0.75rem;
		line-height: 1;
		cursor: pointer;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: center;
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

	.editor-error { font-size: 0.875rem; color: #ff8080; }

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
