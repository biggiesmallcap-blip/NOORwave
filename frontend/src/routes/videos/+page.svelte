<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, type TidalSearchVideo, type VideoDiscoverSet } from '$lib/api/client';
	import GuideFeature from '$lib/components/video/GuideFeature.svelte';
	import GuideRow from '$lib/components/video/GuideRow.svelte';
	import { buildBrowseMix } from '$lib/video/browse_mix';
	import { playFromShelf, playVideoCollection } from '$lib/video/play_collection';
	import { videoSectionQuery, watchUrl } from '$lib/video/section';

	// The Videos tab, laid out like the station guide: today's picks as the
	// featured row, then one row per built shelf with a filmstrip of its
	// videos. A frame plays the shelf from that video. Browsing only - picking
	// anything opens the watch page (see routes/videos/+layout.svelte).
	// TIDAL's editorial modules live on their own tab.

	const HINTS = ['music video', 'live session', 'official video', 'visualizer'];
	// While the server assembles today's set (building: true, no snapshot yet),
	// re-fetch a few times so the picks appear without a manual reload.
	const BUILD_POLL_MS = 6000;
	const BUILD_POLL_MAX = 20;

	let discoverSets = $state<VideoDiscoverSet[]>([]);
	let loadingBrowse = $state(true);
	let browsePollTimer: ReturnType<typeof setTimeout> | null = null;
	let browsePolls = 0;

	// The daily set leads as the featured row; every other built set is its
	// own row below it.
	let dailySet = $derived(discoverSets.find((s) => s.slug === 'daily-picks') ?? null);
	let shelfSets = $derived(
		discoverSets.filter((s) => s.slug !== 'daily-picks' && s.items.length > 0)
	);
	let browseMix = $derived(buildBrowseMix(discoverSets));
	let canStartRadio = $derived(browseMix.length >= 4);
	let hasBrowseContent = $derived(Boolean(dailySet) || shelfSets.length > 0);

	async function loadBrowse() {
		try {
			const discover = await api.getVideosDiscover();
			discoverSets = discover.sets ?? [];
			// Sets build one at a time server-side, so keep polling while more
			// are on the way - the page fills in row by row.
			if (discover.building && browsePolls < BUILD_POLL_MAX) {
				browsePolls += 1;
				browsePollTimer = setTimeout(() => void loadBrowse(), BUILD_POLL_MS);
			}
		} catch {
			// Nothing to browse; the landing hints below take over.
		} finally {
			loadingBrowse = false;
		}
	}

	/** Frames for a shelf: its videos that have a picture. */
	function frames(set: VideoDiscoverSet): TidalSearchVideo[] {
		return set.items.filter((video) => Boolean(video.artwork_url));
	}

	function playFromSet(set: VideoDiscoverSet, index: number) {
		const video = set.items[index];
		if (video) void playFromShelf(video, set.items, set.title, { autoplay: true });
	}

	function playSetFrom(set: VideoDiscoverSet, startWith?: TidalSearchVideo) {
		const index = startWith ? set.items.findIndex((item) => item.tidal_id === startWith.tidal_id) : 0;
		playFromSet(set, Math.max(0, index));
	}

	function playBrowseMix() {
		const first = browseMix[0];
		if (first) void playFromShelf(first, browseMix, 'Video radio', { autoplay: true, continuous: true, radioScope: 'library' });
	}

	/** Old deep links still land here: /videos?videoId= moves to the watch
	 *  page, ?mixId= / ?playlistId= play the collection. The address is cleaned
	 *  first so Back from the watch page returns to plain browsing instead of
	 *  replaying the link. */
	async function handleDeepLink() {
		const params = new URLSearchParams(window.location.search);
		const videoId = params.get('videoId');
		const mixId = params.get('mixId');
		const playlistId = params.get('playlistId');
		if (videoId) {
			await goto(watchUrl(videoId, {
				radio: params.get('radio') === '1',
				title: params.get('title'),
				artistId: Number(params.get('artistId')) || null,
				artistName: params.get('artistName'),
			}), { replaceState: true });
			return;
		}
		if (!mixId && !playlistId) return;
		await goto('/videos', { replaceState: true, keepFocus: true });
		if (mixId) void playVideoCollection('mix', mixId);
		else if (playlistId) void playVideoCollection('playlist', playlistId);
	}

	onMount(() => {
		void loadBrowse();
		void handleDeepLink();
	});

	onDestroy(() => {
		if (browsePollTimer) clearTimeout(browsePollTimer);
	});
</script>

<div class="videos-page">
	{#if dailySet}
		<GuideFeature label={`Daily picks: ${dailySet.title}`} frames={frames(dailySet)} onpick={(video) => dailySet && playSetFrom(dailySet, video)}>
			<span class="eyebrow">Daily picks</span>
			<button type="button" class="feature-title" onclick={() => dailySet && playFromSet(dailySet, 0)}>{dailySet.title}</button>
			{#if dailySet.blurb}<p class="feature-blurb">{dailySet.blurb}</p>{/if}
			<div class="feature-actions">
				<button type="button" class="btn btn-primary" onclick={() => dailySet && playFromSet(dailySet, 0)}>Play all</button>
				{#if canStartRadio}
					<button type="button" class="btn btn-glass" title="Starts with your picks, then keeps finding related artists and genres." onclick={playBrowseMix}>Start video radio</button>
				{/if}
			</div>
		</GuideFeature>
	{:else if loadingBrowse}
		<p class="picks-loading">Assembling today's picks...</p>
	{:else if canStartRadio}
		<div class="radio-line">
			<p>Video radio starts with your picks, then keeps finding related artists and genres.</p>
			<button type="button" class="btn btn-glass" onclick={playBrowseMix}>Start video radio</button>
		</div>
	{/if}

	{#if shelfSets.length > 0}
		<section class="group" aria-label="From your library">
			<h3 class="group-label">From your library</h3>
			{#each shelfSets as set (set.slug)}
				<GuideRow
					title={set.title}
					titleHint={set.blurb}
					meta={set.blurb}
					count={`${set.items.length} ${set.items.length === 1 ? 'video' : 'videos'}`}
					label={set.title}
					frames={frames(set)}
					onplay={(startWith) => playSetFrom(set, startWith)}
				/>
			{/each}
		</section>
	{/if}

	<!-- Only when there is nothing to browse (no TIDAL session / empty
	     library): a few starting searches so the tab is never blank. -->
	{#if !hasBrowseContent && !loadingBrowse}
		<section class="landing-row">
			<span class="eyebrow">Try</span>
			<div class="chips">
				{#each HINTS as item (item)}
					<button type="button" class="hint-chip" onclick={() => videoSectionQuery.set(item)}>{item}</button>
				{/each}
			</div>
		</section>
	{/if}
</div>

<style>
	.videos-page {
		display: grid;
		/* minmax(0, 1fr) not the default auto: an auto track sizes to its
		   widest child's min-content, so a filmstrip would blow the column
		   (and the page) past the container instead of clipping. */
		grid-template-columns: minmax(0, 1fr);
		gap: 18px;
	}

	/* The section label from /search: small, uppercase, accent. */
	.eyebrow,
	.group-label {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 1.5px;
		color: var(--accent);
	}

	.feature-title {
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-align: left;
		cursor: pointer;
		overflow-wrap: anywhere;
	}
	.feature-title:hover { text-decoration: underline; text-underline-offset: 4px; }
	.feature-blurb {
		max-width: 62ch;
		margin: 0;
		color: var(--text-secondary);
	}
	.feature-actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	.radio-line {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		padding: 0 12px;
	}
	.radio-line p {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.group {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: 2px;
		min-width: 0;
	}
	.group-label {
		margin: 6px 0 4px;
		padding: 0 12px;
	}

	.picks-loading {
		margin: 0;
		padding: 4px 12px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.landing-row {
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
		display: grid;
		gap: var(--space-3);
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.hint-chip {
		display: inline-flex;
		align-items: center;
		height: var(--control-h);
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		border-radius: 999px;
		padding: 0 14px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
	}

	.hint-chip:hover {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border-color: var(--accent-line);
	}

	@media (max-width: 860px) {
		.feature-title { font-size: var(--font-size-xl); }
		.radio-line { flex-direction: column; align-items: flex-start; }
	}
</style>
