<script lang="ts">
	// A route between two artists: the stops in order, each with the one video
	// that represents it, and Play route to watch the journey end to end.
	import type { AtlasRouteResponse, AtlasRouteStop, ExploreVideo } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatRuntime, playAtlasQueue, routeLabel, routeQueue } from './play';

	let {
		fromName,
		toName,
		route,
		loading,
		error,
		onclose,
		onretry,
		onstop,
	}: {
		fromName: string;
		toName: string;
		route: AtlasRouteResponse | null;
		loading: boolean;
		error: string | null;
		onclose: () => void;
		onretry: () => void;
		onstop: (artistId: number) => void;
	} = $props();

	let queue = $derived(route ? routeQueue(route.stops) : []);

	function play(video?: ExploreVideo) {
		if (!route || queue.length === 0) return;
		void playAtlasQueue(video ?? queue[0], queue, routeLabel(route.stops));
	}

	function stopMenu(event: MouseEvent, stop: AtlasRouteStop) {
		openContextMenu(event, buildArtistMenu({ tidal_id: stop.artist_id, name: stop.name }), stop.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<article class="card route" aria-label="Route">
	<header class="head">
		<p class="kicker"><span class="route-glyph" aria-hidden="true"></span>Route</p>
		<button type="button" class="close" onclick={onclose} aria-label="Clear route">
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>
	<h2 class="title">{fromName} <span class="to">to</span> {toName}</h2>

	{#if error}
		<div class="error" role="alert">
			<p>{error}</p>
			<button type="button" class="link" onclick={onretry}>Retry</button>
		</div>
	{:else if loading}
		<p class="note">Plotting the shortest path through the artist graph...</p>
		<div class="skeleton">{#each [0, 1, 2, 3] as n (n)}<span class="sk-row tall"></span>{/each}</div>
	{:else if route && !route.found}
		<p class="note">No path within six hops. These two have no chain of links between them yet; try artists in neighbouring scenes.</p>
	{:else if route}
		<p class="meta-line">{route.stops.length} stops, {queue.length} {queue.length === 1 ? 'video' : 'videos'}</p>
		<div class="actions">
			<button type="button" class="btn btn-primary" disabled={queue.length === 0} onclick={() => play()}>
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" class="fill" /></svg>
				Play route
			</button>
		</div>
		<ol class="stops">
			{#each route.stops as stop, index (stop.artist_id)}
				<li class="stop">
					<span class="num" aria-hidden="true">{index + 1}</span>
					<div class="stop-body">
						<p class="stop-head">
							<button type="button" class="stop-name" onclick={() => onstop(stop.artist_id)} oncontextmenu={(event) => stopMenu(event, stop)}>{stop.name}</button>
							{#if stop.via}<span class="row-tag">via {stop.via}</span>{/if}
						</p>
						{#if stop.video}
							{@const video = stop.video}
							<button type="button" class="row video-row" onclick={() => play(video)} oncontextmenu={(event) => videoMenu(event, video)}>
								<span class="art small"><ArtworkImage src={video.artwork_url} size={320} className="thumb" alt={video.title} fallbackText={stop.name} tint /></span>
								<span class="row-text"><span class="row-name">{video.title}</span></span>
								<span class="row-meta">{formatRuntime(video.duration_ms)}</span>
							</button>
						{:else}
							<p class="note">No playable video for this stop.</p>
						{/if}
					</div>
				</li>
			{/each}
		</ol>
	{/if}
</article>

<style>
	.route-glyph {
		width: 14px;
		height: 2px;
		border-radius: 2px;
		background: #ffe6a8;
		box-shadow: 0 0 8px #ffd27a;
	}

	.to {
		color: var(--stage-text-3);
		font-style: italic;
		font-weight: var(--font-weight-medium);
	}

	.meta-line {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
	}

	.stops {
		display: grid;
		gap: var(--space-3);
		margin: 0;
		padding: 0;
		list-style: none;
		position: relative;
	}

	.stop {
		display: grid;
		grid-template-columns: 22px minmax(0, 1fr);
		gap: var(--space-2);
		position: relative;
	}

	/* The thread between numbered stops. */
	.stop:not(:last-child)::before {
		content: '';
		position: absolute;
		left: 10px;
		top: 24px;
		bottom: calc(var(--space-3) * -1);
		width: 1px;
		background: linear-gradient(rgba(255, 230, 168, 0.55), rgba(255, 230, 168, 0.12));
	}

	.num {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		border-radius: 50%;
		border: 1.5px solid #ffe6a8;
		color: #ffe6a8;
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		line-height: 1;
		background: rgba(8, 10, 20, 0.9);
	}

	.stop-body {
		display: grid;
		gap: 4px;
		min-width: 0;
	}

	.stop-head {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin: 2px 0 0;
		min-width: 0;
	}

	.stop-name {
		color: var(--stage-text);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.stop-name:hover {
		text-decoration: underline;
	}

	.fill {
		fill: currentColor;
		stroke: none;
	}
</style>
