<script lang="ts">
	import { api, type TidalSearchVideo, type VideoStationCard } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import GuideFeature from './GuideFeature.svelte';
	import { cleanBio, spotlightArtistId, stationFrames } from './stations';

	// Channel 01: the day's artist. A short intro (portrait, why it is here,
	// a line of bio) beside a mosaic of their videos. The profile loads after
	// the guide and only adds to the row; without it the row is complete.
	let { card, number, busy = false, onAir = false, rise = null, onplay }: {
		card: VideoStationCard;
		number: string;
		busy?: boolean;
		onAir?: boolean;
		rise?: number | null;
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

	function artistMenu(event: MouseEvent) {
		if (artistId == null) return;
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: artistId, name: card.title, in_library: false }, { isLocal: false }), card.title);
	}
</script>

<GuideFeature label={`Today's spotlight: ${card.title}`} {frames} {busy} {onAir} {rise} onpick={(video) => onplay(card, video)}>
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
</GuideFeature>

<style>
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
		line-height: var(--line-height-normal);
		overflow-wrap: anywhere;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-top: var(--space-1);
	}

	@media (max-width: 860px) {
		.name { font-size: var(--font-size-xl); }
	}
</style>
