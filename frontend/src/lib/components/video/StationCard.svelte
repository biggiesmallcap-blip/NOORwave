<script lang="ts">
	import type { VideoStationCard } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	// One station: a single lead frame (the 2x2 mosaic of letterboxed stills
	// read as noise), two offset edges behind it so it reads as a collection
	// rather than one video, and a meta line chosen by the row (see
	// `stationMeta`) so a shelf never repeats the same sentence.
	let { card, meta, busy = false, onplay }: {
		card: VideoStationCard;
		meta: string;
		busy?: boolean;
		onplay: (card: VideoStationCard) => void;
	} = $props();

	let lead = $derived(card.preview.find((video) => video.artwork_url)?.artwork_url ?? null);
	const compact = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });
</script>

<button
	type="button"
	class="station-card"
	aria-label={`Play ${card.title} station`}
	aria-busy={busy}
	title={card.subtitle}
	onclick={() => onplay(card)}
>
	<div class="stack">
		<div class="art">
			<ArtworkImage className="lead" src={lead} size={640} fallbackText="VID" decorative={true} fadeIn={true} />
			{#if card.unwatched_count > 0}
				<span class="count">{compact.format(card.unwatched_count)} new</span>
			{/if}
			<PlayOverlay position="corner" size="sm" label={`Play ${card.title} station`} />
		</div>
	</div>
	<span class="title">{card.title}</span>
	<span class="meta">{meta}</span>
</button>

<style>
	.station-card {
		display: grid;
		gap: 4px;
		width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: inherit;
		text-align: left;
		cursor: pointer;
		min-width: 0;
	}

	/* Two faint edges peek above the lead frame: a stack of videos. */
	.stack {
		position: relative;
		padding-top: 8px;
		margin-bottom: 6px;
	}
	.stack::before,
	.stack::after {
		content: '';
		position: absolute;
		left: 50%;
		translate: -50% 0;
		height: 12px;
		border-radius: 8px 8px 0 0;
		background: var(--bg-hover);
		border: 1px solid var(--border-strong);
		border-bottom: 0;
	}
	.stack::before {
		top: 0;
		width: 84%;
		opacity: 0.5;
	}
	.stack::after {
		top: 4px;
		width: 92%;
	}

	.art {
		position: relative;
		z-index: 1;
		aspect-ratio: 16 / 9;
		border-radius: 10px;
		overflow: hidden;
		background: var(--bg-raised);
		transition: transform var(--motion-fast);
	}
	.art :global(.lead) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.station-card:hover .art,
	.station-card:focus-visible .art {
		transform: translateY(-2px);
	}

	.count {
		position: absolute;
		left: 8px;
		bottom: 8px;
		padding: 2px 8px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.62);
		color: #fff;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.02em;
	}

	.title {
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.meta {
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
	.station-card:focus-visible {
		outline: none;
	}
	.station-card:focus-visible .art {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.station-card[aria-busy='true'] {
		opacity: 0.6;
	}
</style>
