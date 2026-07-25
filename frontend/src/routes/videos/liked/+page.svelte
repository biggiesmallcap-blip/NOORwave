<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type LikedVideo, type TidalSearchVideo } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { showToast } from '$lib/stores/toast';
	import { formatTrackDuration } from '$lib/utils/format';
	import { playVideo } from '$lib/stores/video_session';

	// While the background resolve is still working, re-fetch so the wall fills
	// in without a manual reload. Same cadence as the editorial set build.
	const SCAN_POLL_MS = 6000;

	let videos = $state<LikedVideo[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let running = $state(false);
	let scannedArtists = $state(0);
	let totalArtists = $state(0);
	let tidalConnected = $state(true);
	let refreshing = $state(false);

	let query = $state('');
	let activeGenre = $state<string | null>(null);
	let activeYear = $state<number | null>(null);
	let sort = $state<'recent' | 'title'>('recent');

	let pollTimer: ReturnType<typeof setTimeout> | null = null;

	/** A card keys on the pair, not the track: one liked song can carry several
	 *  videos (live takes, covers, alternates) and each is its own card. */
	function cardKey(video: LikedVideo): string {
		return `${video.track_id}:${video.tidal_video_id}`;
	}

	/** Lift a row into the shape the video queue already speaks, so Play all and
	 *  Shuffle need no new playback code. */
	function toQueueItem(video: LikedVideo): TidalSearchVideo {
		return {
			tidal_id: video.tidal_video_id,
			title: video.video_title,
			duration_ms: video.duration_ms,
			artist_id: video.artist_id,
			artist_name: video.artist_name,
			album_tidal_id: null,
			artwork_url: video.artwork_url,
			quality: null,
			explicit: null,
			type: 'video',
		};
	}

	async function load() {
		try {
			const res = await api.getLikedVideos();
			videos = res.videos;
			running = res.running;
			scannedArtists = res.scanned_artists;
			totalArtists = res.total_artists;
			tidalConnected = res.tidal_connected;
			error = null;
			schedulePoll();
		} catch (err) {
			error = err instanceof Error ? err.message : 'Could not load your liked videos.';
		} finally {
			loading = false;
		}
	}

	/** Poll only while there is something to wait for. A finished library
	 *  settles into no timer at all. */
	function schedulePoll() {
		if (pollTimer) {
			clearTimeout(pollTimer);
			pollTimer = null;
		}
		const pending = running || (totalArtists > 0 && scannedArtists < totalArtists);
		if (!pending) return;
		pollTimer = setTimeout(() => void load(), SCAN_POLL_MS);
	}

	async function refresh() {
		if (refreshing) return;
		refreshing = true;
		try {
			const res = await api.refreshLikedVideos();
			running = res.running;
			showToast(
				res.running ? 'Looking for new videos...' : 'Everything is already up to date.'
			);
			await load();
		} catch {
			showToast('Could not start the refresh.');
		} finally {
			refreshing = false;
		}
	}

	async function hide(video: LikedVideo) {
		const key = cardKey(video);
		const previous = videos;
		// Optimistic: the card is gone the moment you say it is wrong.
		videos = videos.filter((v) => cardKey(v) !== key);
		try {
			await api.hideLikedVideo(video.track_id, video.tidal_video_id);
			showToast(`Hidden "${video.video_title}".`);
		} catch {
			videos = previous;
			showToast('Could not hide that one.');
		}
	}

	function menu(event: MouseEvent, video: LikedVideo) {
		event.preventDefault();
		event.stopPropagation();
		const items = [
			...buildVideoMenu({
				tidal_id: video.tidal_video_id,
				artist_id: video.artist_id,
				artist_name: video.artist_name,
			}),
			{ separator: true, label: '' },
			{
				label: 'Wrong match - hide this',
				icon: '⊘',
				onSelect: () => void hide(video),
			},
		];
		openContextMenu(event, items, video.video_title);
	}

	// --- Filtering and sorting ---

	let genres = $derived(
		[...new Set(videos.map((v) => v.genre).filter((g): g is string => Boolean(g)))].sort()
	);

	let years = $derived(
		[...new Set(videos.map((v) => v.album_year).filter((y): y is number => y != null))].sort(
			(a, b) => b - a
		)
	);

	let filtered = $derived.by(() => {
		const needle = query.trim().toLowerCase();
		let rows = videos;
		if (needle) {
			rows = rows.filter(
				(v) =>
					v.video_title.toLowerCase().includes(needle) ||
					v.track_title.toLowerCase().includes(needle) ||
					(v.artist_name ?? '').toLowerCase().includes(needle)
			);
		}
		if (activeGenre) rows = rows.filter((v) => v.genre === activeGenre);
		// A card with no album year drops out while a year is ticked: 62% of
		// likes carry one, and an "Unknown" bucket would be a bigger pill than
		// most real years.
		if (activeYear != null) rows = rows.filter((v) => v.album_year === activeYear);

		const sorted = [...rows];
		if (sort === 'title') {
			sorted.sort(
				(a, b) =>
					a.video_title.localeCompare(b.video_title) ||
					(a.artist_name ?? '').localeCompare(b.artist_name ?? '')
			);
		} else {
			sorted.sort((a, b) => (b.liked_at ?? '').localeCompare(a.liked_at ?? ''));
		}
		return sorted;
	});

	let scanPending = $derived(totalArtists > 0 && scannedArtists < totalArtists);

	/** The label follows the ticked genre, so "Play genre" is just Play all with
	 *  a pill on - one action, and the dock says which slice is playing. */
	let sourceLabel = $derived(activeGenre ? `Liked ${activeGenre} videos` : 'Liked videos');

	/** Everything on screen, in view order: what Play all, Shuffle and a single
	 *  card click all queue from. */
	async function start(queue: TidalSearchVideo[], index: number) {
		if (queue.length === 0) return;
		// 'search' is what the videos page uses for any browsable list with a
		// queue behind it, as opposed to a mix or a single direct pick.
		await playVideo(queue[index], { queue, source: 'search', sourceLabel });
	}

	async function playFrom(index: number) {
		await start(filtered.map(toQueueItem), index);
	}

	async function playAll() {
		await playFrom(0);
	}

	async function shuffle() {
		const queue = filtered.map(toQueueItem);
		for (let i = queue.length - 1; i > 0; i--) {
			const j = Math.floor(Math.random() * (i + 1));
			[queue[i], queue[j]] = [queue[j], queue[i]];
		}
		await start(queue, 0);
	}

	onMount(() => {
		void load();
		return () => {
			if (pollTimer) clearTimeout(pollTimer);
		};
	});
</script>

<svelte:head>
	<title>Liked videos - NOOR</title>
</svelte:head>

<div class="page">
	<header class="page-header">
		<div class="heading">
			<p class="eyebrow">Your library</p>
			<h1>Liked videos</h1>
			<p class="blurb">
				The videos among your likes. Most liked songs do not have one, so this is a slice of
				the library rather than a mirror of it.
			</p>
		</div>
		<div class="header-actions">
			<a class="header-action" href="/videos">TIDAL editorial</a>
			<button
				type="button"
				class="header-action"
				onclick={() => void refresh()}
				disabled={refreshing || !tidalConnected}
			>
				{refreshing ? 'Refreshing...' : 'Refresh'}
			</button>
		</div>
	</header>

	{#if !loading && !error && scanPending}
		<p class="scan-note">
			Looking for videos across your liked artists - {scannedArtists} of {totalArtists} done.
			Cards appear as they are found.
		</p>
	{/if}

	{#if !loading && videos.length > 0}
		<div class="tools">
			<SearchField
				bind:value={query}
				placeholder="Search your liked videos"
				ariaLabel="Search your liked videos"
				variant="page"
				fill
				suppressSuggestions
			/>
			<div class="tool-actions">
				<button class="filter-pill filter-pill--accent" onclick={() => void playAll()}>
					Play all
				</button>
				<button class="filter-pill filter-pill--ghost" onclick={() => void shuffle()}>
					Shuffle
				</button>
				<button
					class="filter-pill"
					class:active={sort === 'recent'}
					onclick={() => (sort = 'recent')}
				>
					Recently liked
				</button>
				<button
					class="filter-pill"
					class:active={sort === 'title'}
					onclick={() => (sort = 'title')}
				>
					A-Z
				</button>
			</div>
		</div>

		{#if genres.length > 0}
			<div class="filter-pills">
				<button
					class="filter-pill"
					class:active={activeGenre === null}
					onclick={() => (activeGenre = null)}>All genres</button
				>
				{#each genres as genre (genre)}
					<button
						class="filter-pill"
						class:active={activeGenre === genre}
						onclick={() => (activeGenre = activeGenre === genre ? null : genre)}
						>{genre}</button
					>
				{/each}
			</div>
		{/if}

		{#if years.length > 0}
			<div class="filter-pills">
				<button
					class="filter-pill"
					class:active={activeYear === null}
					onclick={() => (activeYear = null)}>All years</button
				>
				{#each years as year (year)}
					<button
						class="filter-pill"
						class:active={activeYear === year}
						onclick={() => (activeYear = activeYear === year ? null : year)}
						>{year}</button
					>
				{/each}
			</div>
		{/if}
	{/if}

	{#if loading}
		<div class="video-grid">
			{#each Array(12) as _, i (i)}
				<div class="card-skeleton"><Skeleton rows={2} label="Loading liked videos" /></div>
			{/each}
		</div>
	{:else if error}
		<EmptyState title="Could not load your liked videos" copy={error} />
	{:else if !tidalConnected}
		<EmptyState
			title="Connect TIDAL to find videos"
			copy="This wall is built from TIDAL videos matched against the songs you have liked."
		/>
	{:else if videos.length === 0}
		<EmptyState
			title={scanPending ? 'Still looking' : 'No videos among your likes yet'}
			copy={scanPending
				? 'The first pass works through your liked artists in the background. Cards appear here as they are found.'
				: 'Nothing matched yet. Like a few more songs, or press Refresh to check again.'}
		/>
	{:else if filtered.length === 0}
		<EmptyState title="Nothing matches those filters" copy="Try clearing the search or pills." />
	{:else}
		<div class="video-grid">
			{#each filtered as video, index (cardKey(video))}
				<button
					type="button"
					class="video-card"
					onclick={() => void playFrom(index)}
					oncontextmenu={(event) => menu(event, video)}
					aria-label={`Play ${video.video_title}`}
				>
					<div class="poster-wrap">
						<ArtworkImage
							className="poster"
							src={video.artwork_url}
							size={320}
							fallbackText="VID"
							decorative={true}
						/>
						<PlayOverlay position="corner" size="sm" label={`Play ${video.video_title}`} />
						{#if video.duration_ms}
							<span class="duration">{formatTrackDuration(video.duration_ms)}</span>
						{/if}
					</div>
					<div class="meta">
						<span class="title" title={video.video_title}>{video.video_title}</span>
						<span class="subtitle">{video.artist_name ?? 'TIDAL video'}</span>
					</div>
				</button>
			{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-5) var(--space-5) var(--space-8);
	}

	.page-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}

	.heading {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.eyebrow {
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-tertiary);
		margin: 0;
	}

	.heading h1 {
		margin: 0;
		font-size: var(--font-size-2xl);
	}

	.blurb {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		max-width: 56ch;
	}

	.header-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.header-action {
		display: inline-flex;
		align-items: center;
		padding: 6px 14px;
		border-radius: 20px;
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		text-decoration: none;
		white-space: nowrap;
		cursor: pointer;
		transition:
			background 0.15s,
			color 0.15s,
			border-color 0.15s;
	}

	.header-action:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.header-action:disabled {
		opacity: 0.5;
		cursor: default;
	}

	.scan-note {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.tools {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}

	.tool-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.filter-pills {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.filter-pill {
		padding: 5px 14px;
		border-radius: 20px;
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		white-space: nowrap;
		transition:
			background 0.15s,
			color 0.15s,
			border-color 0.15s;
	}

	.filter-pill:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.filter-pill.active {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.filter-pill--accent {
		border-color: var(--accent);
		color: var(--accent);
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.card-skeleton {
		aspect-ratio: 16 / 9;
		display: flex;
		align-items: center;
	}

	.video-card {
		display: flex;
		flex-direction: column;
		gap: var(--space-2);
		padding: 0;
		border: none;
		background: transparent;
		text-align: left;
		cursor: pointer;
		min-width: 0;
	}

	.poster-wrap {
		position: relative;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-elevated, rgba(255, 255, 255, 0.04));
	}

	.poster-wrap :global(.poster) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.duration {
		position: absolute;
		right: 6px;
		bottom: 6px;
		padding: 2px 6px;
		border-radius: 4px;
		background: rgba(0, 0, 0, 0.7);
		color: #fff;
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.subtitle {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	@media (max-width: 640px) {
		.video-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
