<script lang="ts" generics="T">
	import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal';
	import type { Snippet } from 'svelte';

	// Generic horizontal rail with wheel-to-horizontal scrolling + slim
	// scrollbar styling. The caller supplies the card markup via the `card`
	// snippet so the rail stays content-agnostic — used for albums, videos,
	// similar-artists, etc. on /artists/[id] and /albums/[id], and for the
	// home shelves.
	//
	// `fluid` and `stagger` are opt-in. Callers that predate them keep their
	// own card widths and no entrance animation, unchanged.
	let {
		items,
		card,
		getKey,
		gap = 16,
		padding = '4px 2px 12px',
		fluid = false,
		density = 'tile',
		stagger = false,
		ariaLabel = undefined,
	}: {
		items: T[];
		card: Snippet<[item: T, index: number]>;
		getKey: (item: T, index: number) => string | number;
		gap?: number;
		padding?: string;
		/** Size cards from the rail's own width so a whole number always fits.
		 *  See the `--cols` comment in the style block. */
		fluid?: boolean;
		/** Which fluid ladder to use. `tile` suits square artwork; `wide` is for
		 *  text cards, which need roughly twice the width to stay readable. */
		density?: 'tile' | 'wide';
		/** Cascade the cards in on mount via the shared `.rise-in-card`. */
		stagger?: boolean;
		ariaLabel?: string;
	} = $props();

	// Roughly a screenful of cards. Uncapped, the last card in a long rail
	// waits seconds for its turn; capped, a rail that is scrolled or refilled
	// cascades the same way the first screen did.
	const STAGGER_CAP = 12;
</script>

{#if items.length > 0}
	<div class="media-rail-viewport">
		<div
			class="media-rail"
			class:fluid
			class:wide={fluid && density === 'wide'}
			role={ariaLabel ? 'group' : undefined}
			aria-label={ariaLabel}
			style="--rail-gap: {gap}px; --rail-padding: {padding};"
			use:wheelToHorizontal
		>
			{#each items as item, idx (getKey(item, idx))}
				{#if stagger}
					<div class="rail-slot rise-in-card" style={`--rise-index: ${idx % STAGGER_CAP}`}>
						{@render card(item, idx)}
					</div>
				{:else}
					{@render card(item, idx)}
				{/if}
			{/each}
		</div>
	</div>
{/if}

<style>
	/* The viewport exists purely to be the container-query container. The rail
	   itself cannot be: it is the scrolling box, so querying it would measure
	   its own content. */
	.media-rail-viewport {
		container-type: inline-size;
		min-width: 0;
	}

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
		mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
		-webkit-mask-image: linear-gradient(
			to right,
			transparent 0,
			black 16px,
			black calc(100% - 32px),
			transparent 100%
		);
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

	/* ── Fluid sizing ──────────────────────────────────────────────────────
	   Cards used to be pinned at a fixed px width. The rail is as wide as the
	   content column, which is itself a clamp of the viewport, so the number of
	   cards that fit was whatever that width happened to divide into and the
	   remainder was a card clipped at an arbitrary fraction. It looked
	   different, and equally accidental, at every window size.

	   Instead, derive the card width from the rail: `--cols` whole cards plus a
	   deliberate 0.35 of one more. The partial card is the scroll affordance and
	   is now identical everywhere rather than a leftover. `--cols` steps up on
	   the rail's own width, not the viewport's, so a rail in a narrow column
	   behaves like a narrow rail. */
	.media-rail.fluid {
		--cols: 3;
		--peek: 0.35;
	}

	.media-rail.fluid > :global(*) {
		flex: 0 0
			calc(
				(100% - (var(--cols) + var(--peek) - 1) * var(--rail-gap)) /
					(var(--cols) + var(--peek))
			);
		min-width: 0;
		scroll-snap-align: start;
	}

	@container (min-width: 560px) {
		.media-rail.fluid { --cols: 4; }
	}
	@container (min-width: 760px) {
		.media-rail.fluid { --cols: 5; }
	}
	@container (min-width: 980px) {
		.media-rail.fluid { --cols: 6; }
	}
	@container (min-width: 1200px) {
		.media-rail.fluid { --cols: 7; }
	}
	@container (min-width: 1440px) {
		.media-rail.fluid { --cols: 8; }
	}
	@container (min-width: 1700px) {
		.media-rail.fluid { --cols: 9; }
	}
	@container (min-width: 2000px) {
		.media-rail.fluid { --cols: 10; }
	}

	/* Text cards need roughly twice the width of a square tile before the title
	   and description stop wrapping into slivers, so `wide` runs its own, much
	   shallower ladder rather than scaling the tile one. */
	.media-rail.fluid.wide { --cols: 1; }
	@container (min-width: 560px) {
		.media-rail.fluid.wide { --cols: 2; }
	}
	@container (min-width: 980px) {
		.media-rail.fluid.wide { --cols: 3; }
	}
	@container (min-width: 1440px) {
		.media-rail.fluid.wide { --cols: 4; }
	}
	@container (min-width: 2000px) {
		.media-rail.fluid.wide { --cols: 5; }
	}

	/* The stagger wrapper must not become the layout box in fluid mode — the
	   card inside it needs the full slot width. */
	.rail-slot {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.rail-slot > :global(*) {
		width: 100%;
		min-width: 0;
	}
</style>
