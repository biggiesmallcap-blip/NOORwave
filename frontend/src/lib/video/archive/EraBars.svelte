<script lang="ts">
	import { formatCount } from '$lib/utils/format';
	import { eraLabel } from './lenses';
	import { decadeAt, decadeDomain, decadeRange, decadeTick, type DecadeCount } from './river';

	// The era lens as decade columns, one bar per decade, as tall as its
	// share of the songs (square root, so one big decade does not flatten the
	// rest). Drag across the bars to pick a span, click one to pick that
	// decade, click the picked decade again to let go. The decade labels
	// under the bars are buttons too (Shift extends), so it works from the
	// keyboard.

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

	const PLOT_PX = 92;

	let width = $state(0);
	let drag = $state<{ anchor: number; head: number } | null>(null);
	let hover = $state<number | null>(null);

	let domain = $derived(decadeDomain(facet, { from, to }));
	let max = $derived(Math.max(1, ...domain.map((d) => d.count)));
	let preview = $derived(drag ? decadeRange(drag.anchor, drag.head) : null);
	let hovered = $derived(hover == null ? null : (domain.find((d) => d.decade === hover) ?? null));
	let picking = $derived(from != null || to != null || preview != null);

	function barHeight(count: number): number {
		if (count <= 0) return 2;
		return 6 + Math.sqrt(count / max) * (PLOT_PX - 6);
	}

	function inSelection(decade: number): boolean {
		const lo = preview?.from ?? from;
		const hi = preview?.to ?? to;
		if (lo == null && hi == null) return false;
		return decade >= Math.floor((lo ?? -Infinity) / 10) * 10 && decade <= (hi ?? Infinity);
	}

	function xOf(event: PointerEvent): number {
		return event.clientX - (event.currentTarget as HTMLElement).getBoundingClientRect().left;
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
			const range = decadeRange(Math.min(Math.floor(from / 10) * 10, decade), Math.max(Math.floor(to / 10) * 10, decade));
			onchange(range.from, range.to);
			return;
		}
		const range = decadeRange(decade, decade);
		if (range.from === from && range.to === to) onchange(null, null);
		else onchange(range.from, range.to);
	}
</script>

{#if domain.length > 0}
	<div class="era">
		<div class="head">
			<span class="now">{preview ? eraLabel(preview.from, preview.to) : eraLabel(from, to)}</span>
			<span class="readout">
				{#if hovered}{hovered.decade}s, {formatCount(hovered.count)} {hovered.count === 1 ? 'song' : 'songs'}{:else}Drag across to pick a span{/if}
			</span>
		</div>
		<div
			class="plot"
			class:picking
			style:--cols={domain.length}
			style:height="{PLOT_PX}px"
			bind:clientWidth={width}
			role="presentation"
			onpointerdown={onPointerDown}
			onpointermove={onPointerMove}
			onpointerup={onPointerUp}
			onpointercancel={() => (drag = null)}
			onpointerleave={() => (hover = null)}
		>
			{#each domain as d (d.decade)}
				<span class="col" class:on={inSelection(d.decade)} class:hot={hover === d.decade}>
					<span class="bar" style:height="{barHeight(d.count)}px"></span>
				</span>
			{/each}
		</div>
		<div class="ticks" style:--cols={domain.length}>
			{#each domain as d, index (d.decade)}
				<button
					type="button"
					class="tick"
					class:on={inSelection(d.decade)}
					class:empty={d.count === 0}
					aria-pressed={inSelection(d.decade)}
					aria-label={`${d.decade}s, ${formatCount(d.count)} songs. Shift to extend.`}
					onclick={(event) => pickDecade(d.decade, event.shiftKey)}
				>{decadeTick(d.decade, index, domain.length)}</button>
			{/each}
		</div>
	</div>
{:else}
	<p class="empty">No song years yet</p>
{/if}

<style>
	.era {
		display: grid;
		gap: 8px;
		min-width: 0;
		user-select: none;
	}

	.head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 2px 2px 4px;
	}

	.now {
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
	}

	.readout {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.plot {
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		align-items: end;
		gap: 3px;
		cursor: pointer;
		touch-action: none;
	}

	.col {
		display: flex;
		align-items: flex-end;
		justify-content: center;
		height: 100%;
	}

	.bar {
		width: 100%;
		max-width: 34px;
		border-radius: 4px 4px 1px 1px;
		background: var(--text-tertiary);
		opacity: 0.32;
		transition:
			opacity var(--motion-fast),
			background var(--motion-fast);
	}

	.col.hot .bar {
		opacity: 0.6;
	}

	.picking .col:not(.on) .bar {
		opacity: 0.18;
	}

	.col.on .bar {
		background: var(--accent);
		opacity: 0.95;
	}

	.ticks {
		display: grid;
		grid-template-columns: repeat(var(--cols), minmax(0, 1fr));
		gap: 3px;
	}

	.tick {
		padding: 3px 0;
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

	.empty {
		margin: 0;
		padding: var(--space-3);
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}
</style>
