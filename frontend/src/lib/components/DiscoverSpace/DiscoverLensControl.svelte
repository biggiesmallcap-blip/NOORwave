<script lang="ts">
	import { discoverSpaceStore, setLens } from './discover_space_store';
	import { LENS_LABELS } from './discover_space_story';
	import type { DiscoverLens } from './discover_space_types';

	const lenses: DiscoverLens[] = ['energy', 'reason', 'confidence', 'source', 'genre'];
</script>

<div class="lens-control" role="group" aria-label="Visual lens">
	{#each lenses as lens}
		<button
			class="lens-btn"
			class:active={$discoverSpaceStore.lens === lens}
			onclick={() => setLens(lens)}
			aria-pressed={$discoverSpaceStore.lens === lens}
		>
			{LENS_LABELS[lens]}
		</button>
	{/each}
</div>

<style>
	.lens-control {
		display: flex;
		gap: 4px;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: blur(8px);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 999px;
		padding: 3px;
	}
	.lens-btn {
		padding: 4px 12px;
		border-radius: 999px;
		border: none;
		background: transparent;
		color: rgba(255, 255, 255, 0.5);
		font-size: 0.72rem;
		font-weight: 500;
		cursor: pointer;
		transition: background 0.12s, color 0.12s;
	}
	.lens-btn:hover {
		color: rgba(255, 255, 255, 0.85);
	}
	.lens-btn.active {
		background: rgba(124, 128, 255, 0.25);
		color: rgba(255, 255, 255, 0.95);
	}
</style>
