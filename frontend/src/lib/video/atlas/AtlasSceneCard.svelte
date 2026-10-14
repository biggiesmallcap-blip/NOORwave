<script lang="ts">
	// A scene on the Atlas, opened like the artist card: a still from its
	// sample, the name, a few plain sentences (on air or not, how much there
	// is, who leads it, how much of it you have seen), a way onto the air
	// (its On Air channel) and into the Archive, its sample as rows and its
	// biggest artists.
	import type { AtlasScene, AtlasSceneCard, ExploreVideo } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { noteVideoLaunch } from '$lib/video/launch';
	import { tuneChannel } from '$lib/video/onair/tune';
	import { hoursWords, listWords, sceneSeenLine } from './lede';
	import { sceneArchiveUrl } from './links';
	import { formatRuntime, playAtlasQueue } from './play';
	import { formatCount } from './style';

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

	let sample = $derived(card?.sample ?? []);
	let hero = $derived(sample[0]?.artwork_url ?? null);
	let amount = $derived(
		[
			`${formatCount(scene.artists)} ${scene.artists === 1 ? 'artist' : 'artists'}`,
			`${formatCount(scene.videos)} ${scene.videos === 1 ? 'video' : 'videos'}`,
			hoursWords(scene.hours),
		]
			.filter(Boolean)
			.join(', ')
	);
	let leads = $derived(listWords(scene.exemplars, 4));

	function artistMenu(event: MouseEvent, artist: { id: number; name: string }) {
		openContextMenu(event, buildArtistMenu({ tidal_id: artist.id, name: artist.name }), artist.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function play(video: ExploreVideo) {
		void playAtlasQueue(video, card?.sample ?? [video], scene.name);
	}

	// Tune in right here: the channel starts where it is now and the watch
	// page opens on it, gliding from the card's picture. No stop on the dial.
	let tuning = $state(false);

	async function tuneIn(button: HTMLElement) {
		if (tuning || scene.channel_no == null) return;
		tuning = true;
		noteVideoLaunch(button.closest('.card')?.querySelector('.hero-pic') ?? button);
		try {
			await tuneChannel({ id: `scene:${scene.id}`, title: scene.name });
		} finally {
			tuning = false;
		}
	}
</script>

<article class="card" aria-label={`Scene: ${scene.name}`}>
	{#if hero || (loading && !card)}
		<div class="hero">
			{#if hero}
				<button type="button" class="hero-pic" onclick={() => sample[0] && play(sample[0])} aria-label={`Play ${scene.name}`}>
					<ArtworkImage src={hero} size={640} className="hero-img" fallbackText="" decorative fadeIn />
					<span class="hero-play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z" /></svg></span>
				</button>
			{:else}
				<span class="hero-pic hero-wait" aria-hidden="true"></span>
			{/if}
			<button type="button" class="close on-pic" onclick={onclose} aria-label="Close scene card">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
			</button>
		</div>
	{/if}

	<div class="name-row">
		<h2 class="title">{scene.name}</h2>
		{#if !hero && !(loading && !card)}
			<button type="button" class="close" onclick={onclose} aria-label="Close scene card">
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18" /></svg>
			</button>
		{/if}
	</div>

	<p class="lede">
		{#if scene.channel_no != null}On air as channel {scene.channel_no}.{:else}Not on air yet: a scene gets its channel at 30 hours and 40 artists.{/if}
		{amount}{#if leads}, led by {leads}{/if}.
		{sceneSeenLine(scene.seen_videos, scene.lit_artists)}
	</p>

	<div class="actions">
		{#if scene.channel_no != null}
			<button type="button" class="btn btn-primary" disabled={tuning} onclick={(event) => void tuneIn(event.currentTarget)}>
				<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9h16v11H4zM8 4l4 5 4-5" /></svg>
				Tune in
			</button>
		{/if}
		<a class="btn stage-secondary" href={sceneArchiveUrl(scene.id)}>Open in Archive</a>
	</div>

	{#if error}
		<div class="error" role="alert">
			<p>{error}</p>
			<button type="button" class="link" onclick={onretry}>Retry</button>
		</div>
	{:else if loading && !card}
		<div class="skeleton" aria-label="Loading scene">
			{#each [0, 1, 2, 3] as n (n)}<span class="sk-row tall"></span>{/each}
		</div>
	{:else if card}
		{#if sample.length > 0}
			<section class="group" aria-label="Sample videos">
				<h3 class="label">A taste</h3>
				<ul class="rows">
					{#each sample.slice(0, 6) as video (video.tidal_id)}
						<li>
							<button type="button" class="row video-row" onclick={() => play(video)} oncontextmenu={(event) => videoMenu(event, video)}>
								<span class="art small"><ArtworkImage src={video.artwork_url} size={320} className="thumb" alt={video.title} fallbackText={video.artist_name ?? 'NOOR'} tint /></span>
								<span class="row-text">
									<span class="row-name">{video.title}</span>
									{#if video.artist_name}<span class="row-sub">{video.artist_name}</span>{/if}
								</span>
								<span class="row-meta">{formatRuntime(video.duration_ms)}</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}
		{#if card.top_artists.length > 0}
			<section class="group" aria-label="Biggest artists">
				<h3 class="label">At its heart</h3>
				<ul class="rows">
					{#each card.top_artists.slice(0, 10) as artist (artist.id)}
						<li>
							<button type="button" class="row plain" onclick={() => onartist(artist.id)} oncontextmenu={(event) => artistMenu(event, artist)}>
								<span class="row-name">
									{artist.name}
									{#if artist.light === 3}<span class="watched" title="You have watched, liked or saved them">watched</span>{/if}
								</span>
								<span class="row-meta">{formatCount(artist.videos)}</span>
							</button>
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	{/if}
</article>
