<script lang="ts" generics="T">
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import type { Snippet } from 'svelte';

	// Generic horizontal rail with wheel-to-horizontal scrolling + slim
	// scrollbar styling. The caller supplies the card markup via the `card`
	// snippet so the rail stays content-agnostic — used for albums, videos,
	// similar-artists, etc. on /artists/[id] and /albums/[id].
	let {
		items,
		card,
		getKey,
		gap = 16,
		padding = '4px 2px 12px',
	}: {
		items: T[];
		card: Snippet<[item: T, index: number]>;
		getKey: (item: T, index: number) => string | number;
		gap?: number;
		padding?: string;
	} = $props();
</script>

{#if items.length > 0}
	<div
		class="media-rail"
		style="--rail-gap: {gap}px; --rail-padding: {padding};"
		use:wheelToHorizontal
	>
		{#each items as item, idx (getKey(item, idx))}
			{@render card(item, idx)}
		{/each}
	</div>
{/if}

<style>
	.media-rail {
		display: flex;
		gap: var(--rail-gap);
		overflow-x: auto;
		overflow-y: hidden;
		padding: var(--rail-padding);
		scroll-snap-type: x proximity;
		/* Firefox + most browsers — slim track, semi-transparent thumb. */
		scrollbar-width: thin;
		scrollbar-color: rgba(255, 255, 255, 0.18) transparent;
	}
	.media-rail::-webkit-scrollbar {
		height: 6px;
	}
	.media-rail::-webkit-scrollbar-track {
		background: transparent;
	}
	.media-rail::-webkit-scrollbar-thumb {
		background: rgba(255, 255, 255, 0.18);
		border-radius: 3px;
	}
	.media-rail::-webkit-scrollbar-thumb:hover {
		background: rgba(255, 255, 255, 0.32);
	}
</style>
