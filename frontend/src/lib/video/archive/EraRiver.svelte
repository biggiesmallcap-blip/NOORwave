<script lang="ts">
	import { formatCount } from '$lib/utils/format';
	import {
		decadeAt,
		decadeDomain,
		decadeRange,
		decadeTick,
		riverPath,
		selectionSpan,
		type DecadeCount,
	} from './river';

	// The era lens: the decade facet as a small river. Drag across it to pick
	// a span of decades, click one decade, click the picked decade again to
	// let go. The decade labels under it are buttons too (Shift extends), so
	// the lens works from the keyboard.

	let {
		facet,
		from,
		to,
		onchange,
	}: {
		facet: readonly DecadeCount[];
		from: number | null;
		to: number | null;
		onchange: (from: number | null, to: number | null) => void;
	} = $props();

	const HEIGHT = 44;
	const uid = $props.id();
	const clipId = `era-river-${uid}`;

	let width = $state(0);
	let drag = $state<{ anchor: number; head: number } | null>(null);
	let hover = $state<number | null>(null);

	let domain = $derived(decadeDomain(facet, { from, to }));
	let path = $derived(riverPath(domain, width, HEIGHT));
	let preview = $derived(drag ? decadeRange(drag.anchor, drag.head) : null);
	let span = $derived(selectionSpan(domain, preview?.from ?? from, preview?.to ?? to));
	let hovered = $derived(hover == null ? null : domain.find((d) => d.decade === hover) ?? null);
	let maxCount = $derived(Math.max(1, ...domain.map((d) => d.count)));

	function xOf(event: PointerEvent): number {
		const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
		return event.clientX - rect.left;
	}

	function onPointerDown(event: PointerEvent) {
		if (event.button !== 0) return;
		const decade = decadeAt(domain, width, xOf(event));
		if (decade == null) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		drag = { anchor: decade, head: decade };
	}

	function onPointerMove(event: PointerEvent) {
		const decade = decadeAt(domain, width, xOf(event));
		hover = decade;
		if (drag && decade != null && decade !== drag.head) drag = { ...drag, head: decade };
	}

	function onPointerUp() {
		if (!drag) return;
		const range = decadeRange(drag.anchor, drag.head);
		drag = null;
		if (range.from === from && range.to === to) onchange(null, null);
		else onchange(range.from, range.to);
	}

	function pickDecade(decade: number, extend: boolean) {
		if (extend && from != null && to != null) {
			const lo = Math.min(Math.floor(from / 10) * 10, decade);
			const hi = Math.max(Math.floor(to / 10) * 10, decade);
			const range = decadeRange(lo, hi);
			onchange(range.from, range.to);
			return;
		}
		const range = decadeRange(decade, decade);
		if (range.from === from && range.to === to) onchange(null, null);
		else onchange(range.from, range.to);
	}

	function inSelection(decade: number): boolean {
		if (from == null && to == null) return false;
		return decade >= Math.floor((from ?? -Infinity) / 10) * 10 && decade <= (to ?? Infinity);
	}
</script>

{#if domain.length > 0}
	<div class="river" class:has-selection={span !== null}>
		<div
			class="plot"
			bind:clientWidth={width}
			role="presentation"
			onpointerdown={onPointerDown}
			onpointermove={onPointerMove}
			onpointerup={onPointerUp}
			onpointercancel={() => (drag = null)}
			onpointerleave={() => (hover = null)}
		>
			{#if width > 0}
				<svg viewBox={`0 0 ${width} ${HEIGHT}`} width={width} height={HEIGHT} aria-hidden="true">
					<defs>
						<clipPath id={clipId}>
							{#if span}
								<rect x={span.start * width} y="0" width={(span.end - span.start) * width} height={HEIGHT} />
							{/if}
						</clipPath>
					</defs>
					<path class="water" d={path} />
					{#if span}
						<path class="water picked" d={path} clip-path={`url(#${clipId})`} />
						<line class="edge" x1={span.start * width} x2={span.start * width} y1="0" y2={HEIGHT} />
						<line class="edge" x1={span.end * width} x2={span.end * width} y1="0" y2={HEIGHT} />
					{/if}
				</svg>
			{/if}
			{#if hovered && !drag}
				<span class="readout t-micro" style:left={`${((domain.indexOf(hovered) + 0.5) / domain.length) * 100}%`}>
					{hovered.decade}s &middot; {formatCount(hovered.count)}
				</span>
			{/if}
		</div>
		<div class="ticks" style:--cols={domain.length}>
			{#each domain as d, index (d.decade)}
				<button
					type="button"
					class="tick"
					class:on={inSelection(d.decade)}
					class:empty={d.count === 0}
					aria-pressed={inSelection(d.decade)}
					title={`${d.decade}s: ${formatCount(d.count)} songs${d.count > 0 && d.count === maxCount ? ' (most)' : ''}. Shift-click to extend.`}
					onclick={(event) => pickDecade(d.decade, event.shiftKey)}
				>{decadeTick(d.decade, index, domain.length)}</button>
			{/each}
		</div>
	</div>
{:else}
	<p class="river-empty t-micro">No song years yet</p>
{/if}

<style>
	.river {
		display: grid;
		gap: 4px;
		min-width: 0;
		user-select: none;
	}

	.plot {
		position: relative;
		height: 44px;
		cursor: crosshair;
		touch-action: none;
	}

	svg {
		display: block;
		overflow: visible;
	}

	.water {
		fill: var(--text-tertiary);
		opacity: 0.4;
		transition: opacity var(--motion-fast);
	}

	.plot:hover .water {
		opacity: 0.5;
	}

	.has-selection .water:not(.picked) {
		opacity: 0.22;
	}

	.water.picked {
		fill: var(--accent);
		opacity: 0.9;
	}

	.edge {
		stroke: var(--accent-strong);
		stroke-width: 1;
		opacity: 0.6;
	}

	.readout {
		position: absolute;
		top: -2px;
		transform: translate(-50%, -100%);
		padding: 2px 6px;
		border-radius: 999px;
		background: var(--bg-elevated);
		color: var(--text-secondary);
		white-space: nowrap;
		pointer-events: none;
		box-shadow: var(--panel-shadow);
	}

	.ticks {
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
	}

	.tick {
		padding: 2px 0;
		border: 0;
		border-radius: var(--radius-xs);
		background: none;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		font-variant-numeric: tabular-nums;
		line-height: 1;
		white-space: nowrap;
		overflow: hidden;
		cursor: pointer;
		transition: color var(--motion-fast), background var(--motion-fast);
	}

	.tick:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.tick.empty {
		color: var(--text-muted);
	}

	.tick.on {
		color: var(--accent-strong);
		font-weight: var(--font-weight-semibold);
	}

	.tick:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.river-empty {
		margin: 0;
		align-self: center;
	}
</style>
