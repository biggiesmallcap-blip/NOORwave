<script lang="ts">
	import type { TidalSearchVideo } from '$lib/api/client';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import VideoCard from '$lib/components/video/VideoCard.svelte';

	// One editorial set: its own headline, its blurb, and a horizontal rail of
	// picks. Rails rather than grids so several shelves fit on a browse page
	// without turning it into a scroll marathon.
	let {
		title,
		blurb,
		items,
		eyebrow = null,
		playLabel = 'Play all',
		onSelect,
		onPlayAll,
	}: {
		title: string;
		blurb: string;
		items: TidalSearchVideo[];
		eyebrow?: string | null;
		playLabel?: string;
		onSelect: (video: TidalSearchVideo, index: number) => void;
		onPlayAll?: () => void;
	} = $props();
</script>

<section class="set-shelf">
	<header class="set-head">
		<div class="set-titles">
			{#if eyebrow}<p class="eyebrow">{eyebrow}</p>{/if}
			<h2>{title}</h2>
			{#if blurb}<p class="set-blurb">{blurb}</p>{/if}
		</div>
		{#if onPlayAll}
			<button type="button" class="set-play" onclick={onPlayAll}>{playLabel}</button>
		{/if}
	</header>

	<MediaRail {items} getKey={(item) => item.tidal_id}>
		{#snippet card(item, index)}
			<div class="set-card">
				<VideoCard video={item} onSelect={() => onSelect(item, index)} />
			</div>
		{/snippet}
	</MediaRail>
</section>

<style>
	.set-shelf {
		display: grid;
		/* min-width: 0 so this shelf can shrink to its grid column; without it
		   the rail's cards set a min-content floor and the shelf (and page)
		   overflow instead of the rail scrolling. */
		grid-template-columns: minmax(0, 1fr);
		min-width: 0;
		gap: 10px;
	}

	.set-head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-4);
		padding: 0 2px;
	}

	.set-titles {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.set-head h2 {
		margin: 0;
		font-size: var(--font-size-lg);
	}

	.set-blurb {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		max-width: 68ch;
	}

	.set-play {
		flex: 0 0 auto;
		padding: var(--space-2) var(--space-4);
		border-radius: 999px;
		border: 1px solid var(--panel-border);
		background: var(--bg-hover);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast);
	}

	.set-play:hover,
	.set-play:focus-visible {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		outline: none;
	}

	.set-card {
		width: clamp(200px, 22vw, 260px);
	}

	@media (max-width: 620px) {
		.set-card {
			width: 64vw;
		}

		.set-blurb {
			font-size: var(--font-size-xs);
		}
	}
</style>
