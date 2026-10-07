<script lang="ts">
	import type { VideoStationCard } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	let { card, busy = false, onplay }: {
		card: VideoStationCard;
		busy?: boolean;
		onplay: (card: VideoStationCard) => void;
	} = $props();

	let tiles = $derived(Array.from({ length: 4 }, (_, i) => card.preview[i]?.artwork_url ?? null));
</script>

<button
	type="button"
	class="station-card"
	aria-label={`Play ${card.title} station`}
	aria-busy={busy}
	onclick={() => onplay(card)}
>
	<div class="mosaic">
		{#each tiles as src, i (i)}
			<ArtworkImage className="tile" {src} size={320} fallbackText="VID" decorative={true} fadeIn={true} />
		{/each}
		<PlayOverlay position="corner" size="sm" label={`Play ${card.title} station`} />
	</div>
	<span class="title">{card.title}</span>
	<span class="subtitle">{card.subtitle}</span>
</button>

<style>
	.station-card {
		display: grid;
		gap: 4px;
		padding: 0;
		border: 0;
		background: transparent;
		color: inherit;
		text-align: left;
		cursor: pointer;
		min-width: 0;
	}
	.mosaic {
		position: relative;
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 2px;
		border-radius: 10px;
		overflow: hidden;
	}
	.mosaic :global(.tile) {
		aspect-ratio: 16 / 9;
		width: 100%;
		object-fit: cover;
	}
	.title {
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.subtitle {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.station-card:hover :global(.play-overlay),
	.station-card:focus-visible :global(.play-overlay) {
		opacity: 1;
	}
	.station-card[aria-busy='true'] {
		opacity: 0.6;
	}
</style>
