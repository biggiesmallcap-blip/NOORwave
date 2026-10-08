<script lang="ts">
	import type { Snippet } from 'svelte';
	import ArtworkImage from './ArtworkImage.svelte';

	let {
		title,
		eyebrow = '',
		artwork = null,
		backdrop = null,
		fallbackText = 'NOOR',
		variant = 'standard',
		shape = 'square',
		align = 'end',
		label,
		oncontextmenu,
		cover,
		titleContent,
		meta,
		details,
		actions,
	}: {
		title: string;
		eyebrow?: string;
		artwork?: string | string[] | null;
		backdrop?: string | string[] | null;
		fallbackText?: string;
		variant?: 'standard' | 'immersive' | 'text';
		shape?: 'square' | 'round';
		align?: 'start' | 'end';
		label?: string;
		oncontextmenu?: (event: MouseEvent) => void;
		cover?: Snippet;
		titleContent?: Snippet;
		meta?: Snippet;
		details?: Snippet;
		actions?: Snippet;
	} = $props();

	const backdropSource = $derived(backdrop ?? artwork);
</script>

<header
	class="detail-hero"
	class:immersive={variant === 'immersive'}
	class:text-only={variant === 'text'}
	class:round={shape === 'round'}
	class:top-aligned={align === 'start'}
	role="group"
	aria-label={label ?? `${title} header`}
	oncontextmenu={oncontextmenu}
>
	{#if variant !== 'text' && backdropSource}
		<div class="backdrop" aria-hidden="true">
			<ArtworkImage
				className="backdrop-art"
				src={backdropSource}
				size={1280}
				decorative
				loading="eager"
				fetchPriority="high"
			/>
		</div>
	{/if}

	<div class="inner">
		{#if variant !== 'text'}
			<div class="cover" class:custom={cover != null}>
				{#if cover}
					{@render cover()}
				{:else}
					<ArtworkImage
						className="cover-art"
						src={artwork}
						alt={title}
						size={640}
						fallbackText={fallbackText}
						tint
						loading="eager"
						fetchPriority="high"
					/>
				{/if}
			</div>
		{/if}

		<div class="copy">
			{#if eyebrow}
				<p class="eyebrow">{eyebrow}</p>
			{/if}
			{#if titleContent}
				<div class="title-slot">{@render titleContent()}</div>
			{:else}
				<!-- Long names (classical works, deluxe editions) step down a size so
				     they do not take two 56px lines. -->
				<h1 class:long={title.length > 40} class:very-long={title.length > 70}>{title}</h1>
			{/if}
			{#if meta}
				<div class="meta">{@render meta()}</div>
			{/if}
			{#if details}
				<div class="details">{@render details()}</div>
			{/if}
			{#if actions}
				<div class="actions">{@render actions()}</div>
			{/if}
		</div>
	</div>
</header>

<style>
	/* Not a box (STYLING.md "Detail header"): the backdrop is decoration
	   behind the hero and fades into the ground on every side. The container
	   lets ActionBar fold its labels by the hero's width. */
	.detail-hero {
		container: detail-hero / inline-size;
		position: relative;
		isolation: isolate;
		overflow: hidden;
		padding: var(--space-4) 0;
	}

	.detail-hero.immersive {
		min-height: 300px;
	}

	.detail-hero.text-only {
		overflow: visible;
		padding: var(--space-3) 0 0;
	}

	.backdrop {
		position: absolute;
		inset: -4rem;
		z-index: -2;
		opacity: 0.32;
		mask-image: linear-gradient(90deg, transparent, #000 18%, #000 82%, transparent);
	}

	.backdrop::after {
		position: absolute;
		inset: 0;
		background: linear-gradient(
			180deg,
			color-mix(in srgb, var(--bg-base) 35%, transparent),
			color-mix(in srgb, var(--bg-base) 74%, transparent) 68%,
			var(--bg-base)
		);
		content: '';
	}

	.backdrop :global(.backdrop-art) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
		filter: var(--art-backdrop-filter);
		transform: scale(1.16);
	}

	.inner {
		display: flex;
		align-items: flex-end;
		gap: var(--space-5);
		width: 100%;
	}

	.text-only .inner {
		align-items: flex-start;
	}

	.top-aligned .inner {
		align-items: flex-start;
	}

	.cover {
		display: grid;
		flex: none;
		width: clamp(7rem, 14vw, 11rem);
		aspect-ratio: 1 / 1;
		place-items: center;
		overflow: hidden;
		border-radius: var(--radius-md);
		background: var(--bg-raised);
		box-shadow: 0 18px 40px -16px rgba(0, 0, 0, 0.7);
	}

	.immersive .cover {
		width: clamp(10rem, 16vw, 15rem);
	}

	.round .cover {
		border-radius: 50%;
	}

	.top-aligned .cover {
		align-self: flex-start;
	}

	.cover :global(.cover-art),
	.cover :global(img),
	.cover :global(.fallback) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.cover :global(.fallback) {
		display: grid;
		place-items: center;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
	}

	.copy {
		display: flex;
		flex: 1;
		min-width: 0;
		max-width: 72rem;
		flex-direction: column;
		gap: var(--space-2);
	}

	.eyebrow {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.14em;
		line-height: var(--line-height-snug);
		text-transform: uppercase;
	}

	h1,
	.title-slot :global(h1) {
		margin: 0;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		overflow-wrap: anywhere;
	}

	.immersive h1,
	.immersive .title-slot :global(h1) {
		font-size: var(--font-size-4xl);
	}

	h1.long,
	.immersive h1.long {
		font-size: var(--font-size-2xl);
	}

	h1.very-long,
	.immersive h1.very-long {
		font-size: var(--font-size-xl);
	}

	.meta,
	.actions {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2);
	}

	.meta {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.details {
		color: var(--text-secondary);
	}

	.actions {
		margin-top: var(--space-1);
	}

	@media (max-width: 760px) {
		.inner {
			flex-direction: column;
			align-items: flex-start;
			gap: var(--space-4);
		}

		.immersive .cover {
			width: 11.25rem;
		}

		.immersive h1,
		.immersive .title-slot :global(h1) {
			font-size: var(--font-size-2xl);
		}
	}
</style>
