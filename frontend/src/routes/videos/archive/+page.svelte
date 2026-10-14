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
		lensKey,
		lensSummary,
		lensesToSearch,
		parseLenses,
		type ArchiveLenses,
	} from '$lib/video/archive/lenses';
	import { playFromList } from '$lib/video/archive/play';
	import { gridColumns } from '$lib/video/related_feed';
	import { keepView, lastView } from '$lib/video/view_cache';

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

	// Back on the Archive with the same lenses: the last first page shows at
	// once, still (no entry motion), and is checked quietly underneath.
	const kept = lastView<FeedState>('archive');
	const seed = kept && kept.key === lensKey(parseLenses(page.url.searchParams)) ? kept : null;
	let feedState = $state<FeedState>(seed ?? EMPTY_FEED);
	const feed = createArchiveFeed({
		fetch: (query, signal) => api.getVideoArchive(query, signal),
		onChange: (next) => {
			feedState = next;
			if (next.phase === 'ready' && next.base === 0) keepView('archive', { ...next, items: next.items.slice(0, PAGE_SIZE) });
		},
		pageSize: PAGE_SIZE,
		debounceMs: 220,
		seed,
	});
	let still = $derived(seed !== null && feedState.key === seed.key);

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
	// The opener says what the tiles are; the lens row keeps the count.
	let openerLine = $derived(
		lenses.source === 'liked'
			? 'The songs in the videos you liked, one tile each.'
			: lenses.source === 'saved'
				? 'The songs in the videos you saved, one tile each.'
				: 'Every song with a video, one tile each. The picture plays its best cut.'
	);

	let openSong = $state<ArchiveSong | null>(null);
	let sentinel = $state<HTMLDivElement | null>(null);
	let gridEl = $state<HTMLDivElement | null>(null);
	// After a jump, placeholder tiles stand in above the loaded songs for the
	// songs before them (up to three pages, whole rows), so a fast flick up runs
	// into shimmering tiles that turn into songs, never the page's top.
	let columns = $state(4);
	let above = $derived(
		feedState.base > 0 ? Math.ceil(Math.min(feedState.base, PAGE_SIZE * 3) / columns) * columns : 0
	);
	/** Index into `jumps` of the section at the top of the screen. */
	let section = $state(-1);
	/** Offset in the whole result of the first song on screen. */
	let firstOffset = $state(0);
	/** Earlier pages load only once the listener scrolls up after a jump,
	 *  not the moment the jumped-to row lands at the top. */
	let earlierArmed = $state(false);
	// The lens row docks to the top once scrolled past: it slides away
	// while you scroll down and comes back as soon as you scroll up, so the
	// lenses, sort and a way back to the top are always one flick away, even
	// deep in a jumped list whose real top is pages of loading away.
	let dockSentinel = $state<HTMLDivElement | null>(null);
	let dockHeight = $state(0);
	let docked = $state(false);
	let dockHidden = $state(false);
	/** Set while the page moves the scroll itself (holding the place across
	 *  a prepend), so that move is not read as the listener scrolling down. */
	let ownScroll = false;
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

	/** The song tiles in the grid, without placeholders or skeletons. */
	function songTiles(): HTMLElement[] {
		return gridEl ? [...gridEl.querySelectorAll<HTMLElement>(':scope > .rise-in-card')] : [];
	}

	$effect(() => {
		const grid = gridEl;
		if (!grid) return;
		const measure = () => (columns = gridColumns(getComputedStyle(grid).gridTemplateColumns));
		const observer = new ResizeObserver(measure);
		observer.observe(grid);
		measure();
		return () => observer.disconnect();
	});

	/** Jump the list to a section: load from there when it is not loaded,
	 *  then bring that row to the top. */
	async function jump(offset: number) {
		earlierArmed = false;
		const index = await feed.jumpTo(offset);
		if (index === null) return;
		dockHidden = false;
		await tick();
		const tile = songTiles()[index];
		ownScroll = true;
		tile?.scrollIntoView({ block: 'start', behavior: 'auto' });
		updateSection();
	}

	/** The first song on screen: its place in the whole result (for the
	 *  docked count) and its section (for the rail's mark). */
	function updateSection() {
		const grid = gridEl;
		if (!grid) {
			section = -1;
			return;
		}
		const top = scroller()?.getBoundingClientRect().top ?? 0;
		const tiles = songTiles();
		let lo = 0;
		let hi = tiles.length - 1;
		let first = 0;
		while (lo <= hi) {
			const mid = (lo + hi) >> 1;
			if (tiles[mid].getBoundingClientRect().bottom > top + 8) {
				first = mid;
				hi = mid - 1;
			} else lo = mid + 1;
		}
		const offset = feedState.base + first;
		firstOffset = offset;
		let at = jumps.length > 0 ? 0 : -1;
		for (let i = 0; i < jumps.length; i += 1) if (jumps[i].offset <= offset) at = i;
		section = at;
	}

	$effect(() => {
		const el = scroller();
		if (!el) return;
		let last = el.scrollTop;
		let frame = 0;
		const onScroll = () => {
			const delta = el.scrollTop - last;
			last = el.scrollTop;
			if (ownScroll) {
				ownScroll = false;
			} else {
				if (delta < -4) earlierArmed = true;
				if (delta > 6) dockHidden = true;
				else if (delta < -6) dockHidden = false;
			}
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

	// Docked once the row's own place has scrolled out of view.
	$effect(() => {
		const sentinel = dockSentinel;
		const el = scroller() ?? sentinel?.closest<HTMLElement>('main.workspace') ?? null;
		if (!sentinel || !el) return;
		const observer = new IntersectionObserver(
			(entries) => {
				const entry = entries[entries.length - 1];
				docked = !entry.isIntersecting && entry.boundingClientRect.top < (entry.rootBounds?.top ?? 0);
				if (!docked) dockHidden = false;
			},
			{ root: el, threshold: 0 }
		);
		observer.observe(sentinel);
		return () => observer.disconnect();
	});

	/** Back to the first song: from the loaded window when it starts there,
	 *  else a jump to 0, then the page top. */
	async function toTop() {
		const el = scroller();
		if (feedState.base > 0) {
			earlierArmed = false;
			await feed.jumpTo(0);
			await tick();
		}
		el?.scrollTo({ top: 0, behavior: feedState.base > 0 ? 'auto' : 'smooth' });
		dockHidden = false;
	}

	// Earlier pages come in above without moving what is on screen. The
	// browser's own scroll anchoring does not hold at scroll offset 0 and
	// does not know placeholders turn into songs, so the first song tile is
	// measured right before the prepend lands (not when it was asked for:
	// the listener may have scrolled on meanwhile) and put back at the same
	// height after it. Placeholders above it then turn into songs in place.
	let earlierAnchor: { key: string; top: number } | null = null;
	let prepending = false;

	function loadEarlier() {
		if (feedState.loadingEarlier || feedState.base <= 0) return;
		prepending = true;
		feed.loadEarlier();
	}

	$effect.pre(() => {
		void feedState.items;
		if (!prepending || feedState.loadingEarlier) return;
		prepending = false;
		const el = scroller();
		if (!el) return;
		const top = el.getBoundingClientRect().top;
		for (const tile of songTiles()) {
			const rect = tile.getBoundingClientRect();
			const key = tile.dataset.key;
			if (rect.bottom > top && key) {
				earlierAnchor = { key, top: rect.top };
				return;
			}
		}
		// Everything on screen is placeholders: hold the first song tile.
		const first = songTiles()[0];
		if (first?.dataset.key) earlierAnchor = { key: first.dataset.key, top: first.getBoundingClientRect().top };
	});

	$effect(() => {
		void feedState.base;
		void feedState.items;
		const anchor = earlierAnchor;
		const el = scroller();
		if (!anchor || !el || feedState.loadingEarlier) return;
		earlierAnchor = null;
		const tile = gridEl?.querySelector<HTMLElement>(`[data-key="${CSS.escape(anchor.key)}"]`);
		const shift = tile ? tile.getBoundingClientRect().top - anchor.top : 0;
		if (Math.abs(shift) >= 1) {
			ownScroll = true;
			el.scrollTop += shift;
		}
	});

	// Scrolling up toward the first loaded song brings the page before it
	// in, well ahead (1500 px), so the placeholders are rarely seen for
	// long. Not right after a jump, only once the listener scrolls up.
	$effect(() => {
		const grid = gridEl;
		void above;
		void items;
		if (!grid || !earlierArmed || feedState.base <= 0 || feedState.loadingEarlier) return;
		const observer = new IntersectionObserver(
			(entries) => {
				if (entries.some((entry) => entry.isIntersecting)) loadEarlier();
			},
			{ root: scroller(), rootMargin: '1500px 0px 0px 0px' }
		);
		void tick().then(() => {
			const first = grid.querySelector(':scope > .rise-in-card');
			if (first) observer.observe(first);
			grid.querySelectorAll(':scope > .placeholder-above').forEach((el) => observer.observe(el));
		});
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

<div
	class="archive"
	style:--dock-h={`${dockHeight}px`}
	style:--dock-offset={docked && !dockHidden ? `${dockHeight}px` : '0px'}
>
	<header class="opener">
		<h2>Archive</h2>
		<p>{openerLine}</p>
	</header>

	<!-- Lenses over nothing are noise: while the catalog is being built the
	     page is just the honest note below. -->
	{#if !building}
		<div class="dock-sentinel" bind:this={dockSentinel} aria-hidden="true"></div>
		<div class="lens-dock" class:docked class:hidden={docked && dockHidden} bind:clientHeight={dockHeight}>
			{#if docked}
				<button type="button" class="to-top" onclick={() => void toTop()} aria-label="Back to the top" title="Back to the first song">
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
						<path d="M8 13V3M3.5 7.5 8 3l4.5 4.5" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
					</svg>
					Top
				</button>
			{/if}
			<div class="dock-lenses">
				<LensBar
					{lenses}
					{facets}
					total={firstLoad && !facets ? null : feedState.total}
					at={docked ? firstOffset : null}
					onchange={setLenses}
				/>
			</div>
		</div>
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
				<div class="grid" class:dimmed={refreshing} class:still bind:this={gridEl}>
					{#each Array(above) as _, i (i)}
						<div class="tile-skeleton placeholder-above" aria-hidden="true">
							<div class="sk-art"></div>
							<div class="sk-line"></div>
							<div class="sk-line short"></div>
						</div>
					{/each}
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

	/* The page's opener, as the Logbook's and History's: the word in the
	   titles' italic serif, one quiet line under it. */
	.opener {
		display: grid;
		gap: 6px;
	}

	.opener h2 {
		margin: 0;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: 1.1;
		letter-spacing: -0.01em;
	}

	.opener p {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	/* Where the lens row sits when not docked; it takes no room. */
	.dock-sentinel {
		height: 0;
		margin-bottom: calc(var(--space-4) * -1);
	}

	/* The lens row, docked to the top of the scroll once scrolled past: a
	   floating bar on the page's own ground that slides away on the way
	   down and back on the way up. */
	.lens-dock {
		position: sticky;
		top: 0;
		z-index: var(--z-raised);
		display: flex;
		align-items: center;
		gap: var(--space-3);
		margin: 0 -12px;
		padding: 6px 12px;
		border-radius: var(--radius-md);
		transition:
			transform var(--motion-base),
			opacity var(--motion-base),
			background var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	/* Solid: the tiles' titles must not read through it. */
	.lens-dock.docked {
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
	}

	.lens-dock.hidden {
		transform: translateY(calc(-100% - 12px));
		opacity: 0;
		pointer-events: none;
	}

	.dock-lenses {
		flex: 1;
		min-width: 0;
	}

	/* The way back up, plainly labelled and in the accent so it is found
	   at a glance in the docked row. */
	.to-top {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		flex: none;
		height: var(--control-h);
		padding: 0 14px 0 11px;
		border: 1px solid var(--accent-line);
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.to-top:hover {
		background: color-mix(in srgb, var(--accent-soft) 60%, var(--accent) 22%);
	}

	.to-top:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	@media (prefers-reduced-motion: reduce) {
		.lens-dock {
			transition: none;
		}
	}

	/* The results and, for the A-Z and year sorts, the jump rail beside
	   them. */
	/* overflow-anchor: the page holds the place across a prepend itself
	   (see earlierAnchor); the browser's own anchoring would move the
	   scroll too, and its move reads as the listener scrolling down. */
	.results {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-4);
		min-width: 0;
		overflow-anchor: none;
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
		scroll-margin-top: calc(var(--dock-h, 0px) + var(--space-4));
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

	/* A kept page shown again does not replay its entrance; pages loaded
	   after it still rise in. */
	.grid.still > .rise-in-card:nth-child(-n + 60) {
		animation: none;
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

	/* Placeholders above a jumped list show at once: they are scrolled
	   into, not waited for. */
	.tile-skeleton.placeholder-above {
		animation: none;
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
