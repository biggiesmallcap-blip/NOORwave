<script lang="ts">
	// Journeys across the map. Two kinds:
	// - Somewhere new: from where you are to a scene you have barely touched,
	//   through artists new to you (or to a scene you click on the map).
	// - Collab chain: to any artist you pick, every step a real video both
	//   artists are on.
	// First the choice, then the journey: its stops with why each is there,
	// and Play journey, which the map then follows stop by stop.
	import type { AtlasJourneyResponse, AtlasJourneyStop, ExploreVideo, JourneyKind } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatRuntime, journeyLabel, journeyQueue } from './play';

	let {
		fromName,
		picking,
		journey,
		kind,
		loading,
		error,
		playing,
		onnew,
		oncollab,
		onpickfrom,
		onplay,
		onagain,
		onclose,
		onretry,
		onstop,
	}: {
		/** The start, or null until one is picked. */
		fromName: string | null;
		/** Waiting for a click on the map: the start or the collab destination. */
		picking: 'from' | 'to' | null;
		journey: AtlasJourneyResponse | null;
		kind: JourneyKind | null;
		loading: boolean;
		error: string | null;
		/** This journey is what is playing. */
		playing: boolean;
		onnew: () => void;
		oncollab: () => void;
		onpickfrom: () => void;
		onplay: (queue: ExploreVideo[], label: string, start?: ExploreVideo) => void;
		onagain: () => void;
		onclose: () => void;
		onretry: () => void;
		onstop: (artistId: number) => void;
	} = $props();

	let queue = $derived(journey?.found ? journeyQueue(journey.stops) : []);
	let label = $derived(journey ? journeyLabel(journey) : '');

	function play(video?: ExploreVideo) {
		if (!journey || queue.length === 0) return;
		onplay(queue, label, video ?? queue[0]);
	}

	function stopMenu(event: MouseEvent, stop: AtlasJourneyStop) {
		openContextMenu(event, buildArtistMenu({ tidal_id: stop.artist_id, name: stop.name }), stop.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<article class="card journey" aria-label="Journeys">
	<header class="head">
		<p class="kicker"><span class="journey-glyph" aria-hidden="true"></span>Journeys</p>
		<button type="button" class="close" onclick={onclose} aria-label="Close journeys">
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>

	{#if !journey && !loading && !error}
		<div class="from">
			<span class="from-label">From</span>
			{#if picking === 'from'}
				<span class="from-name waiting">Click an artist on the map</span>
			{:else}
				<span class="from-name">{fromName ?? 'Pick a start'}</span>
				<button type="button" class="link" onclick={onpickfrom}>{fromName ? 'Change' : 'Pick'}</button>
			{/if}
		</div>

		<button type="button" class="choice" disabled={!fromName || picking !== null} onclick={onnew}>
			<span class="choice-name">Somewhere new</span>
			<span class="choice-copy">To a scene you have barely touched, through artists new to you.</span>
		</button>
		<button type="button" class="choice" class:waiting={picking === 'to'} disabled={!fromName || picking === 'from'} onclick={oncollab}>
			<span class="choice-name">Collab chain</span>
			<span class="choice-copy">
				{picking === 'to' ? 'Now click any artist on the map (or find one in the search).' : 'To any artist you pick, every step a video both artists are on.'}
			</span>
		</button>
		<p class="note">Or click a scene on the map to travel there.</p>
	{:else}
		<h2 class="title">{journey?.title ?? (kind === 'collab' ? 'Collab chain' : 'Somewhere new')}</h2>
		{#if error}
			<div class="error" role="alert">
				<p>{error}</p>
				<button type="button" class="link" onclick={onretry}>Retry</button>
			</div>
		{:else if loading}
			<p class="note">{kind === 'collab' ? 'Following collaborations across the map...' : 'Finding a scene you have not been to...'}</p>
			<div class="skeleton">{#each [0, 1, 2, 3] as n (n)}<span class="sk-row tall"></span>{/each}</div>
		{:else if journey && !journey.found}
			<p class="note">{journey.reason ? `${journey.reason[0].toUpperCase()}${journey.reason.slice(1)}.` : 'No journey found.'}</p>
			<div class="actions">
				<button type="button" class="btn btn-secondary" onclick={onagain}>Choose again</button>
			</div>
		{:else if journey}
			<p class="meta-line">
				{#if journey.destination}To <strong>{journey.destination.name}</strong>, {/if}{journey.stops.length} stops, {queue.length} {queue.length === 1 ? 'video' : 'videos'}
			</p>
			<div class="actions">
				<button type="button" class="btn btn-primary" disabled={queue.length === 0} onclick={() => play()}>
					<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" class="fill" /></svg>
					{playing ? 'Playing' : 'Play journey'}
				</button>
				<button type="button" class="btn btn-secondary" onclick={onagain}>New journey</button>
			</div>
			<ol class="stops">
				{#each journey.stops as stop, index (`${stop.artist_id}:${index}`)}
					<li class="stop">
						<span class="num" aria-hidden="true">{index + 1}</span>
						<div class="stop-body">
							<p class="stop-head">
								<button type="button" class="stop-name" onclick={() => onstop(stop.artist_id)} oncontextmenu={(event) => stopMenu(event, stop)}>{stop.name}</button>
							</p>
							{#if stop.why}<p class="why">{stop.why}</p>{/if}
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
	{/if}
</article>

<style>
	.journey-glyph {
		width: 14px;
		height: 2px;
		border-radius: 2px;
		background: #ffe6a8;
		box-shadow: 0 0 8px #ffd27a;
	}

	.from {
		display: flex;
		align-items: baseline;
		gap: var(--space-2);
		min-width: 0;
		font-size: var(--font-size-sm);
	}

	.from-label {
		color: var(--stage-text-3);
	}

	.from-name {
		min-width: 0;
		overflow: hidden;
		color: var(--stage-text);
		font-weight: var(--font-weight-semibold);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.from-name.waiting {
		color: #ffe6a8;
		font-weight: var(--font-weight-medium);
	}

	/* The two kinds: a wide row each, a title and one line on what it does. */
	.choice {
		display: grid;
		gap: 4px;
		width: 100%;
		padding: var(--space-3) var(--space-4);
		border: 1px solid var(--stage-line);
		border-radius: var(--radius-md);
		background: rgba(255, 255, 255, 0.03);
		color: var(--stage-text);
		text-align: left;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast);
	}

	.choice:hover:not(:disabled),
	.choice.waiting {
		border-color: rgba(255, 230, 168, 0.45);
		background: rgba(255, 230, 168, 0.06);
	}

	.choice:disabled {
		cursor: default;
		opacity: 0.5;
	}

	.choice:focus-visible {
		outline: 2px solid #ffe6a8;
		outline-offset: 2px;
	}

	.choice-name {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
	}

	.choice-copy {
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.meta-line {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
	}

	.meta-line strong {
		color: var(--stage-text);
		font-weight: var(--font-weight-semibold);
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

	.why {
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
	}

	.fill {
		fill: currentColor;
		stroke: none;
	}
</style>
