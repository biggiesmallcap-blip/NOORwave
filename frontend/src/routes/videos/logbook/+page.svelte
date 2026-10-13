<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type TidalSearchVideo, type VideoHistoryEntry } from '$lib/api/client';
	import type { AtlasScene, VideoCensus } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { formatDateShort } from '$lib/utils/format';
	import { hueColor } from '$lib/video/archive/lenses';
	import {
		censusFigures,
		coverageFigures,
		cutShares,
		sceneCoverage,
		seenLabel,
	} from '$lib/video/archive/logbook';

	// Logbook: yours. The three ways back to what you have touched (Liked,
	// Saved, History), then the catalog as a calm page of numbers and how much
	// of each scene you have seen. Plain counts only: no badges, no streaks.

	const SCENES_SHOWN = 16;

	let census = $state<VideoCensus | null>(null);
	let censusState = $state<'loading' | 'ready' | 'unavailable'>('loading');
	let scenes = $state<AtlasScene[]>([]);
	let saved = $state<TidalSearchVideo[] | null>(null);
	let lastWatched = $state<VideoHistoryEntry | null>(null);
	let historyLoaded = $state(false);
	let allScenes = $state(false);

	let figures = $derived(census ? censusFigures(census) : []);
	let coverage = $derived(census ? coverageFigures(census) : []);
	let cuts = $derived(census ? cutShares(census) : []);
	let sceneRows = $derived(sceneCoverage(scenes));
	let shownScenes = $derived(allScenes ? sceneRows : sceneRows.slice(0, SCENES_SHOWN));
	let visited = $derived(sceneRows.filter((row) => row.seen > 0).length);

	async function loadCensus() {
		try {
			census = await api.getVideoCensus();
			censusState = 'ready';
		} catch {
			censusState = 'unavailable';
		}
	}

	onMount(() => {
		void loadCensus();
		api.getVideoAtlas().then((atlas) => (scenes = atlas.scenes)).catch(() => {});
		api.getSavedVideos().then((res) => (saved = res.items)).catch(() => {});
		api.getVideoHistory(1)
			.then((res) => (lastWatched = res.items[0] ?? null))
			.catch(() => {})
			.finally(() => (historyLoaded = true));
	});

	function videoMenu(event: MouseEvent, video: TidalSearchVideo) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<svelte:head>
	<title>Logbook - NOOR</title>
</svelte:head>

<div class="logbook">
	<section class="entries" aria-label="Your videos">
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
					<ArtworkImage className="entry-img" src={saved[0].artwork_url} size={160} fallbackText="VID" decorative={true} />
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
					<ArtworkImage className="entry-img" src={lastWatched.video.artwork_url} size={160} fallbackText="VID" decorative={true} />
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

	<section class="census" aria-labelledby="census-title">
		<h2 id="census-title" class="t-section">The catalog</h2>
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
		<section class="scenes" aria-labelledby="scenes-title">
			<div class="scenes-head">
				<h2 id="scenes-title" class="t-section">Scenes</h2>
				<span class="t-meta">{visited} of {sceneRows.length} visited</span>
			</div>
			<ul class="scene-rows">
				{#each shownScenes as row (row.id)}
					<li>
						<a class="scene-row" href={`/videos/archive?scene=${row.id}`} title={`Open ${row.name} in the Archive`}>
							<span class="scene-name"><span class="hue" style:background={hueColor(row.hue)}></span>{row.name}</span>
							<span class="bar" style:background={hueColor(row.hue, 0.16)}><span class="fill" style:width={`${row.seen > 0 ? Math.max(row.ratio * 100, 1.5) : 0}%`} style:background={hueColor(row.hue)}></span></span>
							<span class="seen">{seenLabel(row)}</span>
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

<style>
	/* Width, gutters and the bottom inset come from the video layout. A calm
	   page: rows on the ground, numbers in columns, no tiles or borders. */
	.logbook {
		display: grid;
		gap: var(--section-gap);
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

	.census h2,
	.scenes h2 {
		margin: 0;
	}

	.quiet {
		margin: 0;
		color: var(--text-secondary);
	}

	.figures {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
		gap: var(--space-5);
		margin: 0;
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

	.num {
		color: var(--text-primary);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		font-variant-numeric: tabular-nums;
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

	.cut-bars,
	.scene-rows {
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
		height: 6px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--text-tertiary) 18%, transparent);
		overflow: hidden;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		border-radius: inherit;
		background: var(--text-tertiary);
		opacity: 0.8;
	}

	.cut-count,
	.seen {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		text-align: right;
		white-space: nowrap;
	}

	.scenes {
		display: grid;
		gap: var(--space-4);
	}

	.scenes-head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
	}

	.scene-row {
		display: grid;
		grid-template-columns: minmax(140px, 260px) minmax(0, 1fr) 140px;
		align-items: center;
		gap: var(--space-4);
		padding: 6px 8px;
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		text-decoration: none;
		transition: background var(--motion-fast);
	}

	.scene-row:hover {
		background: var(--bg-hover);
	}

	.scene-row:focus-visible,
	.more:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.scene-name {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.hue {
		flex: 0 0 auto;
		width: 8px;
		height: 8px;
		border-radius: 50%;
	}

	.scene-row .fill {
		opacity: 0.85;
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

	@media (max-width: 640px) {
		.entry {
			grid-template-columns: 72px minmax(0, 1fr) auto 12px;
			gap: var(--space-3);
		}

		.entry-art {
			width: 72px;
		}

		.scene-row {
			grid-template-columns: minmax(0, 1fr) 96px;
		}

		.scene-row .bar {
			grid-column: 1 / -1;
			grid-row: 2;
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
