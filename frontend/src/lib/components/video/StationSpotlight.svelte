<script lang="ts">
	import { api, type TidalSearchVideo, type VideoStationCard } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { cleanBio, spotlightArtistId, stationFrames } from './stations';

	// Channel 01: the day's artist. A short intro (portrait, why it is here,
	// a line of bio) beside a mosaic of their videos. The profile loads after
	// the guide and only adds to the row; without it the row is complete.
	let { card, number, busy = false, onAir = false, onplay }: {
		card: VideoStationCard;
		number: string;
		busy?: boolean;
		onAir?: boolean;
		onplay: (card: VideoStationCard, startWith?: TidalSearchVideo) => void;
	} = $props();

	let artistId = $derived(spotlightArtistId(card));
	let frames = $derived(stationFrames(card).slice(0, 5));
	let lead = $derived(frames[0]?.artwork_url ?? null);
	let portrait = $state<string | null>(null);
	let bio = $state<string | null>(null);

	$effect(() => {
		const id = artistId;
		portrait = null;
		bio = null;
		if (id == null) return;
		let live = true;
		api.getTidalArtistProfile(id, true)
			.then((profile) => {
				if (!live) return;
				portrait = profile.picture_url ?? null;
				bio = cleanBio(profile.bio?.summary) ?? cleanBio(profile.bio?.text);
			})
			.catch(() => {});
		return () => {
			live = false;
		};
	});

	function frameLabel(video: TidalSearchVideo): string {
		return video.artist_name ? `${video.title} - ${video.artist_name}` : video.title;
	}

	function frameMenu(event: MouseEvent, video: TidalSearchVideo) {
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function artistMenu(event: MouseEvent) {
		if (artistId == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: artistId, name: card.title, in_library: false }, { isLocal: false }), card.title);
	}
</script>

<article class="spotlight" class:on-air={onAir} aria-busy={busy} aria-label={`Today's spotlight: ${card.title}`}>
	<div class="wash" aria-hidden="true">
		<ArtworkImage src={lead} size={320} fallbackText="" decorative={true} fadeIn={true} />
	</div>

	<div class="intro">
		<div class="who">
			<div class="portrait">
				<ArtworkImage src={portrait ?? lead} size={160} fallbackText={card.title.slice(0, 1)} decorative={true} fadeIn={true} />
			</div>
			<div class="titles">
				<span class="eyebrow">{number} Today's spotlight{#if onAir} - On air{/if}</span>
				<button type="button" class="name" oncontextmenu={artistMenu} onclick={() => onplay(card)}>{card.title}</button>
			</div>
		</div>
		<p class="why">{card.subtitle}</p>
		{#if bio}
			<p class="bio">{bio}</p>
		{/if}
		<div class="actions">
			<button type="button" class="btn btn-primary" disabled={busy} onclick={() => onplay(card)}>
				{busy ? 'Starting...' : 'Play station'}
			</button>
			{#if artistId != null}
				<a class="btn btn-glass" href={`/tidal/artists/${artistId}`}>Artist page</a>
			{/if}
		</div>
	</div>

	<div class="mosaic" data-count={frames.length}>
		{#each frames as video, index (video.tidal_id)}
			<button
				type="button"
				class="frame"
				class:lead={index === 0}
				aria-label={`Play ${card.title} from ${frameLabel(video)}`}
				onclick={() => onplay(card, video)}
				oncontextmenu={(event) => frameMenu(event, video)}
			>
				<ArtworkImage src={video.artwork_url} size={index === 0 ? 640 : 320} fallbackText="" decorative={true} fadeIn={true} />
				<span class="caption" aria-hidden="true">
					<span class="caption-title">{video.title}</span>
					{#if video.artist_name}<span class="caption-artist">{video.artist_name}</span>{/if}
				</span>
			</button>
		{/each}
	</div>
</article>

<style>
	.spotlight {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 0.85fr) minmax(0, 1.15fr);
		gap: clamp(18px, 2.6vw, 36px);
		align-items: center;
		padding: clamp(14px, 2vw, 22px);
		border-radius: 16px;
		overflow: hidden;
		isolation: isolate;
	}
	.spotlight[aria-busy='true'] { opacity: 0.7; }
	.spotlight.on-air { box-shadow: inset 3px 0 0 var(--accent); }

	/* A faint wash of the lead frame, so the row reads as the featured one
	   without a heavy card. */
	.wash {
		position: absolute;
		inset: 0;
		z-index: -1;
		opacity: 0.2;
		filter: blur(56px) saturate(1.3);
		transform: scale(1.25);
		pointer-events: none;
	}
	.wash :global(img) { width: 100%; height: 100%; object-fit: cover; }

	.intro {
		display: grid;
		gap: 10px;
		justify-items: start;
		min-width: 0;
	}
	.who {
		display: flex;
		align-items: center;
		gap: 14px;
		min-width: 0;
		max-width: 100%;
	}
	.portrait {
		flex: 0 0 auto;
		width: 72px;
		height: 72px;
		border-radius: 50%;
		overflow: hidden;
		background: var(--bg-raised);
	}
	.portrait :global(img) { width: 100%; height: 100%; object-fit: cover; }
	.titles {
		display: grid;
		gap: 2px;
		min-width: 0;
	}
	/* The section label from /search: small, uppercase, accent. */
	.eyebrow {
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 1.5px;
		color: var(--accent);
	}
	.name {
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-align: left;
		cursor: pointer;
		overflow-wrap: anywhere;
	}
	.name:hover { text-decoration: underline; text-underline-offset: 4px; }
	.why {
		margin: 0;
		color: var(--text-secondary);
	}
	/* One sentence, already shortened at a clause break by cleanBio, so it
	   is never clipped here. */
	.bio {
		max-width: 62ch;
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		line-height: 1.55;
		overflow-wrap: anywhere;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	/* Lead frame over two rows, four small frames beside it. */
	.mosaic {
		display: grid;
		grid-template-columns: 2fr 1fr 1fr;
		gap: 6px;
		min-width: 0;
	}
	.mosaic[data-count='1'] { grid-template-columns: 1fr; }
	.mosaic[data-count='2'],
	.mosaic[data-count='3'] { grid-template-columns: 2fr 1fr; }
	.frame {
		position: relative;
		padding: 0;
		border: 0;
		border-radius: 8px;
		overflow: hidden;
		aspect-ratio: 16 / 9;
		background: var(--bg-raised);
		cursor: pointer;
		transition: transform var(--motion-fast);
	}
	/* The lead takes the height of the two small rows; its image is lifted
	   out of flow so a square source can't stretch them. */
	.frame.lead {
		position: relative;
		grid-row: span 2;
		aspect-ratio: auto;
		border-radius: 12px;
	}
	.frame.lead :global(img) {
		position: absolute;
		inset: 0;
	}
	.mosaic[data-count='1'] .frame.lead,
	.mosaic[data-count='2'] .frame.lead {
		grid-row: auto;
		aspect-ratio: 16 / 9;
	}
	.frame :global(img) { width: 100%; height: 100%; object-fit: cover; }
	.frame:hover,
	.frame:focus-visible { transform: translateY(-2px); }
	.caption {
		position: absolute;
		inset: auto 0 0 0;
		display: grid;
		padding: 22px 10px 8px;
		background: linear-gradient(to top, rgba(0, 0, 0, 0.88), rgba(0, 0, 0, 0.55) 60%, transparent);
		color: #fff;
		text-align: left;
		opacity: 0;
		transition: opacity 120ms ease;
		pointer-events: none;
	}
	.frame:hover .caption,
	.frame:focus-visible .caption { opacity: 1; }
	.caption-title,
	.caption-artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		line-height: 1.25;
	}
	.caption-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}
	.caption-artist {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.78);
	}
	.frame:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	@media (max-width: 860px) {
		.spotlight { grid-template-columns: minmax(0, 1fr); }
		.name { font-size: var(--font-size-xl); }
	}
</style>
