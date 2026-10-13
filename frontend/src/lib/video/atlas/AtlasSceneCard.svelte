<script lang="ts">
	// A scene on the Atlas: its name and exemplars, how much of it you have
	// seen, a way onto the air (its On Air channel) and into the Archive, a
	// strip of sample videos and its biggest artists.
	import type { AtlasScene, AtlasSceneCard, ExploreVideo, Light } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { sceneArchiveUrl, sceneChannelUrl } from './links';
	import { playAtlasQueue } from './play';
	import { formatCount, formatHours, lightSwatch } from './style';

	let {
		scene,
		card,
		loading,
		error,
		onclose,
		onretry,
		onartist,
	}: {
		scene: AtlasScene;
		card: AtlasSceneCard | null;
		loading: boolean;
		error: string | null;
		onclose: () => void;
		onretry: () => void;
		onartist: (id: number) => void;
	} = $props();

	let seenShare = $derived(scene.videos > 0 ? Math.min(1, scene.seen_videos / scene.videos) : 0);
	let exemplars = $derived(scene.exemplars.slice(0, 4).join(', '));

	function artistMenu(event: MouseEvent, artist: { id: number; name: string }) {
		openContextMenu(event, buildArtistMenu({ tidal_id: artist.id, name: artist.name }), artist.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function play(video: ExploreVideo) {
		void playAtlasQueue(video, card?.sample ?? [video], scene.name);
	}
</script>

<article class="card" style={`--hue: ${scene.hue}`} aria-label={`Scene: ${scene.name}`}>
	<header class="head">
		<p class="kicker">
			<span class="swatch" aria-hidden="true"></span>
			{#if scene.channel_no != null}Channel {scene.channel_no}{:else}Scene{/if}
		</p>
		<button type="button" class="close" onclick={onclose} aria-label="Close scene card">
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>
	<h2 class="title">{scene.name}</h2>
	{#if exemplars}<p class="exemplars">{exemplars}</p>{/if}

	<dl class="stats">
		<div><dt>Artists</dt><dd>{formatCount(scene.artists)}</dd></div>
		<div><dt>Videos</dt><dd>{formatCount(scene.videos)}</dd></div>
		<div><dt>Hours</dt><dd>{formatHours(scene.hours)}</dd></div>
	</dl>

	<div class="seen">
		<p class="seen-line">
			<span>Seen {formatCount(scene.seen_videos)} of {formatCount(scene.videos)}</span>
			{#if scene.lit_artists > 0}<span class="lit">{formatCount(scene.lit_artists)} artists lit</span>{/if}
		</p>
		<div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax={scene.videos} aria-valuenow={scene.seen_videos} aria-label="Videos seen in this scene">
			<span style={`width: ${Math.max(seenShare > 0 ? 1.5 : 0, seenShare * 100)}%`}></span>
		</div>
	</div>

	<div class="actions">
		{#if scene.channel_no != null}
			<a class="btn btn-primary" href={sceneChannelUrl(scene.id)}>
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9h16v11H4zM8 4l4 5 4-5" /></svg>
				Tune in
			</a>
		{/if}
		<a class="btn stage-secondary" href={sceneArchiveUrl(scene.id)}>Open in Archive</a>
	</div>
	{#if scene.channel_no == null}
		<p class="note">Not on air yet: a scene gets its channel at 30 hours and 40 artists.</p>
	{/if}

	{#if error}
		<div class="error" role="alert">
			<p>{error}</p>
			<button type="button" class="link" onclick={onretry}>Retry</button>
		</div>
	{:else if loading && !card}
		<div class="skeleton" aria-label="Loading scene">
			<div class="strip">
				{#each [0, 1, 2] as n (n)}<span class="sk-tile"></span>{/each}
			</div>
			{#each [0, 1, 2, 3] as n (n)}<span class="sk-row"></span>{/each}
		</div>
	{:else if card}
		{#if card.sample.length > 0}
			<section class="group" aria-label="Sample videos">
				<h3 class="label">A taste</h3>
				<div class="strip">
					{#each card.sample as video (video.tidal_id)}
						<button
							type="button"
							class="tile"
							onclick={() => play(video)}
							oncontextmenu={(event) => videoMenu(event, video)}
							title={`${video.title} - ${video.artist_name ?? ''}`}
						>
							<span class="art"><ArtworkImage src={video.artwork_url} size={320} className="thumb" alt={video.title} fallbackText={video.artist_name ?? 'NOOR'} tint /></span>
							<span class="tile-title">{video.title}</span>
							<span class="tile-meta">{video.artist_name ?? ''}</span>
						</button>
					{/each}
				</div>
			</section>
		{/if}
		{#if card.top_artists.length > 0}
			<section class="group" aria-label="Biggest artists">
				<h3 class="label">At its heart</h3>
				<ul class="rows">
					{#each card.top_artists.slice(0, 10) as artist (artist.id)}
						<li>
							<button type="button" class="row" onclick={() => onartist(artist.id)} oncontextmenu={(event) => artistMenu(event, artist)}>
								<span class="dot" style={`background: ${lightSwatch(artist.light as Light, scene.hue)}`} aria-hidden="true"></span>
								<span class="row-name">{artist.name}</span>
								<span class="row-meta">{formatCount(artist.videos)}</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/if}
</article>

<style>
	/* Shared card anatomy (.head, .title, .actions, .group, .rows, .thumb ...)
	   lives on the panel in AtlasView, so the three cards read as one family. */
	.swatch {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: hsl(var(--hue), 80%, 66%);
		box-shadow: 0 0 10px hsl(var(--hue), 90%, 60%);
	}

	.exemplars {
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.stats {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--space-2);
		margin: var(--space-2) 0 0;
	}

	.stats div {
		display: grid;
		gap: 2px;
	}

	.stats dt {
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.12em;
	}

	.stats dd {
		margin: 0;
		color: var(--stage-text);
		font-family: var(--font-mono);
		font-size: var(--font-size-md);
		font-variant-numeric: tabular-nums;
	}

	.seen {
		display: grid;
		gap: 6px;
	}

	.seen-line {
		display: flex;
		justify-content: space-between;
		gap: var(--space-2);
		margin: 0;
		color: var(--stage-text-2);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.lit {
		color: hsl(40, 90%, 84%);
	}

	.bar {
		height: 4px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}

	.bar span {
		display: block;
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, hsl(var(--hue), 80%, 55%), hsl(var(--hue), 95%, 78%));
		box-shadow: 0 0 12px hsl(var(--hue), 90%, 60%);
	}

	.strip {
		display: grid;
		grid-auto-flow: column;
		grid-auto-columns: 148px;
		gap: var(--space-3);
		overflow-x: auto;
		scrollbar-width: none;
		padding-bottom: 2px;
		margin: 0 calc(var(--card-pad) * -1);
		padding-inline: var(--card-pad);
		scroll-padding-inline: var(--card-pad);
		scroll-snap-type: x proximity;
	}

	.strip::-webkit-scrollbar {
		display: none;
	}

	.tile {
		display: grid;
		gap: 4px;
		text-align: left;
		color: inherit;
		scroll-snap-align: start;
		min-width: 0;
	}

	.tile-title {
		color: var(--stage-text);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.tile-meta {
		color: var(--stage-text-3);
		font-size: var(--font-size-2xs);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.sk-tile {
		display: block;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.06);
	}
</style>
