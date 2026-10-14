<script lang="ts">
	// An artist on the Atlas, opened like a Today piece: a still from their
	// best cut, the name, a few plain sentences (how much there is, where on
	// the map, what it is tagged, how well you know them), then play and
	// radio, a journey that starts here, their best cuts, the artists they
	// share videos with (drawn on the map too), the neighbours you can fly
	// to, and for uncharted artists a probe that asks the crawler to chart
	// them next.
	import type { AtlasArtistCard, ExploreVideo, Light } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { KNOWN_LINE, genreWords, hoursWords } from './lede';
	import { formatRuntime, playAtlasQueue, playAtlasRadio } from './play';
	import { formatCount } from './style';

	let {
		id,
		name,
		videos,
		light,
		hue,
		sceneName,
		card,
		loading,
		error,
		probe,
		onclose,
		onretry,
		onscene,
		onneighbor,
		onjourney,
		onprobe,
	}: {
		id: number;
		name: string | null;
		videos: number;
		light: Light;
		/** Scene hue, null when uncharted. */
		hue: number | null;
		sceneName: string | null;
		card: AtlasArtistCard | null;
		loading: boolean;
		error: string | null;
		probe: { busy: boolean; message: string | null; sent: boolean };
		onclose: () => void;
		onretry: () => void;
		onscene: (() => void) | null;
		onneighbor: (id: number) => void;
		/** Start a journey somewhere new from this artist. */
		onjourney: () => void;
		onprobe: () => void;
	} = $props();

	let displayName = $derived(card?.name ?? name ?? 'Unnamed artist');
	let effectiveLight = $derived((card?.light ?? light) as Light);
	let uncharted = $derived(card ? !card.charted : hue === null);
	let best = $derived(card?.best ?? []);
	let collabs = $derived(card?.collabs ?? []);
	let collabQueue = $derived(collabs.flatMap((collab) => (collab.video ? [collab.video] : [])));
	let hero = $derived(best[0]?.artwork_url ?? null);
	let videoCount = $derived(card?.videos ?? videos);
	let amount = $derived(
		[`${formatCount(videoCount)} ${videoCount === 1 ? 'video' : 'videos'}`, card ? hoursWords(card.hours) : '']
			.filter(Boolean)
			.join(', ')
	);
	let tagged = $derived(genreWords(card?.genres ?? []));

	function artistMenu(event: MouseEvent, artist: { id: number; name: string }) {
		openContextMenu(event, buildArtistMenu({ tidal_id: artist.id, name: artist.name }), artist.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function playFrom(video: ExploreVideo) {
		void playAtlasQueue(video, best, `${displayName}: best cuts`);
	}

	function playCollab(video: ExploreVideo) {
		void playAtlasQueue(video, collabQueue, `${displayName}: collabs`);
	}
</script>

<article class="card artist" aria-label={`Artist: ${displayName}`}>
	<!-- The picture: the still from their best cut, edge to edge, melting
	     into the panel; it plays the best cuts. -->
	{#if hero || (loading && !card)}
		<div class="hero">
			{#if hero}
				<button type="button" class="hero-pic" onclick={() => best[0] && playFrom(best[0])} aria-label={`Play ${displayName}: best cuts`}>
					<ArtworkImage src={hero} size={640} className="hero-img" fallbackText="" decorative fadeIn />
					<span class="hero-play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z" /></svg></span>
				</button>
			{:else}
				<span class="hero-pic hero-wait" aria-hidden="true"></span>
			{/if}
			<button type="button" class="close on-pic" onclick={onclose} aria-label="Close artist card">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
			</button>
		</div>
	{/if}

	<div class="name-row">
		<h2 class="title" oncontextmenu={(event) => artistMenu(event, { id, name: displayName })}>
			<a href={`/tidal/artists/${id}`} class="title-link">{displayName}</a>
		</h2>
		{#if !hero && !(loading && !card)}
			<button type="button" class="close" onclick={onclose} aria-label="Close artist card">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
			</button>
		{/if}
	</div>

	<p class="lede">
		{amount}{#if uncharted}, not linked to the rest of the map yet.{:else if sceneName}, on the {#if onscene}<button type="button" class="scene-link" onclick={onscene} title="Open the scene">{sceneName}</button>{:else}{sceneName}{/if} side of the map.{:else}.{/if}
		{#if tagged}Tagged {tagged}.{/if}
		{KNOWN_LINE[effectiveLight]}
	</p>

	<div class="actions">
		<button type="button" class="btn btn-primary" disabled={best.length === 0} onclick={() => best[0] && playFrom(best[0])}>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" class="fill" /></svg>
			Play best cuts
		</button>
		<button type="button" class="btn stage-secondary" disabled={best.length === 0} onclick={() => best[0] && void playAtlasRadio(best[0], displayName)}>Radio</button>
	</div>

	<p class="journey">
		<button type="button" class="journey-go" onclick={onjourney} title="Journey somewhere new (J)">Journey somewhere new</button>:
		a playlist from here into a scene you have not watched.
	</p>

	{#if uncharted}
		<div class="probe">
			<p>No graph links yet, so this artist sits on the outer ring.</p>
			<button type="button" class="btn stage-secondary" disabled={probe.busy || probe.sent} onclick={onprobe}>
				{probe.busy ? 'Sending...' : probe.sent ? 'Probe sent' : 'Send a probe'}
			</button>
			{#if probe.message}<p class="probe-msg" role="status">{probe.message}</p>{/if}
		</div>
	{/if}

	{#if error}
		<div class="error" role="alert">
			<p>{error}</p>
			<button type="button" class="link" onclick={onretry}>Retry</button>
		</div>
	{:else if loading && !card}
		<div class="skeleton" aria-label="Loading artist">
			{#each [0, 1, 2, 3] as n (n)}<span class="sk-row tall"></span>{/each}
		</div>
	{:else if card}
		{#if best.length > 0}
			<section class="group" aria-label="Best cuts">
				<h3 class="label">Best cuts</h3>
				<ul class="rows">
					{#each best as video (video.tidal_id)}
						<li>
							<button type="button" class="row video-row" onclick={() => playFrom(video)} oncontextmenu={(event) => videoMenu(event, video)}>
								<span class="art small"><ArtworkImage src={video.artwork_url} size={320} className="thumb" alt={video.title} fallbackText={displayName} tint /></span>
								<span class="row-text">
									<span class="row-name">{video.title}</span>
									{#if video.why}<span class="row-sub">{video.why}</span>{/if}
								</span>
								<span class="row-meta">{formatRuntime(video.duration_ms)}</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{:else}
			<p class="note">No playable cuts in the catalog yet.</p>
		{/if}
		{#if collabs.length > 0}
			<section class="group" aria-label="Collabs">
				<h3 class="label">Collabs{#if card.collabs_total > collabs.length}<span class="count">{card.collabs_total}</span>{/if}</h3>
				<ul class="rows">
					{#each collabs as collab (collab.id)}
						<li>
							{#if collab.video}
								{@const video = collab.video}
								<button type="button" class="row video-row" onclick={() => playCollab(video)} oncontextmenu={(event) => videoMenu(event, video)}>
									<span class="art small"><ArtworkImage src={video.artwork_url} size={320} className="thumb" alt={video.title} fallbackText={collab.name} tint /></span>
									<span class="row-text">
										<span class="row-name">{collab.name}</span>
										<span class="row-sub">{video.title}</span>
									</span>
									<span class="row-meta">{formatRuntime(video.duration_ms)}</span>
								</button>
							{:else}
								<button type="button" class="row plain" onclick={() => onneighbor(collab.id)} oncontextmenu={(event) => artistMenu(event, collab)}>
									<span class="row-name">
										{collab.name}
										{#if collab.light === 3}<span class="watched" title="You have watched, liked or saved them">watched</span>{/if}
									</span>
								</button>
							{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}
		{#if card.neighbors.length > 0}
			<section class="group" aria-label="Neighbours">
				<h3 class="label">Neighbours</h3>
				<ul class="rows">
					{#each card.neighbors.slice(0, 10) as neighbor (neighbor.id)}
						<li>
							<button type="button" class="row plain" onclick={() => onneighbor(neighbor.id)} oncontextmenu={(event) => artistMenu(event, neighbor)}>
								<span class="row-name">
									{neighbor.name}
									{#if neighbor.light === 3}<span class="watched" title="You have watched, liked or saved them">watched</span>{/if}
								</span>
								<span class="row-tag">{neighbor.source}</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/if}
</article>

<style>
	/* Shared card anatomy (the picture, name row, lede, rows) lives on the
	   panel in AtlasView. */

	.title-link {
		color: inherit;
		text-decoration: none;
	}

	.title-link:hover {
		text-decoration: underline;
		text-decoration-thickness: 1px;
		text-underline-offset: 5px;
	}

	.scene-link,
	.journey-go {
		padding: 0;
		border: 0;
		background: none;
		color: var(--stage-text);
		font: inherit;
		text-decoration: underline;
		text-decoration-color: rgba(255, 255, 255, 0.3);
		text-decoration-thickness: 1px;
		text-underline-offset: 3px;
		cursor: pointer;
	}

	.scene-link:hover,
	.journey-go:hover {
		text-decoration-color: currentColor;
	}

	.scene-link:focus-visible,
	.journey-go:focus-visible {
		outline: 2px solid var(--stage-text);
		outline-offset: 2px;
		border-radius: 2px;
	}

	/* The journey, said in one line: the action, then what it is. */
	.journey {
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.journey-go {
		font-weight: var(--font-weight-semibold);
	}

	.probe {
		display: grid;
		gap: var(--space-2);
		justify-items: start;
		padding: var(--space-3);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.05);
	}

	.probe p {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-normal);
	}

	.probe .probe-msg {
		color: hsl(40, 90%, 84%);
	}

	.fill {
		fill: currentColor;
		stroke: none;
	}

	.count {
		margin-left: var(--space-2);
		color: var(--stage-text-3);
		font-variant-numeric: tabular-nums;
	}
</style>
