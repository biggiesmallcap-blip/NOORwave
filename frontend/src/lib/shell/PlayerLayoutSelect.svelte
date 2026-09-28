<script lang="ts">
	import { playerPlacement, type EffectivePlayerLayout, type PlayerPlacement } from '$lib/stores/playerLayout';

	let { effective, compact = false } = $props<{
		effective: EffectivePlayerLayout;
		compact?: boolean;
	}>();

	let fallback = $derived(effective !== 'mobile' && effective !== $playerPlacement);
</script>

<label class="player-layout-select" class:compact title={fallback ? 'Using the bottom player until this window is wider' : 'Player position'}>
	<svg viewBox="0 0 20 20" aria-hidden="true" focusable="false">
		<rect x="2" y="3" width="16" height="14" rx="2" />
		<path d="M6 3v14M14 3v14" />
	</svg>
	<span>Layout</span>
	<select
		aria-label="Player position"
		value={$playerPlacement}
		onchange={(event) => playerPlacement.set((event.currentTarget as HTMLSelectElement).value as PlayerPlacement)}
	>
		<option value="right">Player right</option>
		<option value="left">Player left</option>
		<option value="bottom">Player bottom</option>
	</select>
	{#if fallback}<span class="fallback-dot" aria-label="Temporarily using bottom layout"></span>{/if}
</label>

<style>
	.player-layout-select {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: 5px;
		min-height: 40px;
		padding: 0 10px;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		white-space: nowrap;
		cursor: pointer;
	}

	.player-layout-select:hover,
	.player-layout-select:focus-within {
		color: var(--text-primary);
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.player-layout-select:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	svg {
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
	}

	select {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		opacity: 0;
		cursor: pointer;
	}

	.fallback-dot {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--accent);
	}

	.compact {
		min-width: 40px;
		padding: 0 8px;
		justify-content: center;
	}

	.compact > span:not(.fallback-dot) { display: none; }
</style>
