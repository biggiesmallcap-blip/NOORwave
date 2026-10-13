<script lang="ts">
	// An artist on the Atlas: where they sit, how lit they are for you, their
	// best cuts (playable as a queue), radio, the neighbours you can fly to, and
	// for uncharted artists a probe that asks the crawler to chart them next.
	import type { AtlasArtistCard, ExploreVideo, Light } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatRuntime, playAtlasQueue, playAtlasRadio } from './play';
	import { LIGHT_HELP, LIGHT_NAMES, formatCount, formatHours, lightSwatch } from './style';

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
		onroute,
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
		onroute: () => void;
		onprobe: () => void;
	} = $props();

	let displayName = $derived(card?.name ?? name ?? 'Unnamed artist');
	let effectiveLight = $derived((card?.light ?? light) as Light);
	let uncharted = $derived(card ? !card.charted : hue === null);
	let best = $derived(card?.best ?? []);

	function artistMenu(event: MouseEvent, artist: { id: number; name: string }) {
		openContextMenu(event, buildArtistMenu({ tidal_id: artist.id, name: artist.name }), artist.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function playFrom(video: ExploreVideo) {
		void playAtlasQueue(video, best, `${displayName}: best cuts`);
	}
</script>

<article class="card" style={`--hue: ${hue ?? 222}`} aria-label={`Artist: ${displayName}`}>
	<header class="head">
		<p class="kicker">
			<span class="dot" style={`background: ${lightSwatch(effectiveLight, hue)}`} aria-hidden="true"></span>
			{#if sceneName && onscene}
				<button type="button" class="scene-link" onclick={onscene}>{sceneName}</button>
			{:else if uncharted}
				Uncharted
			{:else}
				{sceneName ?? 'Charted'}
			{/if}
		</p>
		<button type="button" class="close" onclick={onclose} aria-label="Close artist card">
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>
	<h2 class="title" oncontextmenu={(event) => artistMenu(event, { id, name: displayName })}>
		<a href={`/tidal/artists/${id}`} class="title-link">{displayName}</a>
	</h2>
	<p class="meta-line">
		<span>{formatCount(card?.videos ?? videos)} videos</span>
		{#if card}<span>{formatHours(card.hours)}</span><span>{formatCount(card.playable)} playable</span>{/if}
	</p>
	<p class="light-line">
		<span class="light-name" style={`color: ${lightSwatch(effectiveLight, hue)}`}>{LIGHT_NAMES[effectiveLight]}</span>
		<span>{LIGHT_HELP[effectiveLight]}</span>
	</p>
	{#if card && card.genres.length > 0}
		<ul class="genres">
			{#each card.genres.slice(0, 6) as genre (genre)}<li>{genre}</li>{/each}
		</ul>
	{/if}

	<div class="actions">
		<button type="button" class="btn btn-primary" disabled={best.length === 0} onclick={() => best[0] && playFrom(best[0])}>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14l11-7z" class="fill" /></svg>
			Play best cuts
		</button>
		<button type="button" class="btn stage-secondary" disabled={best.length === 0} onclick={() => best[0] && void playAtlasRadio(best[0], displayName)}>Radio</button>
		<button type="button" class="btn stage-ghost" onclick={onroute} title="Start a journey from this artist (J)">Journey from here</button>
	</div>

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
		{#if card.neighbors.length > 0}
			<section class="group" aria-label="Neighbours">
				<h3 class="label">Neighbours</h3>
				<ul class="rows">
					{#each card.neighbors.slice(0, 10) as neighbor (neighbor.id)}
						<li>
							<button type="button" class="row" onclick={() => onneighbor(neighbor.id)} oncontextmenu={(event) => artistMenu(event, neighbor)}>
								<span class="dot" style={`background: ${lightSwatch(neighbor.light, hue)}`} aria-hidden="true"></span>
								<span class="row-name">{neighbor.name}</span>
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
	/* Shared card anatomy lives on the panel in AtlasView. */
	.scene-link {
		color: hsl(var(--hue), 80%, 80%);
		font: inherit;
		letter-spacing: inherit;
		text-transform: inherit;
	}

	.scene-link:hover {
		text-decoration: underline;
	}

	.title-link {
		color: inherit;
		text-decoration: none;
	}

	.title-link:hover {
		text-decoration: underline;
		text-decoration-thickness: 1px;
		text-underline-offset: 4px;
	}

	.meta-line {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.light-line {
		display: flex;
		gap: var(--space-2);
		align-items: baseline;
		margin: 0;
		color: var(--stage-text-3);
		font-size: var(--font-size-xs);
	}

	.light-name {
		font-weight: var(--font-weight-semibold);
	}

	.genres {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.genres li {
		padding: 3px 9px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.07);
		color: var(--stage-text-2);
		font-size: var(--font-size-2xs);
		line-height: 1;
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
</style>
