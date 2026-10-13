<script lang="ts">
	import type { ArchiveJump } from '$lib/api/video_explore_types';

	// The Archive's jump rail: letters for the A-Z sorts, decades for the
	// year sorts, down the right edge of the results. Press or drag along it
	// and a bubble shows where you are; let go to jump there. The section on
	// screen is marked. When the rail is too short for every label, the ones
	// in between become dots but can still be reached by dragging. From the
	// keyboard the rail is one tab stop: arrows move, Enter jumps.

	let {
		jumps,
		current,
		onjump,
	}: {
		jumps: readonly ArchiveJump[];
		/** Index into `jumps` of the section on screen, -1 when unknown. */
		current: number;
		onjump: (offset: number) => void;
	} = $props();

	const ITEM_PX = 17;

	let rail = $state<HTMLElement>();
	let height = $state(0);
	let scrub = $state<number | null>(null);
	let focusIndex = $state(-1);

	let step = $derived(Math.max(1, Math.ceil((jumps.length * ITEM_PX) / Math.max(1, height))));
	let active = $derived(scrub ?? (focusIndex >= 0 ? focusIndex : current));

	/** "1980s" -> "80s", except the first decade and the 2000s, which keep
	 *  their century so the rail reads unambiguously. */
	function short(label: string, index: number): string {
		if (!/^\d{4}s$/.test(label) || index === 0 || label.startsWith('20')) return label;
		return `${label.slice(2)}`;
	}

	function indexAt(clientY: number): number {
		if (!rail) return 0;
		const r = rail.getBoundingClientRect();
		const t = Math.min(0.9999, Math.max(0, (clientY - r.top) / Math.max(1, r.height)));
		return Math.floor(t * jumps.length);
	}

	function onPointerDown(event: PointerEvent) {
		if (event.button !== 0 || jumps.length === 0) return;
		event.preventDefault();
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		scrub = indexAt(event.clientY);
	}

	function onPointerMove(event: PointerEvent) {
		if (scrub === null) return;
		scrub = indexAt(event.clientY);
	}

	function onPointerUp() {
		if (scrub === null) return;
		const jump = jumps[scrub];
		scrub = null;
		if (jump) onjump(jump.offset);
	}

	function onKeydown(event: KeyboardEvent) {
		const from = focusIndex >= 0 ? focusIndex : Math.max(0, current);
		let next = from;
		if (event.key === 'ArrowDown') next = Math.min(jumps.length - 1, from + 1);
		else if (event.key === 'ArrowUp') next = Math.max(0, from - 1);
		else if (event.key === 'Home') next = 0;
		else if (event.key === 'End') next = jumps.length - 1;
		else if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			const jump = jumps[from];
			if (jump) onjump(jump.offset);
			return;
		} else return;
		event.preventDefault();
		event.stopPropagation();
		focusIndex = next;
	}

	let bubbleTop = $derived(scrub === null ? 0 : ((scrub + 0.5) / Math.max(1, jumps.length)) * height);
</script>

<nav class="scrubber" aria-label="Jump to a section">
	<div
		bind:this={rail}
		bind:clientHeight={height}
		class="rail"
		class:scrubbing={scrub !== null}
		role="slider"
		tabindex="0"
		aria-label="Jump to a section"
		aria-valuemin={0}
		aria-valuemax={Math.max(0, jumps.length - 1)}
		aria-valuenow={Math.max(0, active)}
		aria-valuetext={jumps[active]?.label ?? ''}
		onpointerdown={onPointerDown}
		onpointermove={onPointerMove}
		onpointerup={onPointerUp}
		onpointercancel={() => (scrub = null)}
		onkeydown={onKeydown}
		onblur={() => (focusIndex = -1)}
	>
		{#each jumps as jump, index (jump.label)}
			{@const shown = index % step === 0 || index === active}
			<span class="mark" class:on={index === active} class:dot={!shown} aria-hidden="true">
				{shown ? short(jump.label, index) : ''}
			</span>
		{/each}
	</div>
	{#if scrub !== null && jumps[scrub]}
		<span class="bubble" style:top={`${bubbleTop}px`} aria-hidden="true">{jumps[scrub].label}</span>
	{/if}
</nav>

<style>
	/* A slim rail on the ground at the right edge of the results; it stays
	   in view while the grid scrolls. */
	.scrubber {
		position: sticky;
		top: var(--space-4);
		align-self: start;
		height: min(640px, calc(100dvh - var(--bottom-player-height, 0px) - 160px));
		min-height: 200px;
		user-select: none;
	}

	.rail {
		display: flex;
		flex-direction: column;
		justify-content: space-between;
		align-items: center;
		height: 100%;
		padding: 6px 0;
		border-radius: 999px;
		cursor: pointer;
		touch-action: none;
		transition: background var(--motion-fast);
	}

	.rail:hover,
	.rail.scrubbing {
		background: var(--bg-hover);
	}

	.rail:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.mark {
		display: grid;
		place-items: center;
		min-height: 12px;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		font-variant-numeric: tabular-nums;
		line-height: 1;
		pointer-events: none;
		transition: color var(--motion-fast);
	}

	.mark.dot::before {
		content: '';
		width: 3px;
		height: 3px;
		border-radius: 50%;
		background: currentColor;
		opacity: 0.6;
	}

	.mark.on {
		color: var(--accent-strong);
	}

	.bubble {
		position: absolute;
		right: calc(100% + 14px);
		min-width: 64px;
		padding: 12px 18px;
		border-radius: var(--radius-md);
		background: var(--accent);
		color: var(--text-on-accent);
		font-size: var(--font-size-2xl);
		line-height: 1;
		font-weight: var(--font-weight-bold);
		text-align: center;
		transform: translateY(-50%);
		box-shadow: var(--panel-shadow);
		pointer-events: none;
	}
</style>
