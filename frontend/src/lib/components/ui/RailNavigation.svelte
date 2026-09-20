<script lang="ts">
	interface Props {
		rail: HTMLElement | null;
		label?: string;
	}

	let { rail, label = 'Shelf' }: Props = $props();
	let hasOverflow = $state(false);
	let canPrevious = $state(false);
	let canNext = $state(false);

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
	}

	function move(direction: -1 | 1) {
		if (!rail) return;
		rail.scrollBy({
			left: direction * Math.max(160, rail.clientWidth * 0.85),
			behavior: typeof matchMedia === 'function'
				&& matchMedia('(prefers-reduced-motion: reduce)').matches
				? 'auto'
				: 'smooth',
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
	<div class="rail-controls" role="group" aria-label={`${label} navigation`}>
		<button
			type="button"
			class="btn btn-glass rail-control previous"
			onclick={() => move(-1)}
			disabled={!canPrevious}
			aria-label={`Show previous items in ${label}`}
		>
			<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m14.5 5-7 7 7 7" /></svg>
		</button>
		<button
			type="button"
			class="btn btn-glass rail-control next"
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

	.rail-control {
		width: 40px;
		height: 40px;
		display: grid;
		place-items: center;
		padding: 0;
		border-radius: 999px;
		box-shadow: var(--panel-shadow);
		backdrop-filter: blur(10px);
		-webkit-backdrop-filter: blur(10px);
		cursor: pointer;
		pointer-events: auto;
	}

	.rail-control:focus-visible {
		outline: 2px solid var(--accent);
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
		stroke-width: 2;
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
	}
</style>
