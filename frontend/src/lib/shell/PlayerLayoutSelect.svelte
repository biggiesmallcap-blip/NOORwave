<script lang="ts">
	import { playerPlacement, type EffectivePlayerLayout, type PlayerPlacement } from '$lib/stores/playerLayout';

	let { effective } = $props<{ effective: EffectivePlayerLayout }>();

	let fallback = $derived(effective !== 'mobile' && effective !== $playerPlacement);
	let positionLabel = $derived(effective === 'bottom' ? 'bottom' : effective === 'left' ? 'left' : 'right');
</script>

<label
	class="player-layout-select"
	title={fallback ? `Player at the bottom until this window is wider. Preferred position: ${$playerPlacement}.` : `Player position: ${positionLabel}`}
>
	<svg viewBox="0 0 20 20" aria-hidden="true" focusable="false">
		<rect x="2.5" y="3" width="15" height="14" rx="2" />
		{#if effective === 'bottom'}
			<path d="M2.5 12h15" />
		{:else if effective === 'left'}
			<path d="M8 3v14" />
		{:else}
			<path d="M12 3v14" />
		{/if}
	</svg>
	<select
		aria-label="Player position"
		value={$playerPlacement}
		onchange={(event) => playerPlacement.set((event.currentTarget as HTMLSelectElement).value as PlayerPlacement)}
	>
		<option value="right">Player right</option>
		<option value="left">Player left</option>
		<option value="bottom">Player bottom</option>
	</select>
	{#if fallback}<span class="fallback-dot" aria-hidden="true"></span>{/if}
</label>

<style>
	.player-layout-select {
		position: relative;
		display: inline-grid;
		place-items: center;
		flex: none;
		width: 32px;
		height: 32px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-tertiary);
		cursor: pointer;
	}

	.player-layout-select:hover,
	.player-layout-select:focus-within {
		color: var(--text-primary);
		border-color: var(--border-subtle);
		background: var(--bg-hover);
	}

	.player-layout-select:focus-within {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	svg {
		width: 17px;
		height: 17px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.4;
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
		position: absolute;
		right: 2px;
		bottom: 2px;
		width: 4px;
		height: 4px;
		border-radius: 50%;
		background: var(--accent);
		pointer-events: none;
	}
</style>
