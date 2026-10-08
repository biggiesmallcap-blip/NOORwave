<script lang="ts">
	import { onDestroy, untrack } from 'svelte';
	import { api, type Track } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import {
		currentTrack,
		isPlaying,
		playTracksInContext,
		setPlayerAutomixEnabled,
		setPlayerShuffleMode,
		startGenreRadio
	} from '$lib/stores/player';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import { pickSeedTrackId, sampleGenreQueue } from './genrePlayback';
	import type { GenreSummary } from './genreSummary';

	type TopArtist = { name: string; artistId: number | null; count: number };

	let {
		node,
		onClose = () => {},
		onSelectGenre = () => {}
	}: {
		node: GenreSummary;
		onClose?: () => void;
		/** Lineage and sub-genre chips move the galaxy to that genre instead of navigating away. */
		onSelectGenre?: (id: number) => void;
	} = $props();

	let tracks = $state<Track[]>([]);
	let topArtists = $state<TopArtist[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let actionError = $state<string | null>(null);
	let loadSeq = 0;

	// Full list with in-page search; "Show more" pages the DOM in chunks.
	const TRACK_PAGE = 100;
	// Clicking a row queues a bounded window from that track; automix extends it.
	const MAX_QUEUE_TRACKS = 300;
	let trackQuery = $state('');
	let visibleCount = $state(TRACK_PAGE);
	let filteredTracks = $derived.by(() => {
		const query = trackQuery.trim().toLowerCase();
		if (!query) return tracks;
		return tracks.filter(
			(track) =>
				track.title.toLowerCase().includes(query) ||
				(track.artist_name ?? '').toLowerCase().includes(query)
		);
	});
	let shownTracks = $derived(filteredTracks.slice(0, visibleCount));
	// Once loaded, the list is the source of truth for the count.
	let trackCount = $derived(loading ? node.trackCount : tracks.length);
	let trail = $derived(node.evolutionHistory.slice(-12));
	let trailMax = $derived(Math.max(1, ...trail.map((point) => point.listenCount)));
	// One or two active periods is noise, not a trend.
	let showTrail = $derived(trail.filter((point) => point.listenCount > 0).length >= 3);
	let maxArtistCount = $derived(topArtists[0]?.count ?? 1);
	let nodeId = $derived(node.id);

	$effect(() => {
		// New search -> restart paging from the top.
		void trackQuery;
		visibleCount = TRACK_PAGE;
	});

	$effect(() => {
		// Reload only when the genre changes, not when the snapshot refreshes.
		void nodeId;
		untrack(() => void loadTracks());
	});

	onDestroy(() => {
		loadSeq += 1;
	});

	function formatMs(ms: number): string {
		const totalSeconds = Math.floor(ms / 1000);
		const hours = Math.floor(totalSeconds / 3600);
		const minutes = Math.floor((totalSeconds % 3600) / 60);
		return hours > 0 ? `${hours}h ${minutes}m` : `${minutes}m`;
	}

	function collectTopArtists(trackList: Track[]): TopArtist[] {
		const byName = new Map<string, TopArtist>();
		for (const track of trackList) {
			const name = track.artist_name?.trim();
			if (!name) continue;
			const entry = byName.get(name) ?? { name, artistId: track.artist_id ?? null, count: 0 };
			entry.count += 1;
			if (entry.artistId == null && track.artist_id != null) entry.artistId = track.artist_id;
			byName.set(name, entry);
		}
		return [...byName.values()].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name)).slice(0, 12);
	}

	function handleArtistContextMenu(event: MouseEvent, artist: TopArtist) {
		if (artist.artistId == null) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({ id: artist.artistId, name: artist.name, in_library: true }, { isLocal: true }),
			artist.name
		);
	}

	async function loadTracks() {
		const targetNode = node;
		const seq = ++loadSeq;
		loading = true;
		error = null;
		actionError = null;
		tracks = [];
		topArtists = [];
		trackQuery = '';
		visibleCount = TRACK_PAGE;
		try {
			const response = await cachedApi.getGenreTracks(targetNode.id, true);
			if (seq !== loadSeq) return;
			tracks = response.tracks;
			topArtists = collectTopArtists(response.tracks);
		} catch (reason) {
			if (seq !== loadSeq) return;
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	async function playFrom(track: Track) {
		actionError = null;
		try {
			// Queue a bounded window starting at the clicked track, from whatever
			// the user is looking at (filtered on search, else the whole genre).
			const startIndex = Math.max(0, filteredTracks.findIndex((t) => t.id === track.id));
			const windowIds = filteredTracks.slice(startIndex, startIndex + MAX_QUEUE_TRACKS).map((t) => t.id);
			const replaced = await api.replacePlaybackQueue(windowIds.map((track_id) => ({ track_id })));
			await setPlayerShuffleMode('genre');
			await setPlayerAutomixEnabled(true);
			const selected = replaced.queue.find((queueItem) => queueItem.track.id === track.id);
			if (selected) await api.playQueueItem(selected.id);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function playMix() {
		actionError = null;
		try {
			const ids = sampleGenreQueue(tracks);
			if (ids.length === 0) {
				actionError = 'This genre has no playable tracks yet.';
				return;
			}
			await playTracksInContext(ids, undefined, { shuffle: true });
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function playRadio() {
		actionError = null;
		try {
			const seed = pickSeedTrackId(tracks);
			if (seed == null) {
				actionError = 'This genre has no playable tracks yet.';
				return;
			}
			await startGenreRadio(seed, 'mixed', node.name);
		} catch (reason) {
			actionError = reason instanceof Error ? reason.message : String(reason);
		}
	}
</script>

<div class="genre-detail" style={`--genre-accent: ${node.color}`}>
	<header class="genre-hero">
		<div class="detail-top">
			<nav class="crumbs" aria-label="Genre lineage">
				{#if node.lineage.length === 0}
					<span class="crumb-family">Genre family</span>
				{/if}
				{#each node.lineage as ancestor, index (ancestor.id)}
					<button type="button" class:crumb-family={index === 0} onclick={() => onSelectGenre(ancestor.id)}>
						{ancestor.name}
					</button>
					<span class="crumb-sep" aria-hidden="true">/</span>
				{/each}
				{#if node.lineage.length > 0}
					<span class="crumb-current">{node.name}</span>
				{/if}
			</nav>
			<button class="close-btn" type="button" onclick={onClose} aria-label="Collapse genre details">
				<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
					<path d="M3.5 3.5l9 9M12.5 3.5l-9 9" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
				</svg>
			</button>
		</div>

		<div class="hero-main">
			<div class="hero-copy">
				<h2 class="genre-title"><span class="genre-dot" aria-hidden="true"></span>{node.name}</h2>
				<p class="hero-stats">
					<span><strong>{trackCount.toLocaleString()}</strong> tracks</span>
					{#if node.listenCount > 0}
						<span><strong>{node.listenCount.toLocaleString()}</strong> listens</span>
						<span><strong>{formatMs(node.totalListenedMs)}</strong> listened</span>
					{/if}
					{#if node.avgBpm != null}<span><strong>{Math.round(node.avgBpm)}</strong> BPM</span>{/if}
					{#if node.avgEnergy != null}<span><strong>{node.avgEnergy.toFixed(2)}</strong> energy</span>{/if}
					{#if node.avgDanceability != null}<span><strong>{node.avgDanceability.toFixed(2)}</strong> dance</span>{/if}
				</p>
				{#if node.cohort}
					<span class="cohort-chip" title={node.cohort.title}>{node.cohort.label}</span>
				{/if}
			</div>
			<div class="hero-actions">
				<button class="btn btn-primary" disabled={loading || tracks.length === 0} onclick={() => void playMix()}>
					Mix this genre
				</button>
				<button
					class="btn btn-glass"
					disabled={loading || tracks.length === 0}
					onclick={() => void playRadio()}
					title="Continuous station with related tracks"
				>
					Radio
				</button>
			</div>
		</div>

		{#if node.children.length > 0}
			<div class="subgenres" aria-label="Sub-genres">
				{#each node.children as child (child.id)}
					<button type="button" class="subgenre-chip" onclick={() => onSelectGenre(child.id)}>
						{child.name}<span>{child.trackCount.toLocaleString()}</span>
					</button>
				{/each}
			</div>
		{/if}

		{#if actionError}
			<p class="action-error" role="status">{actionError}</p>
		{/if}
	</header>

	<div class="genre-body">
		<aside class="genre-side">
			<section class="side-section" aria-label="Top artists">
				<h2>Top artists <small>by tracks</small></h2>
				{#if loading}
					<p class="muted">Loading artists...</p>
				{:else if topArtists.length === 0}
					<p class="muted">No artists yet.</p>
				{:else}
					<ol class="artist-list">
						{#each topArtists as artist (artist.name)}
							<li>
								{#if artist.artistId != null}
									<a
										class="artist-row"
										href={`/artists/${artist.artistId}`}
										oncontextmenu={(event) => handleArtistContextMenu(event, artist)}
									>
										<span class="artist-name">{artist.name}</span>
										<span class="artist-count">{artist.count}</span>
										<span class="artist-bar" style={`--share: ${artist.count / maxArtistCount}`}></span>
									</a>
								{:else}
									<span class="artist-row static">
										<span class="artist-name">{artist.name}</span>
										<span class="artist-count">{artist.count}</span>
										<span class="artist-bar" style={`--share: ${artist.count / maxArtistCount}`}></span>
									</span>
								{/if}
							</li>
						{/each}
					</ol>
				{/if}
			</section>

			{#if showTrail}
				<section class="side-section" aria-label="Listening trail">
					<h2>Listening trail</h2>
					<div class="trail">
						{#each trail as point (point.periodStart)}
							<span
								class="trail-bar"
								style={`--level: ${point.listenCount / trailMax}`}
								title={`${point.periodStart}: ${point.listenCount} plays`}
							></span>
						{/each}
					</div>
					<div class="trail-range">
						<span>{trail[0].periodStart}</span>
						<span>{trail[trail.length - 1].periodStart}</span>
					</div>
				</section>
			{/if}
		</aside>

		<section class="genre-tracks" aria-label="Tracks">
			<div class="tracks-head">
				<h2>Tracks</h2>
				{#if tracks.length > 0}
					<SearchField
						bind:value={trackQuery}
						variant="page"
						size="sm"
						placeholder="Search tracks"
						ariaLabel="Search tracks in this genre"
					/>
				{/if}
			</div>

			{#if loading}
				<p class="muted">Loading tracks...</p>
			{:else if error}
				<p class="action-error">{error}</p>
			{:else}
				<ul class="track-list">
					{#each shownTracks as track (track.id)}
						<TrackRow
							{track}
							variant="art"
							isCurrent={$currentTrack?.id === track.id}
							isPlaying={$currentTrack?.id === track.id && $isPlaying}
							onRowClick={() => void playFrom(track)}
						/>
					{/each}
				</ul>
				{#if filteredTracks.length > shownTracks.length}
					<button class="more-btn" onclick={() => (visibleCount += 2 * TRACK_PAGE)}>
						Show more ({(filteredTracks.length - shownTracks.length).toLocaleString()} left)
					</button>
				{:else if filteredTracks.length === 0 && trackQuery.trim()}
					<p class="muted">No tracks match "{trackQuery.trim()}"</p>
				{/if}
			{/if}
		</section>
	</div>
</div>

<style>
	.genre-detail {
		display: flex;
		flex-direction: column;
		gap: 22px;
		container-type: inline-size;
	}

	.genre-hero {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.detail-top {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
	}

	.crumbs {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.crumb-family {
		color: var(--genre-accent);
		font-weight: var(--font-weight-semibold);
	}

	.crumbs .crumb-family {
		color: var(--genre-accent);
	}

	.crumbs button {
		padding: 0;
		border: 0;
		background: none;
		color: var(--text-secondary);
		font: inherit;
		letter-spacing: inherit;
		text-transform: inherit;
		cursor: pointer;
	}

	.crumbs button:hover {
		color: var(--text-primary);
	}

	.crumb-sep {
		color: var(--text-muted);
	}

	.crumb-current {
		color: var(--text-primary);
	}

	.close-btn {
		display: grid;
		place-items: center;
		width: 34px;
		height: 34px;
		flex-shrink: 0;
		border-radius: 50%;
		border: 1px solid var(--border-subtle);
		background: var(--bg-surface);
		color: var(--text-secondary);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.close-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.hero-main {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 20px;
		flex-wrap: wrap;
	}

	.hero-copy {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 10px;
		min-width: 0;
	}

	.genre-title {
		display: flex;
		align-items: center;
		gap: 12px;
		margin: 0;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		line-height: var(--line-height-tight);
	}

	.genre-dot {
		width: 14px;
		height: 14px;
		flex-shrink: 0;
		border-radius: 50%;
		background: var(--genre-accent);
	}

	.hero-stats {
		display: flex;
		flex-wrap: wrap;
		gap: 4px 16px;
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.hero-stats strong {
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
	}

	.cohort-chip {
		padding: 4px 10px;
		border-radius: 999px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		cursor: help;
	}

	.hero-actions {
		display: flex;
		gap: 8px;
	}

	.subgenres {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.subgenre-chip {
		display: inline-flex;
		align-items: baseline;
		gap: 6px;
		padding: 6px 12px;
		border-radius: 999px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-xs);
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.subgenre-chip:hover {
		background: var(--bg-hover);
		border-color: color-mix(in srgb, var(--genre-accent) 50%, var(--border-subtle));
	}

	.subgenre-chip span {
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
	}

	.action-error {
		margin: 0;
		color: var(--state-error);
		font-size: var(--font-size-sm);
	}

	/* The drawer is narrow: one column, artists as compact pills above the
	   tracks. A wide drawer gets the ranked artist list beside the tracks. */
	.genre-body {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 22px;
		align-items: start;
	}

	@container (min-width: 760px) {
		.genre-body {
			grid-template-columns: minmax(200px, 260px) minmax(0, 1fr);
			gap: 28px;
		}
	}

	.genre-side {
		display: flex;
		flex-direction: column;
		gap: 28px;
	}

	.side-section h2,
	.tracks-head h2 {
		margin: 0 0 12px;
		color: var(--text-primary);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
	}

	.side-section h2 small {
		margin-left: 6px;
		color: var(--text-muted);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.artist-list {
		display: flex;
		flex-direction: column;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.artist-row {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		padding: 7px 10px;
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		text-decoration: none;
		overflow: hidden;
	}

	a.artist-row:hover {
		background: var(--bg-hover);
	}

	.artist-row.static {
		color: var(--text-secondary);
	}

	.artist-name {
		position: relative;
		z-index: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.artist-count {
		position: relative;
		z-index: 1;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.artist-bar {
		position: absolute;
		inset: 0 auto 0 0;
		width: calc(var(--share) * 100%);
		background: color-mix(in srgb, var(--genre-accent) 14%, transparent);
		pointer-events: none;
	}

	.trail {
		display: flex;
		align-items: flex-end;
		gap: 3px;
		height: 40px;
	}

	.trail-bar {
		flex: 1;
		min-height: 2px;
		height: calc(var(--level) * 100%);
		border-radius: 2px;
		background: color-mix(in srgb, var(--genre-accent) 70%, transparent);
	}

	.trail-range {
		display: flex;
		justify-content: space-between;
		margin-top: 6px;
		color: var(--text-muted);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
	}

	.tracks-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 16px;
		margin-bottom: 4px;
	}

	.tracks-head h2 {
		margin: 0;
	}

	.track-list {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.more-btn {
		margin-top: 12px;
		padding: 8px 14px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}

	.more-btn:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.muted {
		margin: 0;
		color: var(--text-muted);
		font-size: var(--font-size-sm);
	}

	/* Narrow drawer: artists as compact pills. Last so it beats the base rules. */
	@container (max-width: 759px) {
		.artist-list {
			flex-direction: row;
			flex-wrap: wrap;
			gap: 6px;
		}

		.artist-row {
			padding: 5px 10px;
			border-radius: 999px;
			background: var(--bg-surface);
			font-size: var(--font-size-xs);
		}

		.artist-bar {
			display: none;
		}
	}
</style>
