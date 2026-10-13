<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api } from '$lib/api/client';
	import type { ArchiveSong } from '$lib/api/video_explore_types';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import ArchiveScrubber from '$lib/video/archive/ArchiveScrubber.svelte';
	import ArchiveTile from '$lib/video/archive/ArchiveTile.svelte';
	import LensBar from '$lib/video/archive/LensBar.svelte';
	import VersionsDrawer from '$lib/video/archive/VersionsDrawer.svelte';
	import { createArchiveFeed, EMPTY_FEED, type FeedState } from '$lib/video/archive/archive_feed';
	import {
		activeLensCount,
		clearLenses,
		lensSummary,
		lensesToSearch,
		parseLenses,
		type ArchiveLenses,
	} from '$lib/video/archive/lenses';
	import { playFromList } from '$lib/video/archive/play';

	// Archive: find one thing. Lenses over every song in the catalog (one
	// tile per song, its best cut on the face), with live counts on each lens.
	// The URL mirrors the lenses, so the Atlas deep links here
	// (/videos/archive?scene=12) and Back restores the view. Requests are
	// debounced and a newer lens state aborts the one in flight.
	//
	// The A-Z and year sorts get a jump rail (ArchiveScrubber): a letter or a
	// decade loads the list from there, and scrolling then carries on both
	// ways, earlier pages coming in above as you scroll up.

	const PAGE_SIZE = 60;
	const BUILD_POLL_MS = 20_000;
	const BUILD_POLL_LIMIT = 15;

	let feedState = $state<FeedState>(EMPTY_FEED);
	const feed = createArchiveFeed({
		fetch: (query, signal) => api.getVideoArchive(query, signal),
		onChange: (next) => (feedState = next),
		pageSize: PAGE_SIZE,
		debounceMs: 220,
	});

	let lenses = $derived(parseLenses(page.url.searchParams));
	let items = $derived(feedState.items);
	let facets = $derived(feedState.facets);
	let active = $derived(activeLensCount(lenses));
	let sceneName = $derived(facets?.scene.find((scene) => scene.id === lenses.scene)?.name ?? null);
	let refreshing = $derived(feedState.phase === 'refreshing');
	let hasMore = $derived(feedState.phase === 'ready' && feedState.base + feedState.items.length < feedState.total);
	let jumps = $derived(feedState.jumps);
	let showRail = $derived(feedState.phase === 'ready' && jumps.length > 1 && items.length > 0);
	// No lenses and no songs: the catalog build has not produced anything yet.
	let buildingNow = $derived(
		feedState.phase === 'unavailable' || (feedState.phase === 'ready' && feedState.total === 0 && active === 0)
	);
	// A building poll goes back through 'loading'; keep the building note up
	// instead of flashing skeletons every time it looks again.
	let buildingSeen = $state(false);
	let building = $derived(buildingNow || (buildingSeen && feedState.phase === 'loading'));
	let firstLoad = $derived(!building && (feedState.phase === 'loading' || feedState.phase === 'idle'));

	let openSong = $state<ArchiveSong | null>(null);
	let sentinel = $state<HTMLDivElement | null>(null);
	let topSentinel = $state<HTMLDivElement | null>(null);
	let gridEl = $state<HTMLDivElement | null>(null);
	/** Index into `jumps` of the section at the top of the screen. */
	let section = $state(-1);
	/** Earlier pages load only once the listener scrolls up after a jump,
	 *  not the moment the jumped-to row lands at the top. */
	let earlierArmed = $state(false);
	let pollTimer: ReturnType<typeof setTimeout> | null = null;
	let polls = 0;

	// The URL is the lens state; every change of it asks the feed.
	$effect(() => {
		feed.request(lenses);
	});

	function setLenses(next: ArchiveLenses) {
		void goto(`/videos/archive${lensesToSearch(next)}`, { replaceState: true, keepFocus: true, noScroll: true });
	}

	function play(song: ArchiveSong) {
		const queue = items.map((item) => item.best);
		void playFromList(song.best, queue, lensSummary(lenses, sceneName));
	}

	$effect(() => {
		if (buildingNow) buildingSeen = true;
		else if (feedState.phase === 'ready' || feedState.phase === 'error') buildingSeen = false;
	});

	// While the catalog is being built, look again now and then.
	$effect(() => {
		if (pollTimer) clearTimeout(pollTimer);
		pollTimer = null;
		if (!buildingNow || polls >= BUILD_POLL_LIMIT) return;
		pollTimer = setTimeout(() => {
			polls += 1;
			feed.retry();
		}, BUILD_POLL_MS);
	});

	function scroller(): HTMLElement | null {
		return gridEl?.closest<HTMLElement>('main.workspace') ?? null;
	}

	/** Jump the list to a section: load from there when it is not loaded,
	 *  then bring that row to the top. */
	async function jump(offset: number) {
		earlierArmed = false;
		const index = await feed.jumpTo(offset);
		if (index === null) return;
		await tick();
		const tile = gridEl?.children[index] as HTMLElement | undefined;
		tile?.scrollIntoView({ block: 'start', behavior: 'auto' });
		updateSection();
	}

	/** The section of the first row on screen, for the rail's mark. */
	function updateSection() {
		const grid = gridEl;
		if (!grid || jumps.length === 0) {
			section = -1;
			return;
		}
		const top = scroller()?.getBoundingClientRect().top ?? 0;
		const tiles = grid.children;
		let lo = 0;
		let hi = tiles.length - 1;
		let first = 0;
		while (lo <= hi) {
			const mid = (lo + hi) >> 1;
			if ((tiles[mid] as HTMLElement).getBoundingClientRect().bottom > top + 8) {
				first = mid;
				hi = mid - 1;
			} else lo = mid + 1;
		}
		const offset = feedState.base + first;
		let at = 0;
		for (let i = 0; i < jumps.length; i += 1) if (jumps[i].offset <= offset) at = i;
		section = at;
	}

	$effect(() => {
		const el = scroller();
		if (!el) return;
		let last = el.scrollTop;
		let frame = 0;
		const onScroll = () => {
			if (el.scrollTop < last - 4) earlierArmed = true;
			last = el.scrollTop;
			if (frame) return;
			frame = requestAnimationFrame(() => {
				frame = 0;
				updateSection();
			});
		};
		el.addEventListener('scroll', onScroll, { passive: true });
		return () => {
			el.removeEventListener('scroll', onScroll);
			if (frame) cancelAnimationFrame(frame);
		};
	});

	$effect(() => {
		void jumps;
		void feedState.base;
		void items.length;
		queueMicrotask(updateSection);
	});

	// Earlier pages come in above without moving what is on screen. The
	// browser's own scroll anchoring does not hold at scroll offset 0 (the
	// very top), where the new rows would push the list down, leave the top
	// sentinel in view and pull in page after page back to the start. So
	// the first row on screen is noted before the prepend and put back to
	// the same height after it.
	let earlierAnchor: { key: string; top: number } | null = null;

	function loadEarlier() {
		const grid = gridEl;
		const el = scroller();
		if (grid && el) {
			const top = el.getBoundingClientRect().top;
			for (const tile of grid.children) {
				const rect = (tile as HTMLElement).getBoundingClientRect();
				const key = (tile as HTMLElement).dataset.key;
				if (rect.bottom > top && key) {
					earlierAnchor = { key, top: rect.top };
					break;
				}
			}
		}
		feed.loadEarlier();
	}

	$effect(() => {
		void feedState.base;
		const anchor = earlierAnchor;
		const el = scroller();
		if (!anchor || !el || feedState.loadingEarlier) return;
		earlierAnchor = null;
		const tile = gridEl?.querySelector<HTMLElement>(`[data-key="${CSS.escape(anchor.key)}"]`);
		if (tile) el.scrollTop += tile.getBoundingClientRect().top - anchor.top;
	});

	// Scrolling up past the first loaded row brings the page before it in.
	$effect(() => {
		if (!topSentinel || !earlierArmed || feedState.base <= 0 || feedState.loadingEarlier) return;
		const observer = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) loadEarlier();
			},
			{ rootMargin: '600px 0px' }
		);
		observer.observe(topSentinel);
		return () => observer.disconnect();
	});

	// Infinite scroll: re-observe after each page so a sentinel that is still
	// in view after a page lands asks for the next one.
	$effect(() => {
		if (!sentinel || feedState.phase !== 'ready' || feedState.loadingMore) return;
		if (feedState.base + feedState.items.length >= feedState.total) return;
		const observer = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) feed.loadMore();
			},
			{ rootMargin: '900px 0px' }
		);
		observer.observe(sentinel);
		return () => observer.disconnect();
	});

	onDestroy(() => {
		feed.dispose();
		if (pollTimer) clearTimeout(pollTimer);
	});
</script>

<svelte:head>
	<title>Archive - NOOR</title>
</svelte:head>

<div class="archive">
	<!-- Lenses over nothing are noise: while the catalog is being built the
	     page is just the honest note below. -->
	{#if !building}
		<LensBar
			{lenses}
			{facets}
			total={firstLoad && !facets ? null : feedState.total}
			onchange={setLenses}
		/>
	{/if}

	{#if lenses.q}
		<p class="query-note">
			Titles matching "{lenses.q}"
			<button type="button" class="link-btn" onclick={() => setLenses({ ...lenses, q: '' })}>Show everything</button>
		</p>
	{/if}

	<div class="refresh-bar" class:active={refreshing || feedState.loadingMore} aria-hidden="true"></div>

	{#if firstLoad}
		<div class="grid" aria-busy="true" aria-label="Loading the archive">
			{#each Array(12) as _, i (i)}
				<div class="tile-skeleton" style={`--rise-index: ${i}`}>
					<div class="sk-art"></div>
					<div class="sk-line"></div>
					<div class="sk-line short"></div>
				</div>
			{/each}
		</div>
	{:else if building}
		<EmptyState
			title="The Archive is still being catalogued"
			copy="NOORwave is sorting every video into songs, scenes and concerts in the background. This page fills in by itself when that pass is done."
		>
			{#snippet actions()}
				<a class="btn btn-secondary" href="/videos/editorial">Browse TIDAL editorial</a>
				<a class="btn btn-ghost" href="/videos/liked">Your liked videos</a>
			{/snippet}
		</EmptyState>
	{:else if feedState.phase === 'error'}
		<ErrorState title="Could not open the Archive" error={feedState.error} onretry={() => feed.retry()} />
	{:else if items.length === 0}
		<EmptyState
			title="Nothing under these lenses"
			copy={lenses.source !== 'catalog'
				? `None of your ${lenses.source} videos fit every lens you picked.`
				: 'No song fits every lens you picked. Put one down to widen the view.'}
		>
			{#snippet actions()}
				<button type="button" class="btn btn-secondary" onclick={() => setLenses(clearLenses(lenses))}>Clear lenses</button>
			{/snippet}
		</EmptyState>
	{:else}
		<div class="results" class:railed={showRail}>
			<div class="list">
				{#if feedState.base > 0}
					<div class="sentinel" bind:this={topSentinel} aria-hidden="true"></div>
					<button type="button" class="earlier" disabled={feedState.loadingEarlier} onclick={loadEarlier}>
						{feedState.loadingEarlier ? 'Loading earlier songs...' : `${feedState.base.toLocaleString()} songs before these`}
					</button>
				{/if}
				<div class="grid" class:dimmed={refreshing} bind:this={gridEl}>
					{#each items as song, index (song.song_key)}
						<div class="rise-in-card" data-key={song.song_key} style={`--rise-index: ${index % 24}`}>
							<ArchiveTile {song} onplay={play} onversions={(picked) => (openSong = picked)} />
						</div>
					{/each}
					{#if feedState.loadingMore}
						{#each Array(4) as _, i (i)}
							<div class="tile-skeleton" aria-hidden="true">
								<div class="sk-art"></div>
								<div class="sk-line"></div>
								<div class="sk-line short"></div>
							</div>
						{/each}
					{/if}
				</div>
				{#if hasMore}
					<div class="sentinel" bind:this={sentinel} aria-hidden="true"></div>
				{:else if feedState.base + items.length > PAGE_SIZE}
					<p class="end t-micro">{feedState.total.toLocaleString()} songs. That is all of them.</p>
				{/if}
			</div>
			{#if showRail}
				<ArchiveScrubber {jumps} current={section} onjump={(offset) => void jump(offset)} />
			{/if}
		</div>
	{/if}

	{#if !building}
		<p class="editorial">
			Looking for TIDAL's own picks? <a href="/videos/editorial">Browse TIDAL editorial videos</a>
		</p>
	{/if}
</div>

{#if openSong}
	{@const picked = openSong}
	<VersionsDrawer
		song={{
			song_key: picked.song_key,
			title: picked.title,
			artist_id: picked.artist_id,
			artist_name: picked.artist_name,
			artwork_url: picked.best.artwork_url,
			cut_count: picked.cut_count,
		}}
		onclose={() => (openSong = null)}
	/>
{/if}

<style>
	/* Width, gutters and the bottom inset come from the video layout. */
	.archive {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	/* The results and, for the A-Z and year sorts, the jump rail beside
	   them. */
	.results {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-4);
		min-width: 0;
	}

	.results.railed {
		grid-template-columns: minmax(0, 1fr) 30px;
	}

	.list {
		display: grid;
		gap: var(--space-4);
		align-content: start;
		min-width: 0;
	}

	/* Rows land under the sticky video header's height, not under it. */
	.grid > :global(*) {
		scroll-margin-top: var(--space-4);
	}

	.earlier {
		justify-self: center;
		height: var(--control-h);
		padding: 0 16px;
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: none;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.earlier:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.earlier:disabled {
		cursor: wait;
		opacity: 0.7;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(min(240px, 100%), 1fr));
		gap: var(--gap-lg) var(--gap);
		transition: opacity var(--motion-fast);
	}

	.grid > :global(*) {
		min-width: 0;
	}

	/* Refresh: keep the old results at 55 percent under a 2px bar. */
	.grid.dimmed {
		opacity: 0.55;
	}

	.refresh-bar {
		position: relative;
		height: 2px;
		margin: calc(var(--space-2) * -1) 0 0;
		overflow: hidden;
	}

	.refresh-bar.active::after {
		content: '';
		position: absolute;
		inset: 0 auto 0 0;
		width: 40%;
		background: var(--accent);
		animation: refresh-sweep 1.1s var(--ease-standard) infinite;
	}

	@keyframes refresh-sweep {
		from { transform: translateX(-100%); }
		to { transform: translateX(250%); }
	}

	/* Skeleton in the tile's exact geometry, shown after 150ms. */
	.tile-skeleton {
		display: grid;
		gap: var(--space-2);
		animation: skeleton-in var(--motion-fast) 150ms both;
	}

	.sk-art,
	.sk-line {
		border-radius: var(--radius-sm);
		background: linear-gradient(90deg, var(--bg-surface) 0%, var(--bg-hover) 50%, var(--bg-surface) 100%);
		background-size: 200% 100%;
		animation: shimmer 1.4s ease-in-out infinite;
	}

	.sk-art {
		aspect-ratio: 16 / 9;
	}

	.sk-line {
		height: 12px;
		width: 78%;
		border-radius: 6px;
	}

	.sk-line.short {
		width: 46%;
	}

	@keyframes skeleton-in {
		from { opacity: 0; }
		to { opacity: 1; }
	}

	@keyframes shimmer {
		0% { background-position: 200% 0; }
		100% { background-position: -200% 0; }
	}

	.sentinel {
		height: 1px;
	}

	.end {
		margin: var(--space-3) 0 0;
		text-align: center;
	}

	.query-note {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.link-btn {
		margin-left: var(--space-2);
		padding: 0;
		border: 0;
		background: none;
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.link-btn:hover {
		text-decoration: underline;
	}

	/* The way to TIDAL's own picks stays reachable, but quiet. */
	.editorial {
		margin: var(--space-5) 0 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-align: center;
	}

	.editorial a {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
	}

	.editorial a:hover {
		color: var(--accent-strong);
	}

	@media (prefers-reduced-motion: reduce) {
		.sk-art,
		.sk-line,
		.refresh-bar.active::after {
			animation: none;
		}
	}
</style>
