<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { ExploreVideo } from '$lib/api/video_explore_types';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import VideoCard from '$lib/components/video/VideoCard.svelte';
	import type { StageItem } from './stage';

	// One row under the watch page's player: a heading line (title, an
	// optional scene hue and a few quiet actions) and a rail of video cards,
	// each with an optional caption that says why it is here. Cards carry the
	// shared video menu (VideoCard), which also reaches the artist.

	let {
		title,
		meta = '',
		hue = null,
		items,
		onplay,
		actions,
	}: {
		title: string;
		meta?: string;
		hue?: string | null;
		items: StageItem[];
		onplay: (video: ExploreVideo) => void;
		actions?: Snippet;
	} = $props();

</script>

<section class="stage-row" aria-label={title}>
	<div class="row-head">
		<h2 class="t-section">
			{#if hue}<span class="hue" style:background={hue}></span>{/if}
			{title}
		</h2>
		{#if meta}<span class="t-meta">{meta}</span>{/if}
		{#if actions}<div class="row-actions">{@render actions()}</div>{/if}
	</div>
	<MediaRail {items} getKey={(item) => item.video.tidal_id} gap={14} padding="4px 2px 10px" ariaLabel={title}>
		{#snippet card(item)}
			<div class="stage-card" class:current={item.current}>
				<VideoCard video={item.video} onSelect={(picked) => !('id' in picked) && onplay(item.video)} />
				{#if item.current}
					<span class="caption now">Playing now</span>
				{:else if item.caption}
					<span class="caption">{item.caption}</span>
				{/if}
				{#if item.index != null}<span class="index" aria-hidden="true">{item.index}</span>{/if}
			</div>
		{/snippet}
	</MediaRail>
</section>

<style>
	.stage-row {
		display: grid;
		gap: var(--group-gap);
		min-width: 0;
	}

	.row-head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		flex-wrap: wrap;
		min-width: 0;
	}

	.row-head h2 {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		margin: 0;
		color: var(--text-primary);
	}

	.hue {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		flex: 0 0 auto;
	}

	.row-actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-left: auto;
	}

	.stage-card {
		position: relative;
		flex: 0 0 clamp(200px, 21%, 280px);
		display: grid;
		align-content: start;
		gap: 4px;
		min-width: 0;
		scroll-snap-align: start;
	}

	.caption {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.caption.now {
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold);
	}

	.stage-card.current :global(.poster-wrap) {
		box-shadow: 0 0 0 2px var(--accent);
	}

	/* Setlist position over the artwork's top-left corner: text over art
	   gets the dark scrim in every theme. */
	.index {
		position: absolute;
		top: 8px;
		left: 8px;
		min-width: 22px;
		padding: 3px 6px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.6);
		color: rgba(255, 255, 255, 0.92);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		font-variant-numeric: tabular-nums;
		line-height: 1;
		text-align: center;
		pointer-events: none;
	}
</style>
