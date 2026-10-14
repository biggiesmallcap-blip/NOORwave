<script lang="ts">
	// Journeys across the map, drawn as the trip itself: where it starts, a
	// gold thread of stops (the map's route colour), and where it goes. Pick a
	// start, then click any artist or scene to go there, or press the empty
	// end and the map chooses somewhere new. The length is the thread: Short,
	// Long and Epic pass through one, two or four new scenes (about 6, 12 or
	// 24 stops), and changing it re-plans the journey on screen. Then its
	// stops, and Play journey, which the map follows stop by stop.
	import type { AtlasJourneyResponse, AtlasJourneyStop, ExploreVideo, JourneyLength } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { listWords } from './lede';
	import { formatRuntime, journeyLabel, journeyQueue } from './play';

	let {
		fromName,
		fromArt,
		end,
		picking,
		choosing,
		length,
		journey,
		loading,
		error,
		playing,
		onlength,
		onnew,
		onpickfrom,
		onplay,
		onagain,
		onclose,
		onretry,
		onstop,
	}: {
		/** The start, or null until one is picked. */
		fromName: string | null;
		fromArt: string | null;
		/** Where it goes, once chosen: an artist (with picture) or a scene. */
		end: { name: string; art: string | null } | null;
		/** Waiting for a click on the map for the start. */
		picking: 'from' | null;
		/** A start is set and nothing is planned: the next click is where to go. */
		choosing: boolean;
		length: JourneyLength;
		journey: AtlasJourneyResponse | null;
		loading: boolean;
		error: string | null;
		/** This journey is what is playing. */
		playing: boolean;
		onlength: (length: JourneyLength) => void;
		onnew: () => void;
		onpickfrom: () => void;
		onplay: (queue: ExploreVideo[], label: string, start?: ExploreVideo) => void;
		onagain: () => void;
		onclose: () => void;
		onretry: () => void;
		onstop: (artistId: number) => void;
	} = $props();

	const LENGTHS: readonly { value: JourneyLength; label: string; stops: number; says: string }[] = [
		{ value: 'short', label: 'Short', stops: 6, says: 'about 6 stops' },
		{ value: 'long', label: 'Long', stops: 12, says: 'about 12 stops' },
		{ value: 'epic', label: 'Epic', stops: 24, says: 'about 24 stops' },
	];

	let queue = $derived(journey?.found ? journeyQueue(journey.stops) : []);
	let label = $derived(journey ? journeyLabel(journey) : '');
	let through = $derived(journey?.through?.length ? listWords(journey.through, 4) : '');
	let planned = $derived(Boolean(journey?.found));

	/** The thread between the ends: the planned stops (scene crossings
	 *  drawn larger), or before planning, as many dots as the length. */
	let beads = $derived.by(() => {
		if (journey?.found) {
			return journey.stops.slice(1, -1).map((stop) => (stop.why.startsWith('Into ') ? 'scene' : 'stop'));
		}
		const count = LENGTHS.find((option) => option.value === length)?.stops ?? 12;
		return Array.from({ length: count - 2 }, () => 'stop');
	});

	let endArt = $derived(journey?.found ? (journey.stops.at(-1)?.video?.artwork_url ?? end?.art ?? null) : (end?.art ?? null));
	let endName = $derived(end?.name ?? journey?.destination?.name ?? null);

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
	<div class="name-row">
		<h2 class="title">{journey?.found ? journey.title : 'Journeys'}</h2>
		<button type="button" class="close" onclick={onclose} aria-label="Close journeys">
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</div>

	<!-- The trip: start, thread, end. -->
	<div class="trip" class:planned class:busy={loading}>
		<button type="button" class="end" class:waiting={picking === 'from' || !fromName} onclick={onpickfrom} disabled={loading} title="Change where it starts">
			<span class="disc">
				{#if fromArt}<ArtworkImage src={fromArt} size={160} className="disc-img" fallbackText="" decorative />{/if}
			</span>
			<span class="end-name">{picking === 'from' || !fromName ? 'Pick a start' : fromName}</span>
			<span class="end-sub">{picking === 'from' || !fromName ? 'click an artist' : 'start'}</span>
		</button>

		<div class="thread" aria-hidden="true">
			{#each beads as bead, i (i)}<i class:scene={bead === 'scene'} style={`--i: ${i}`}></i>{/each}
		</div>

		{#if endName || planned}
			<span class="end">
				<span class="disc">
					{#if endArt}<ArtworkImage src={endArt} size={160} className="disc-img" fallbackText="" decorative />{/if}
				</span>
				<span class="end-name">{endName}</span>
				<span class="end-sub">{loading ? 'planning' : 'end'}</span>
			</span>
		{:else}
			<!-- Nothing chosen yet: the empty end lets the map choose. -->
			<button type="button" class="end open" disabled={!choosing} onclick={onnew} title="Let the map choose somewhere new to you">
				<span class="disc">?</span>
				<span class="end-name">Somewhere new</span>
				<span class="end-sub">or click any artist or scene</span>
			</button>
		{/if}
	</div>

	<div class="lengths" role="radiogroup" aria-label="Journey length">
		{#each LENGTHS as option (option.value)}
			<button
				type="button"
				role="radio"
				aria-checked={length === option.value}
				class:active={length === option.value}
				disabled={loading}
				onclick={() => onlength(option.value)}
			>{option.label}</button>
		{/each}
		<span class="says">{LENGTHS.find((option) => option.value === length)?.says}</span>
	</div>

	{#if error}
		<div class="error" role="alert">
			<p>{error}</p>
			<button type="button" class="link" onclick={onretry}>Retry</button>
		</div>
	{:else if loading}
		<div class="skeleton">{#each [0, 1, 2, 3] as n (n)}<span class="sk-row tall"></span>{/each}</div>
	{:else if journey && !journey.found}
		<p class="lede">{journey.reason ? `${journey.reason[0].toUpperCase()}${journey.reason.slice(1)}.` : 'No journey found.'} Try a shorter length, or somewhere else.</p>
		<div class="actions">
			<button type="button" class="btn stage-secondary" onclick={onagain}>Choose again</button>
		</div>
	{:else if journey}
		<p class="lede">
			{through ? `Through ${through}. ` : ''}{journey.stops.length} stops, {queue.length} {queue.length === 1 ? 'video' : 'videos'}.
		</p>
		<div class="actions">
			<button type="button" class="btn btn-primary" disabled={queue.length === 0} onclick={() => play()}>
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" class="fill" /></svg>
				{playing ? 'Playing' : 'Play journey'}
			</button>
			<button type="button" class="btn stage-secondary" onclick={onagain}>New journey</button>
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
</article>

<style>
	/* The trip: two ends and the thread between them, in the map's route
	   gold, so the card and the line on the map read as one thing. */
	.trip {
		display: grid;
		grid-template-columns: 88px minmax(0, 1fr) 88px;
		align-items: start;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	.end {
		display: grid;
		justify-items: center;
		gap: 4px;
		min-width: 0;
		padding: 0;
		border: 0;
		background: none;
		color: var(--stage-text);
		text-align: center;
	}

	button.end {
		cursor: pointer;
	}

	button.end:disabled {
		cursor: default;
	}

	.disc {
		position: relative;
		display: grid;
		place-items: center;
		width: 52px;
		height: 52px;
		border-radius: 50%;
		overflow: hidden;
		background: rgba(255, 255, 255, 0.05);
		box-shadow: 0 0 0 1.5px #ffe6a8, 0 0 18px rgba(255, 210, 122, 0.25);
		color: #ffe6a8;
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-style: italic;
	}

	.end :global(.disc-img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	/* An end still to choose: a dashed ring, breathing gently. */
	.end.waiting .disc,
	.end.open .disc {
		box-shadow: none;
		border: 1.5px dashed rgba(255, 230, 168, 0.6);
		background: transparent;
	}

	.end.open:not(:disabled) .disc {
		animation: end-breathe 2.4s var(--ease-standard) infinite;
	}

	.end.open:not(:disabled):hover .disc {
		border-style: solid;
		background: rgba(255, 230, 168, 0.08);
	}

	.end.open:disabled {
		opacity: 0.45;
	}

	@keyframes end-breathe {
		50% { box-shadow: 0 0 16px rgba(255, 210, 122, 0.35); }
	}

	/* Names wrap to two lines rather than cut off. */
	.end-name {
		display: -webkit-box;
		max-width: 100%;
		overflow: hidden;
		color: var(--stage-text);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
		overflow-wrap: anywhere;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
	}

	.end-sub {
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		line-height: var(--line-height-snug);
	}

	.end:focus-visible {
		outline: 2px solid #ffe6a8;
		outline-offset: 4px;
		border-radius: var(--radius-sm);
	}

	/* The stops as beads on a gold line; scene crossings are rings. */
	.thread {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: space-between;
		height: 52px;
		padding: 0 6px;
	}

	.thread::before {
		content: '';
		position: absolute;
		left: 0;
		right: 0;
		top: 50%;
		height: 1px;
		background: linear-gradient(90deg, rgba(255, 230, 168, 0.7), rgba(255, 230, 168, 0.35));
	}

	.trip:not(.planned) .thread::before {
		background: repeating-linear-gradient(90deg, rgba(255, 230, 168, 0.5) 0 4px, transparent 4px 8px);
	}

	.thread i {
		position: relative;
		flex: none;
		width: 4px;
		height: 4px;
		border-radius: 50%;
		background: #ffe6a8;
		opacity: 0.85;
		animation: bead-in var(--motion-base) both;
		animation-delay: calc(min(var(--i), 24) * 12ms);
	}

	.thread i.scene {
		width: 9px;
		height: 9px;
		background: #05060c;
		box-shadow: inset 0 0 0 1.5px #ffe6a8;
		opacity: 1;
	}

	.trip.busy .thread i {
		animation: bead-wait 1.2s var(--ease-standard) infinite;
		animation-delay: calc(var(--i) * 40ms);
	}

	@keyframes bead-in {
		from { opacity: 0; transform: scale(0.2); }
	}

	@keyframes bead-wait {
		50% { opacity: 0.25; }
	}

	/* Length as three words under the thread, the chosen one lit. */
	.lengths {
		display: flex;
		align-items: baseline;
		justify-content: center;
		gap: var(--space-4);
		margin: 0;
	}

	.lengths button {
		padding: 2px 0;
		border: 0;
		border-bottom: 1px solid transparent;
		background: none;
		color: var(--stage-text-3);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: color var(--motion-fast), border-color var(--motion-fast);
	}

	.lengths button:hover:not(:disabled) {
		color: var(--stage-text);
	}

	.lengths button.active {
		color: #ffe6a8;
		border-bottom-color: rgba(255, 230, 168, 0.6);
	}

	.lengths button:focus-visible {
		outline: 2px solid #ffe6a8;
		outline-offset: 3px;
	}

	.says {
		color: var(--stage-text-3);
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

	.why {
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
	}

	.fill {
		fill: currentColor;
		stroke: none;
	}

	@media (prefers-reduced-motion: reduce) {
		.thread i,
		.trip.busy .thread i,
		.end.open:not(:disabled) .disc {
			animation: none;
		}
	}
</style>
