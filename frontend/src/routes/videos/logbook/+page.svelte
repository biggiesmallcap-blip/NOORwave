<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalSearchVideo, type VideoHistoryEntry } from '$lib/api/client';
	import type { VideoCensus, VideoSteer } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { formatDateShort } from '$lib/utils/format';
	import { hueColor } from '$lib/video/archive/lenses';
	import {
		censusFigures,
		coverageFigures,
		cutShares,
		sceneCoverage,
		seenLabel,
		steerAge,
	} from '$lib/video/archive/logbook';
	import { keepView, lastView } from '$lib/video/view_cache';

	// Logbook: yours. The three ways back to what you have touched (Liked,
	// Saved, History), the More and Less like this presses On Air still
	// remembers (each with an undo), then the catalog as a calm page of
	// numbers and how much of each scene you have seen. Plain counts only: no
	// badges, no streaks. Sections open like On Air's, in the titles' serif.

	const SCENES_SHOWN = 24;
	/** A first visit waits this long for its answers before showing what it
	 *  has (skeletons where the rest will go). */
	const REVEAL_MS = 150;

	type LogbookView = {
		census: VideoCensus | null;
		saved: TidalSearchVideo[] | null;
		steers: VideoSteer[];
		lastWatched: VideoHistoryEntry | null;
	};
	const seen = lastView<LogbookView>('logbook');

	let census = $state<VideoCensus | null>(seen?.census ?? null);
	let censusState = $state<'loading' | 'ready' | 'unavailable'>(seen ? (seen.census ? 'ready' : 'unavailable') : 'loading');
	let saved = $state<TidalSearchVideo[] | null>(seen?.saved ?? null);
	let lastWatched = $state<VideoHistoryEntry | null>(seen?.lastWatched ?? null);
	let historyLoaded = $state(Boolean(seen));
	let allScenes = $state(false);
	let steers = $state<VideoSteer[]>(seen?.steers ?? []);
	// A first visit holds the page for its first answers (a few ms with the
	// census cached on the server), then rises in section by section. A
	// revisit shows the last answers at once, still, and refreshes them.
	let shown = $state(Boolean(seen));
	const rise = !seen;

	let figures = $derived(census ? censusFigures(census) : []);
	let coverage = $derived(census ? coverageFigures(census) : []);
	let cuts = $derived(census ? cutShares(census) : []);
	// Per-scene rows ride on the census, so the Logbook never loads the map.
	let sceneRows = $derived(sceneCoverage(census?.scene_rows ?? []));
	let shownScenes = $derived(allScenes ? sceneRows : sceneRows.slice(0, SCENES_SHOWN));
	let visited = $derived(sceneRows.filter((row) => row.seen > 0).length);
	// Bars compare scenes with each other (the most seen one fills its bar):
	// against a whole scene every bar would be a sliver.
	let bestRatio = $derived(Math.max(0, ...sceneRows.map((row) => row.ratio)));

	async function loadCensus() {
		try {
			census = await api.getVideoCensus();
			censusState = 'ready';
		} catch {
			// A refresh that fails keeps the census already on screen.
			if (!census) censusState = 'unavailable';
		}
	}

	onMount(() => {
		const reveal = setTimeout(() => (shown = true), REVEAL_MS);
		void Promise.allSettled([
			loadCensus(),
			api.getSavedVideos().then((res) => (saved = res.items)).catch(() => {}),
			api.getVideoSteers().then((res) => (steers = res.items)).catch(() => {}),
			api.getVideoHistory(1)
				.then((res) => (lastWatched = res.items[0] ?? null))
				.catch(() => {})
				.finally(() => (historyLoaded = true)),
		]).then(() => {
			clearTimeout(reveal);
			shown = true;
		});
		return () => clearTimeout(reveal);
	});

	// Whatever is on screen is what the next visit opens with.
	$effect(() => {
		if (!historyLoaded || censusState === 'loading') return;
		keepView<LogbookView>('logbook', { census, saved, steers, lastWatched });
	});

	/** Undo a remembered press; it comes back if the server says no. */
	async function undoSteer(steer: VideoSteer) {
		const before = steers;
		steers = steers.filter((item) => item.artist_id !== steer.artist_id);
		try {
			await api.forgetVideoSteer(steer.artist_id);
		} catch {
			steers = before;
		}
	}

	function fadeDate(atS: number): string {
		return new Date(atS * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
	}

	function artistMenu(event: MouseEvent, steer: VideoSteer) {
		event.preventDefault();
		event.stopPropagation();
		const name = steer.name ?? 'Artist';
		openContextMenu(event, buildArtistMenu({ tidal_id: steer.artist_id, name }, { isLocal: false }), name);
	}

	function videoMenu(event: MouseEvent, video: TidalSearchVideo) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<svelte:head>
	<title>Logbook - NOOR</title>
</svelte:head>

<div class="logbook-frame">
{#if shown}
<div class="logbook">
	<section class="entries" class:rise-in-shelf={rise} aria-label="Your videos" style="--rise-index: 0">
		<a class="entry" href="/videos/liked">
			<span class="entry-art liked" aria-hidden="true">
				<svg viewBox="0 0 24 24" width="22" height="22"><path d="M12 20.5s-7.5-4.6-7.5-10.1A4.4 4.4 0 0 1 12 7.6a4.4 4.4 0 0 1 7.5 2.8c0 5.5-7.5 10.1-7.5 10.1z" fill="currentColor" /></svg>
			</span>
			<span class="entry-copy">
				<span class="entry-title">Liked videos</span>
				<span class="t-meta">Videos matched to the songs you like</span>
			</span>
			<span class="entry-count"></span>
			<span class="chevron" aria-hidden="true"></span>
		</a>

		<a class="entry" href="/videos/liked" oncontextmenu={(event) => saved?.[0] && videoMenu(event, saved[0])}>
			<span class="entry-art">
				{#if saved && saved.length > 0}
					<ArtworkImage className="entry-img" src={saved.slice(0, 6).map((video) => video.artwork_url ?? '')} size={160} fallbackText="" tint decorative={true} />
				{:else}
					<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><path d="M6.5 3.5h11v17l-5.5-4-5.5 4z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" /></svg>
				{/if}
			</span>
			<span class="entry-copy">
				<span class="entry-title">Saved</span>
				<span class="t-meta">
					{#if saved === null}Videos you saved from anywhere{:else if saved.length === 0}Nothing saved yet. Save a video from the stage.{:else}Last saved: {saved[0].title}{/if}
				</span>
			</span>
			<span class="entry-count">{saved ? saved.length.toLocaleString() : ''}</span>
			<span class="chevron" aria-hidden="true"></span>
		</a>

		<a class="entry" href="/videos/history" oncontextmenu={(event) => lastWatched && videoMenu(event, lastWatched.video)}>
			<span class="entry-art">
				{#if lastWatched}
					<ArtworkImage className="entry-img" src={lastWatched.video.artwork_url} size={160} fallbackText="" tint decorative={true} />
				{:else}
					<svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true"><path d="M12 7v5l3 2M3.5 12a8.5 8.5 0 1 0 2.5-6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /><path d="M3 4v4h4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
				{/if}
			</span>
			<span class="entry-copy">
				<span class="entry-title">History</span>
				<span class="t-meta">
					{#if lastWatched}
						{formatDateShort(lastWatched.watched_at)}: {lastWatched.video.title}{lastWatched.video.artist_name ? `, ${lastWatched.video.artist_name}` : ''}
					{:else if historyLoaded}
						Nothing watched yet
					{:else}
						Everything you have watched
					{/if}
				</span>
			</span>
			<span class="entry-count">{census && census.watched_videos > 0 ? census.watched_videos.toLocaleString() : ''}</span>
			<span class="chevron" aria-hidden="true"></span>
		</a>
	</section>

	{#if steers.length > 0}
		<section class="steers" class:rise-in-shelf={rise} aria-labelledby="steers-title" style="--rise-index: 1">
			<header class="opener">
				<h2 id="steers-title">Steering</h2>
				<p>More and Less like this lean On Air a little, then fade within four weeks.</p>
			</header>
			<ul class="steer-rows">
				{#each steers as steer (steer.artist_id)}
					<li class="steer-row">
						<span class="steer-tag" class:less={!steer.more}>{steer.more ? 'More' : 'Less'}</span>
						<a
							class="steer-name"
							href={`/tidal/artists/${steer.artist_id}`}
							oncontextmenu={(event) => artistMenu(event, steer)}
						>{steer.name ?? `Artist ${steer.artist_id}`}</a>
						<span class="meter" title={`${Math.round(steer.strength * 100)}% of its first strength`}>
							<span class="fill" class:less={!steer.more} style:width={`${Math.max(steer.strength * 100, 4)}%`}></span>
						</span>
						<span class="when">{steerAge(steer.at)}, fades by {fadeDate(steer.forgotten_at)}</span>
						<button
							type="button"
							class="undo"
							aria-label={`Undo ${steer.more ? 'more' : 'less'} like ${steer.name ?? 'this artist'}`}
							onclick={() => void undoSteer(steer)}
						>Undo</button>
					</li>
				{/each}
			</ul>
		</section>
	{/if}

	<section class="census" class:rise-in-shelf={rise} aria-labelledby="census-title" style="--rise-index: 2">
		<header class="opener">
			<h2 id="census-title">The catalog</h2>
			<p>Everything video discovery has found, sorted into songs, scenes and concerts.</p>
		</header>
		{#if censusState === 'loading'}
			<div class="figures" aria-busy="true">
				{#each Array(6) as _, i (i)}
					<div class="figure ghost"><span class="ghost-num"></span><span class="ghost-label"></span></div>
				{/each}
			</div>
		{:else if censusState === 'unavailable' || !census}
			<p class="t-body quiet">The census appears here once the catalog has been sorted into songs, scenes and concerts. That pass runs in the background.</p>
		{:else}
			{#if census.building}
				<p class="t-body quiet">Still sorting the catalog. These numbers fill in as each pass finishes.</p>
			{/if}
			<dl class="figures">
				{#each figures as figure (figure.label)}
					<div class="figure">
						<dt>{figure.label}</dt>
						<dd>
							<span class="num">{figure.value}</span>
							{#if figure.note}<span class="note">{figure.note}</span>{/if}
						</dd>
					</div>
				{/each}
			</dl>

			<div class="two-up">
				<div class="block">
					<h3 class="block-label">What it knows</h3>
					<dl class="small-figures">
						{#each coverage as figure (figure.label)}
							<div class="small-figure">
								<dt>{figure.label}</dt>
								<dd><span class="small-num">{figure.value}</span>{#if figure.note}<span class="note">{figure.note}</span>{/if}</dd>
							</div>
						{/each}
						<div class="small-figure">
							<dt>You have watched</dt>
							<dd>
								<span class="small-num">{census.watched_videos.toLocaleString()}</span>
								<span class="note">videos, from {census.lit_artists.toLocaleString()} artists</span>
							</dd>
						</div>
					</dl>
				</div>

				{#if cuts.length > 0}
					<div class="block">
						<h3 class="block-label">Cuts</h3>
						<ul class="cut-bars">
							{#each cuts as cut (cut.cut)}
								<li>
									<a class="cut-name" href={`/videos/archive?cut=${cut.cut}`}>{cut.label}</a>
									<span class="bar"><span class="fill" style:width={`${Math.max(cut.share * 100, 1.5)}%`}></span></span>
									<span class="cut-count">{cut.count.toLocaleString()}</span>
								</li>
							{/each}
						</ul>
					</div>
				{/if}
			</div>
		{/if}
	</section>

	{#if sceneRows.length > 0}
		<section class="scenes" class:rise-in-shelf={rise} aria-labelledby="scenes-title" style="--rise-index: 3">
			<header class="opener">
				<h2 id="scenes-title">Scenes</h2>
				<p>{visited} of {sceneRows.length} visited. Each bar compares a scene with the one you have seen most.</p>
			</header>
			<ul class="scene-grid">
				{#each shownScenes as row (row.id)}
					<li>
						<a class="scene" class:unseen={row.seen === 0} href={`/videos/archive?scene=${row.id}`} title={`${seenLabel(row)}. Open ${row.name} in the Archive`}>
							<span class="scene-line">
								<span class="hue" style:background={hueColor(row.hue)}></span>
								<span class="scene-name">{row.name}</span>
								<span class="scene-count"><strong>{row.seen.toLocaleString()}</strong> of {row.videos.toLocaleString()}</span>
							</span>
							<span class="scene-bar" style:background={hueColor(row.hue, 0.14)}>
								<span class="fill" style:width={`${row.seen > 0 && bestRatio > 0 ? Math.max((row.ratio / bestRatio) * 100, 3) : 0}%`} style:background={hueColor(row.hue)}></span>
							</span>
						</a>
					</li>
				{/each}
			</ul>
			{#if sceneRows.length > SCENES_SHOWN}
				<button type="button" class="more" onclick={() => (allScenes = !allScenes)}>
					{allScenes ? 'Show fewer' : `Show all ${sceneRows.length} scenes`}
				</button>
			{/if}
		</section>
	{/if}
</div>
{/if}
</div>

<style>
	/* Width, gutters and the bottom inset come from the video layout. A calm
	   page: rows on the ground, numbers in columns, no tiles or borders. */
	.logbook-frame {
		container-type: inline-size;
		min-width: 0;
	}

	.logbook {
		container-type: inline-size;
		display: grid;
		gap: var(--space-7);
		min-width: 0;
		max-width: 1100px;
	}

	.entries {
		display: grid;
		gap: var(--row-gap);
	}

	.entry {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr) auto 20px;
		align-items: center;
		gap: var(--space-4);
		padding: 8px;
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		text-decoration: none;
		transition: background var(--motion-fast);
	}

	.entry:hover {
		background: var(--bg-hover);
	}

	.entry:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.entry-art {
		position: relative;
		display: grid;
		place-items: center;
		width: 96px;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-surface);
		color: var(--text-tertiary);
	}

	.entry-art.liked {
		color: var(--accent-strong);
		background: var(--accent-soft);
	}

	.entry-art :global(.entry-img) {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
		font-size: var(--font-size-2xs);
	}

	.entry-art :global(.entry-img.fallback) {
		display: grid;
		place-items: center;
	}

	.entry-copy {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.entry-title {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
	}

	.entry-count {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}

	.chevron {
		width: 8px;
		height: 8px;
		border-right: 1.5px solid var(--text-tertiary);
		border-top: 1.5px solid var(--text-tertiary);
		transform: rotate(45deg);
	}

	.census {
		display: grid;
		gap: var(--space-5);
	}

	/* Section openers, as On Air's: the word in the titles' italic serif,
	   one quiet line under it. */
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
		line-height: var(--line-height-tight);
		letter-spacing: -0.01em;
	}
	.opener p {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.quiet {
		margin: 0;
		color: var(--text-secondary);
	}

	.figures {
		display: grid;
		grid-template-columns: repeat(6, minmax(0, 1fr));
		gap: var(--space-5) var(--space-6);
		margin: 0;
		padding-bottom: var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
	}

	.figure {
		display: grid;
		gap: 4px;
		align-content: start;
	}

	.figure dt {
		color: var(--text-tertiary);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}

	.small-figure dt {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.figure dd,
	.small-figure dd {
		display: grid;
		gap: 2px;
		margin: 0;
	}

	/* Big numbers in the serif, lining figures: a printed annual, not a
	   dashboard. */
	.num {
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-tight);
		font-variant-numeric: lining-nums tabular-nums;
	}

	.note {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.ghost-num,
	.ghost-label {
		display: block;
		border-radius: 6px;
		background: var(--bg-surface);
		animation: ghost-pulse 1.4s ease-in-out infinite;
	}

	.ghost-num {
		height: 30px;
		width: 70%;
	}

	.ghost-label {
		height: 10px;
		width: 40%;
	}

	@keyframes ghost-pulse {
		50% { opacity: 0.5; }
	}

	.two-up {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(320px, 100%), 1fr));
		gap: var(--space-6);
	}

	.block {
		display: grid;
		gap: var(--group-gap);
		align-content: start;
	}

	.block-label {
		margin: 0;
		color: var(--accent-strong);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}

	.small-figures {
		display: grid;
		gap: var(--space-3);
		margin: 0;
	}

	.small-figure {
		display: grid;
		grid-template-columns: 150px minmax(0, 1fr);
		align-items: baseline;
		gap: var(--space-3);
	}

	.small-figure dd {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		flex-wrap: wrap;
	}

	.small-num {
		color: var(--text-primary);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-semibold);
		font-variant-numeric: tabular-nums;
	}

	.cut-bars {
		display: grid;
		gap: 6px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.cut-bars li {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr) 72px;
		align-items: center;
		gap: var(--space-3);
	}

	.cut-name {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		text-decoration: none;
	}

	.cut-name:hover {
		color: var(--accent-strong);
	}

	.bar {
		position: relative;
		display: block;
		height: 3px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--text-tertiary) 14%, transparent);
		overflow: hidden;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		border-radius: inherit;
		background: var(--text-tertiary);
		opacity: 0.8;
	}

	.cut-count {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		text-align: right;
		white-space: nowrap;
	}

	.scenes,
	.steers {
		display: grid;
		gap: var(--space-4);
	}

	/* One remembered press per row: the lean, the artist, a short meter of
	   how much of it is left, when it goes, and a quiet undo. */
	.steer-rows {
		display: grid;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.steer-row {
		display: grid;
		grid-template-columns: 52px minmax(0, 260px) 72px minmax(0, 1fr) 64px;
		align-items: center;
		gap: var(--space-4);
		padding: 7px 8px;
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}

	.steer-row:hover,
	.steer-row:focus-within {
		background: var(--bg-hover);
	}

	.steer-tag {
		justify-self: start;
		padding: 2px 8px;
		border-radius: 999px;
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.steer-tag.less {
		background: var(--bg-surface);
		color: var(--text-secondary);
	}

	.steer-name {
		min-width: 0;
		overflow: hidden;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.steer-name:hover {
		text-decoration: underline;
		text-underline-offset: 3px;
	}

	.meter {
		position: relative;
		display: block;
		height: 3px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--text-tertiary) 16%, transparent);
		overflow: hidden;
	}

	.meter .fill {
		background: var(--accent);
		opacity: 1;
	}

	.meter .fill.less {
		background: var(--text-tertiary);
	}

	.when {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		white-space: nowrap;
	}

	/* The undo waits for the row: no wall of identical buttons. */
	.undo {
		justify-self: end;
		height: 26px;
		padding: 0 12px;
		border: 0;
		border-radius: 999px;
		background: none;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		opacity: 0;
		transition:
			opacity var(--motion-fast),
			background var(--motion-fast),
			color var(--motion-fast);
	}

	.steer-row:hover .undo,
	.steer-row:focus-within .undo {
		opacity: 1;
	}

	.undo:hover {
		background: var(--bg-surface);
		color: var(--text-primary);
	}

	.steer-name:focus-visible,
	.undo:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	/* Scenes in three quiet columns: a dot of the scene's colour, the name,
	   how many you have seen, and a hairline bar beneath. */
	.scene-grid {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 4px clamp(24px, 3vw, 48px);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.scene {
		display: grid;
		gap: 6px;
		padding: 8px 8px 9px;
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		text-decoration: none;
		transition: background var(--motion-fast);
	}

	.scene:hover {
		background: var(--bg-hover);
	}

	.scene:focus-visible,
	.more:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.scene-line {
		display: grid;
		grid-template-columns: 8px minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
	}

	.hue {
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}

	.scene-name {
		overflow: hidden;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.scene-count {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}

	.scene-count strong {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
	}

	.scene-bar {
		position: relative;
		display: block;
		height: 2px;
		margin-left: 18px;
		border-radius: 999px;
		overflow: hidden;
	}

	.scene-bar .fill {
		opacity: 0.9;
	}

	.scene.unseen .scene-name {
		color: var(--text-secondary);
	}

	.more {
		justify-self: start;
		height: var(--control-h);
		padding: 0 12px;
		border: 0;
		border-radius: 999px;
		background: none;
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.more:hover {
		background: var(--bg-hover);
	}

	/* Wide screens (1440p and 4K): two columns instead of one narrow one.
	   Your shelves and steering on the left, the catalog on the right, the
	   scenes across both underneath. */
	@container (min-width: 1560px) {
		.logbook {
			max-width: none;
			grid-template-columns: minmax(0, 5fr) minmax(0, 7fr);
			grid-template-areas:
				'entries census'
				'steers census'
				'scenes scenes';
			grid-template-rows: auto 1fr auto;
			column-gap: clamp(56px, 4vw, 120px);
			align-items: start;
		}

		.entries {
			grid-area: entries;
		}

		.steers {
			grid-area: steers;
		}

		.census {
			grid-area: census;
		}

		.scenes {
			grid-area: scenes;
		}

		.scene-grid {
			grid-template-columns: repeat(4, minmax(0, 1fr));
		}

		.steer-row {
			grid-template-columns: 52px minmax(0, 1fr) 72px auto 64px;
		}
	}

	@container (max-width: 900px) {
		.scene-grid {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
		.figures {
			grid-template-columns: repeat(3, minmax(0, 1fr));
		}
	}

	@media (max-width: 640px) {
		.entry {
			grid-template-columns: 72px minmax(0, 1fr) auto 12px;
			gap: var(--space-3);
		}

		.entry-art {
			width: 72px;
		}

		.scene-grid {
			grid-template-columns: minmax(0, 1fr);
		}

		.steer-row {
			grid-template-columns: 52px minmax(0, 1fr) auto;
		}

		.steer-row .meter,
		.steer-row .when {
			display: none;
		}

		.undo {
			opacity: 1;
		}

		.small-figure {
			grid-template-columns: minmax(0, 1fr);
			gap: 2px;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.ghost-num,
		.ghost-label {
			animation: none;
		}
	}
</style>
