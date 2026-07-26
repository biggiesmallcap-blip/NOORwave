<script lang="ts">
	import { onMount } from 'svelte';
	import {
		api,
		type LikedVideo,
		type LikedVideoVersion,
		type TidalSearchVideo,
	} from '$lib/api/client';
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

	/** Which card's version list is open. One at a time. */
	let openVersions = $state<string | null>(null);

	/** The video that represents a song on the wall: the best match, longest cut
	 *  among equals. Server-sorted, so this is just the head. */
	function face(video: LikedVideo): LikedVideoVersion {
		return video.versions[0];
	}

	/** The release year, when TIDAL gave us one. Most of the time a version's
	 *  title already says what it is ("Live at Austin City Limits, 2012"); this
	 *  is for the cases where four videos are all just called "Jamming". */
	function versionMeta(version: LikedVideoVersion): string {
		return version.release_year ? String(version.release_year) : '';
	}

	/** Lift a version into the shape the video queue already speaks, so Play all
	 *  and Shuffle need no new playback code. */
	function toQueueItem(video: LikedVideo, version: LikedVideoVersion): TidalSearchVideo {
		return {
			tidal_id: version.tidal_video_id,
			title: version.video_title,
			duration_ms: version.duration_ms,
			artist_id: video.artist_id,
			artist_name: video.artist_name,
			album_tidal_id: null,
			artwork_url: version.artwork_url,
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

	/** Drop one version. The card survives while it has others left, so hiding a
	 *  bad alternate does not cost you the song. */
	async function hide(video: LikedVideo, version: LikedVideoVersion) {
		const previous = videos;
		// Optimistic: the version is gone the moment you say it is wrong.
		videos = videos
			.map((v) =>
				v.song_key === video.song_key
					? {
							...v,
							versions: v.versions.filter(
								(ver) => ver.tidal_video_id !== version.tidal_video_id
							),
						}
					: v
			)
			.filter((v) => v.versions.length > 0);
		try {
			await api.hideLikedVideo(video.track_ids, version.tidal_video_id);
			showToast(`Hidden "${version.video_title}".`);
		} catch {
			videos = previous;
			showToast('Could not hide that one.');
		}
	}

	function menu(event: MouseEvent, video: LikedVideo, version: LikedVideoVersion) {
		event.preventDefault();
		event.stopPropagation();
		const items = [
			...buildVideoMenu({
				tidal_id: version.tidal_video_id,
				artist_id: video.artist_id,
				artist_name: video.artist_name,
			}),
			{ separator: true, label: '' },
			{
				label: 'Wrong match - hide this',
				icon: '⊘',
				onSelect: () => void hide(video, version),
			},
		];
		openContextMenu(event, items, version.video_title);
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
					v.track_title.toLowerCase().includes(needle) ||
					(v.artist_name ?? '').toLowerCase().includes(needle) ||
					v.versions.some((ver) => ver.video_title.toLowerCase().includes(needle))
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
					a.track_title.localeCompare(b.track_title) ||
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

	/** One video per song, not every version: queueing six cuts of the same song
	 *  back to back is nobody's idea of playing the wall. Pick a specific
	 *  version from the card's version list instead. */
	function wallQueue(): TidalSearchVideo[] {
		return filtered.map((v) => toQueueItem(v, face(v)));
	}

	async function playFrom(index: number) {
		await start(wallQueue(), index);
	}

	async function playAll() {
		await playFrom(0);
	}

	/** Play one specific version, with the rest of the wall queued behind it. */
	async function playVersion(video: LikedVideo, version: LikedVideoVersion) {
		const queue = wallQueue();
		const index = filtered.findIndex((v) => v.song_key === video.song_key);
		const item = toQueueItem(video, version);
		if (index >= 0) queue[index] = item;
		openVersions = null;
		await start(queue, Math.max(index, 0));
	}

	async function shuffle() {
		const queue = wallQueue();
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
			<!-- The way back, in the place /videos puts the way here. Its own line
			     above the title so it reads as navigation, not as a label. -->
			<a class="back-link" href="/videos">
				<span aria-hidden="true">&lsaquo;</span> Videos
			</a>
			<h1>Liked videos</h1>
			<p class="blurb">
				The videos among your likes. Most liked songs do not have one, so this is a slice of
				the library rather than a mirror of it.
			</p>
		</div>
		<div class="header-actions">
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

	<!-- One row: find, narrow, order, play. Genre and year are selects rather
	     than pill rails because they are unbounded - 35 genres and 40 years as
	     chips buried the wall under five rows of chrome. -->
	{#if !loading && videos.length > 0}
		<div class="tools">
			<SearchField
				bind:value={query}
				placeholder="Search your liked videos"
				ariaLabel="Search your liked videos"
				variant="page"
				size="sm"
				fill
				suppressSuggestions
			/>

			{#if genres.length > 0}
				<select class="tool-select" bind:value={activeGenre} aria-label="Filter by genre">
					<option value={null}>All genres</option>
					{#each genres as genre (genre)}
						<option value={genre}>{genre}</option>
					{/each}
				</select>
			{/if}

			{#if years.length > 0}
				<select class="tool-select" bind:value={activeYear} aria-label="Filter by year">
					<option value={null}>All years</option>
					{#each years as year (year)}
						<option value={year}>{year}</option>
					{/each}
				</select>
			{/if}

			<select class="tool-select" bind:value={sort} aria-label="Sort">
				<option value="recent">Recently liked</option>
				<option value="title">A-Z</option>
			</select>

			<div class="tool-actions">
				<button class="tool-btn tool-btn--accent" onclick={() => void playAll()}>Play all</button>
				<button class="tool-btn" onclick={() => void shuffle()}>Shuffle</button>
			</div>
		</div>

		{#if activeGenre || activeYear !== null}
			<p class="filter-note">
				Showing {filtered.length} of {videos.length}
				<button class="link-btn" onclick={() => { activeGenre = null; activeYear = null; }}>
					Clear filters
				</button>
			</p>
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
			{#each filtered as video, index (video.song_key)}
				<div class="card-slot">
					<button
						type="button"
						class="video-card"
						onclick={() => void playFrom(index)}
						oncontextmenu={(event) => menu(event, video, face(video))}
						aria-label={`Play ${video.track_title}`}
					>
						<div class="poster-wrap">
							<ArtworkImage
								className="poster"
								src={face(video).artwork_url}
								size={320}
								fallbackText="VID"
								decorative={true}
							/>
							<PlayOverlay position="corner" size="sm" label={`Play ${video.track_title}`} />
							{#if face(video).duration_ms}
								<span class="duration">{formatTrackDuration(face(video).duration_ms!)}</span>
							{/if}
						</div>
						<div class="meta">
							<span class="title" title={video.track_title}>{video.track_title}</span>
							<span class="subtitle">{video.artist_name ?? 'TIDAL video'}</span>
						</div>
					</button>

					<!-- The escape hatch for songs with more than one video. The card
					     click still plays, so the count is its own control rather
					     than a change of what tapping the artwork does. -->
					{#if video.versions.length > 1}
						<button
							type="button"
							class="versions-chip"
							aria-expanded={openVersions === video.song_key}
							onclick={() =>
								(openVersions = openVersions === video.song_key ? null : video.song_key)}
						>
							{video.versions.length} versions
						</button>
					{/if}

					{#if openVersions === video.song_key}
						<div class="versions-popout">
							<p class="versions-heading">{video.track_title}</p>
							{#each video.versions as version (version.tidal_video_id)}
								<button
									type="button"
									class="version-row"
									onclick={() => void playVersion(video, version)}
									oncontextmenu={(event) => menu(event, video, version)}
								>
									<span class="version-main">
										<span class="version-title">{version.video_title}</span>
										<!-- The year does the work when the titles are all the
										     same word, which is often. -->

										{#if versionMeta(version)}
											<span class="version-meta">{versionMeta(version)}</span>
										{/if}
									</span>
									{#if version.duration_ms}
										<span class="version-duration"
											>{formatTrackDuration(version.duration_ms)}</span
										>
									{/if}
								</button>
							{/each}
						</div>
					{/if}
				</div>
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

	.back-link {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		align-self: flex-start;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		text-decoration: none;
	}

	.back-link:hover {
		color: var(--text-primary);
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
		height: var(--control-h, 30px);
		padding: 0 14px;
		border-radius: 999px;
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
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	/* The field earns a share of the row, not the whole thing: left unbounded it
	   pushed the selects and the actions onto lines of their own. */
	.tools :global(.sf) {
		flex: 1 1 220px;
		max-width: 320px;
	}

	/* Height, radius and padding come from the shared toolbar-control shape
	   (--control-h, app.css) so this row sits level with the pill rows on
	   /search and /library rather than being half a step taller. The fallback
	   keeps it honest until that token lands. */
	.tool-select {
		flex: 0 0 auto;
		height: var(--control-h, 30px);
		padding: 0 10px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		background: rgba(255, 255, 255, 0.05);
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--font-size-sm);
		cursor: pointer;
		max-width: 145px;
	}

	.tool-select:hover {
		color: var(--text-primary);
	}

	.tool-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.tool-btn {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		height: var(--control-h, 30px);
		padding: 0 14px;
		border-radius: 999px;
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

	.tool-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	/* Accent belongs to the primary action alone, so it never competes with a
	   selected filter for meaning. */
	.tool-btn--accent {
		background: var(--accent);
		border-color: var(--accent);
		color: #fff;
	}

	.tool-btn--accent:hover {
		background: var(--accent);
		color: #fff;
	}

	.filter-note {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.link-btn {
		margin-left: var(--space-2);
		padding: 0;
		border: none;
		background: none;
		color: var(--accent);
		font: inherit;
		font-size: var(--font-size-sm);
		cursor: pointer;
	}

	.link-btn:hover {
		text-decoration: underline;
	}

	.video-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
		gap: 14px;
	}

	.card-slot {
		position: relative;
		min-width: 0;
	}

	.versions-chip {
		position: absolute;
		left: 6px;
		top: 6px;
		padding: 2px 8px;
		border-radius: 20px;
		border: 1px solid rgba(255, 255, 255, 0.18);
		background: rgba(0, 0, 0, 0.72);
		color: #fff;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		white-space: nowrap;
	}

	.versions-chip:hover {
		border-color: var(--accent);
	}

	/* Wider than the card it hangs off: a version's title is the only thing that
	   says what it is ("Live At Grand Central"), so clipping it to card width
	   defeats the point of the list. */
	.versions-popout {
		position: absolute;
		left: 0;
		top: 100%;
		z-index: 5;
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 100%;
		width: max-content;
		max-width: min(340px, 80vw);
		margin-top: 4px;
		padding: 6px;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
		background: var(--bg-elevated, #16161c);
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
	}

	/* The last column would otherwise push its popout off the page edge. */
	.card-slot:nth-child(4n) .versions-popout,
	.card-slot:last-child .versions-popout {
		left: auto;
		right: 0;
	}

	.versions-heading {
		margin: 0 0 2px;
		padding: 0 6px;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.version-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 6px 8px;
		border: none;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		text-align: left;
		cursor: pointer;
		min-width: 0;
	}

	.version-row:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.version-main {
		display: flex;
		flex-direction: column;
		gap: 1px;
		min-width: 0;
	}

	/* Wraps rather than ellipsing: two lines of a real title beat one line of
	   "Coming Around Again (Live At Grand Cen...". */
	.version-title {
		min-width: 0;
		overflow-wrap: anywhere;
	}

	.version-meta {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
	}

	.version-duration {
		flex: 0 0 auto;
		color: var(--text-tertiary);
		font-variant-numeric: tabular-nums;
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
