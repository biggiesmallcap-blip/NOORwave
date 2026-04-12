<script lang="ts">
	import type { DiscoverViewMode } from './discover.types';

	let {
		mode = 'radio',
		onModeChange = (_m: DiscoverViewMode) => {}
	}: {
		mode?: DiscoverViewMode;
		onModeChange?: (m: DiscoverViewMode) => void;
	} = $props();

	const modes: { value: DiscoverViewMode; label: string; icon: string }[] = [
		{ value: 'radio', label: 'Radio', icon: '◎' },
		{ value: 'explore', label: 'Explore', icon: '◉' },
		{ value: 'harmonic', label: 'Harmonic', icon: '♫' },
		{ value: 'energy_arc', label: 'Energy Arc', icon: '⚡' },
		{ value: 'samples', label: 'Samples', icon: '◇' },
	];
</script>

<div class="discover-filters glass-panel">
	{#each modes as m}
		<button
			class="mode-btn"
			class:active={mode === m.value}
			onclick={() => onModeChange(m.value)}
		>
			<span class="mode-icon">{m.icon}</span>
			<span>{m.label}</span>
		</button>
	{/each}
</div>

<style>
	.discover-filters { padding: 8px; display: flex; flex-direction: column; gap: 4px; }
	.mode-btn {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 8px 12px;
		border: none;
		background: transparent;
		color: var(--text-secondary, rgba(255,255,255,0.6));
		border-radius: 8px;
		cursor: pointer;
		font-size: 0.85rem;
	}
	.mode-btn:hover {
		background: rgba(255,255,255,0.05);
	}
	.mode-btn.active {
		background: rgba(124,128,255,0.15);
		color: var(--text-primary, rgba(255,255,255,0.95));
	}
	.mode-icon { font-size: 1.2rem; }
</style>
