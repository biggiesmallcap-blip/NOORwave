<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import {
		api,
		type TidalHomeModule,
		type VideoDiscoverSet,
	} from '$lib/api/client';
	import TidalDiscoverShelves from '$lib/components/search/TidalDiscoverShelves.svelte';
	import VideoSetShelf from '$lib/components/video/VideoSetShelf.svelte';
	import { buildBrowseMix } from '$lib/video/browse_mix';
	import { playEditorialItem, playFromShelf, playVideoCollection } from '$lib/video/play_collection';
	import { videoSectionQuery, watchUrl } from '$lib/video/section';

	// The Videos tab: today's picks, the built shelves and a little of TIDAL's
	// editorial video page. Browsing only - picking anything opens the watch
	// page (see routes/videos/+layout.svelte).

	const HINTS = ['music video', 'live session', 'official video', 'visualizer'];
	// While the server assembles today's set (building: true, no snapshot yet),
	// re-fetch a few times so the picks appear without a manual reload.
	const BUILD_POLL_MS = 6000;
	const BUILD_POLL_MAX = 20;
	// TIDAL's videos page ships several modules; a couple is plenty next to the
	// library-derived shelves.
	const EDITORIAL_MODULE_MAX = 3;

	let discoverSets = $state<VideoDiscoverSet[]>([]);
	let editorialModules = $state<TidalHomeModule[]>([]);
	let loadingBrowse = $state(true);
	let browsePollTimer: ReturnType<typeof setTimeout> | null = null;
	let browsePolls = 0;

	// The daily set leads as the first rail; every other built set is its own
	// rail below it.
	let dailySet = $derived(discoverSets.find((s) => s.slug === 'daily-picks') ?? null);
	let shelfSets = $derived(
		discoverSets.filter((s) => s.slug !== 'daily-picks' && s.items.length > 0)
	);
	let browseMix = $derived(buildBrowseMix(discoverSets));
	let hasBrowseContent = $derived(
		Boolean(dailySet) || shelfSets.length > 0 || editorialModules.length > 0
	);

	async function loadBrowse() {
		try {
			const [discover, page] = await Promise.allSettled([
				api.getVideosDiscover(),
				api.getTidalPage('videos'),
			]);
			if (discover.status === 'fulfilled') {
				discoverSets = discover.value.sets ?? [];
				// Sets build one at a time server-side, so keep polling while
				// more are on the way - the page fills in shelf by shelf.
				if (discover.value.building && browsePolls < BUILD_POLL_MAX) {
					browsePolls += 1;
					browsePollTimer = setTimeout(() => void loadBrowse(), BUILD_POLL_MS);
				}
			}
			if (page.status === 'fulfilled' && editorialModules.length === 0) {
				editorialModules = (page.value.modules ?? [])
					.filter((m) => m.items.length >= 4)
					.slice(0, EDITORIAL_MODULE_MAX);
			}
		} finally {
			loadingBrowse = false;
		}
	}

	function playFromSet(set: VideoDiscoverSet, index: number) {
		const video = set.items[index];
		if (video) void playFromShelf(video, set.items, set.title, { autoplay: true });
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
	{#if browseMix.length >= 4}
		<div class="browse-mix">
			<div>
				<p class="eyebrow">Keep watching</p>
				<h2>Video radio</h2>
				<p>Starts with your picks, then keeps finding related artists and genres.</p>
			</div>
			<button type="button" class="mix-play" onclick={playBrowseMix}>Start radio</button>
		</div>
	{/if}
	{#if dailySet}
		<VideoSetShelf
			eyebrow="Daily picks"
			title={dailySet.title}
			blurb={dailySet.blurb}
			items={dailySet.items}
			onSelect={(_video, index) => dailySet && playFromSet(dailySet, index)}
			onPlayAll={() => dailySet && playFromSet(dailySet, 0)}
		/>
	{:else if loadingBrowse}
		<p class="picks-loading">Assembling today's picks...</p>
	{/if}
	{#each shelfSets as set, i (set.slug)}
		<VideoSetShelf
			index={dailySet ? i + 1 : i}
			title={set.title}
			blurb={set.blurb}
			items={set.items}
			onSelect={(_video, index) => playFromSet(set, index)}
			onPlayAll={() => playFromSet(set, 0)}
		/>
	{/each}

	{#if editorialModules.length > 0}
		<section class="results-section">
			<div class="section-heading section-heading--split">
				<div class="section-heading">
					<p class="eyebrow">From TIDAL's desk</p>
					<h2>Editorial picks</h2>
				</div>
				<a class="text-btn" href="/videos/editorial">More from TIDAL</a>
			</div>
			<TidalDiscoverShelves
				modules={editorialModules}
				mediaKind="video"
				onItemSelect={(item) => playEditorialItem(item, editorialModules)}
			/>
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
		   widest child's min-content, so a shelf rail of 12 cards would blow the
		   column (and the page) past the container instead of scrolling. */
		grid-template-columns: minmax(0, 1fr);
		gap: 28px;
	}

	.browse-mix {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: var(--space-4);
		padding: 0 2px;
	}

	.browse-mix h2,
	.browse-mix p {
		margin: 0;
	}

	.browse-mix h2 {
		font-size: var(--font-size-lg);
	}

	.browse-mix p:not(.eyebrow) {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.mix-play {
		flex: 0 0 auto;
		padding: var(--space-2) var(--space-4);
		border: 1px solid var(--accent-line);
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		cursor: pointer;
	}

	.mix-play:hover,
	.mix-play:focus-visible {
		background: var(--bg-hover);
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.picks-loading {
		margin: 0;
		padding: 4px 2px;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.results-section {
		display: grid;
		gap: 14px;
	}

	.section-heading {
		display: flex;
		align-items: baseline;
		justify-content: flex-start;
		gap: 12px;
	}

	.section-heading--split {
		justify-content: space-between;
		width: 100%;
	}

	/* Matches VideoSetShelf's heading so a section and a shelf read as the
	   same kind of thing rather than two competing scales. */
	.section-heading h2 {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-lg);
	}

	.text-btn {
		color: var(--accent-strong);
		font-weight: var(--font-weight-bold);
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

	@media (max-width: 620px) {
		.videos-page {
			gap: 20px;
		}
	}
</style>
