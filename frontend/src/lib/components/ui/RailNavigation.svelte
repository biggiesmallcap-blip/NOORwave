<script lang="ts">
	import { prefersReducedMotion } from '$lib/stores/motion';
	interface Props {
		rail: HTMLElement | null;
		label?: string;
	}

	let { rail, label = 'Shelf' }: Props = $props();
	let hasOverflow = $state(false);
	let canPrevious = $state(false);
	let canNext = $state(false);
	let controls = $state<HTMLElement | null>(null);
	// Vertical center of the first card's artwork, relative to the controls
	// box, so the arrows sit mid-picture instead of mid-card (art plus text).
	let artCenter = $state<number | null>(null);

	const ART = 'img, picture, [data-rail-art], .art, .artwork, .cover, .thumb';

	function measureArt() {
		const art = rail?.firstElementChild?.querySelector<HTMLElement>(ART);
		if (!art || !controls) {
			artCenter = null;
			return;
		}
		// Layout offsets, not client rects: cards rise in and lift on hover with
		// transforms, which must not move the arrows.
		const host = controls.offsetParent;
		let top = 0;
		let node: HTMLElement | null = art;
		while (node && node !== host) {
			top += node.offsetTop;
			node = node.offsetParent as HTMLElement | null;
		}
		if (!node || art.offsetHeight === 0) {
			artCenter = null;
			return;
		}
		artCenter = Math.round(top - controls.offsetTop + art.offsetHeight / 2);
	}

	function updateState() {
		if (!rail) {
			hasOverflow = false;
			canPrevious = false;
			canNext = false;
			return;
		}
		const max = Math.max(0, rail.scrollWidth - rail.clientWidth);
		hasOverflow = max > 2;
		canPrevious = rail.scrollLeft > 2;
		canNext = rail.scrollLeft < max - 2;
		if (hasOverflow) queueMicrotask(measureArt);
	}

	function move(direction: -1 | 1) {
		if (!rail) return;
		rail.scrollBy({
			left: direction * Math.max(160, rail.clientWidth * 0.85),
			behavior: prefersReducedMotion() ? 'auto' : 'smooth',
		});
	}

	$effect(() => {
		const node = rail;
		if (!node) return;

		const onScroll = () => updateState();
		const observer = typeof ResizeObserver === 'undefined'
			? null
			: new ResizeObserver(updateState);
		const mutationObserver = typeof MutationObserver === 'undefined'
			? null
			: new MutationObserver(updateState);
		node.addEventListener('scroll', onScroll, { passive: true });
		observer?.observe(node);
		for (const child of node.children) observer?.observe(child);
		mutationObserver?.observe(node, { childList: true });
		updateState();

		return () => {
			node.removeEventListener('scroll', onScroll);
			observer?.disconnect();
			mutationObserver?.disconnect();
		};
	});
</script>

{#if hasOverflow}
	<div
		class="rail-controls"
		class:art-aligned={artCenter !== null}
		style:--rail-art-center={artCenter === null ? undefined : `${artCenter}px`}
		role="group"
		aria-label={`${label} navigation`}
		bind:this={controls}
	>
		<button
			type="button"
			class="rail-control previous"
			onclick={() => move(-1)}
			disabled={!canPrevious}
			aria-label={`Show previous items in ${label}`}
		>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m14.5 5-7 7 7 7" /></svg>
		</button>
		<button
			type="button"
			class="rail-control next"
			onclick={() => move(1)}
			disabled={!canNext}
			aria-label={`Show next items in ${label}`}
		>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9.5 5 7 7-7 7" /></svg>
		</button>
	</div>
{/if}

<style>
	.rail-controls {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 0 4px;
		pointer-events: none;
		z-index: 4;
	}

	/* Arrows center on the first card's artwork when there is one. */
	.rail-controls.art-aligned {
		align-items: flex-start;
	}

	.rail-controls.art-aligned .rail-control {
		margin-top: calc(var(--rail-art-center) - 18px);
	}

	/* A solid themed disc with a full-strength chevron: readable over any
	   cover, the same in every shelf. */
	.rail-control {
		width: 36px;
		height: 36px;
		display: grid;
		place-items: center;
		padding: 0;
		border: 1px solid var(--border-strong);
		border-radius: 999px;
		background: var(--bg-surface-strong);
		color: var(--text-primary);
		box-shadow: 0 4px 16px rgba(0, 0, 0, 0.45);
		cursor: pointer;
		pointer-events: auto;
		transition: background var(--motion-fast), transform var(--motion-fast);
	}

	.rail-control:hover {
		background: color-mix(in srgb, var(--bg-surface-strong) 82%, var(--text-primary));
		transform: scale(1.06);
	}

	.rail-control:active {
		transform: scale(0.96);
	}

	.rail-control:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.rail-control:disabled {
		opacity: 0;
		pointer-events: none;
	}

	.rail-control svg {
		width: 20px;
		height: 20px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2.4;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	@media (prefers-reduced-motion: reduce) {
		.rail-control { transition: none; }
	}

	@media (max-width: 760px) {
		.rail-control {
			width: 44px;
			height: 44px;
		}

		.rail-controls.art-aligned .rail-control {
			margin-top: calc(var(--rail-art-center) - 22px);
		}
	}
</style>
