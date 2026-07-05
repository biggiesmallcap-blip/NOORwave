<script lang="ts">
	import { discoverSpaceStore, setLens } from './discover_space_store';
	import { LENS_LABELS, REASON_LABELS } from './discover_space_story';
	import type { DiscoverLens, DiscoverReason } from './discover_space_types';

	const lenses: DiscoverLens[] = ['energy', 'reason', 'confidence', 'source', 'genre'];

	// The legend lives inside the lens control (one overlay instead of two);
	// its content follows the active lens.
	let showLegend = $state(false);

	const reasonColors: Record<DiscoverReason, string> = {
		harmonic: 'rgba(180,160,255,0.8)',
		behavioral: 'rgba(100,180,255,0.8)',
		bpm: 'rgba(255,220,80,0.8)',
		artist: 'rgba(255,160,80,0.8)',
		album: 'rgba(255,160,80,0.8)',
		genre: 'rgba(80,220,120,0.8)',
		energy: 'rgba(80,220,120,0.8)',
		external: 'rgba(120,100,220,0.8)',
		unknown: 'rgba(120,120,140,0.5)',
	};

	const sourceColors = {
		library: '#6080e0',
		lastfm: '#e04060',
		engine: '#60c080',
		mixed: '#a080c0',
	};
</script>

<div class="lens-wrap">
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
		<button
			class="lens-btn legend-toggle"
			class:active={showLegend}
			onclick={() => (showLegend = !showLegend)}
			aria-expanded={showLegend}
			aria-label="Toggle legend"
		>
			◈
		</button>
	</div>

	{#if showLegend}
		<div class="legend-body">
			<div class="legend-section">
				<div class="legend-heading">Lens: {LENS_LABELS[$discoverSpaceStore.lens]}</div>

				{#if $discoverSpaceStore.lens === 'reason' || $discoverSpaceStore.lens === 'energy'}
					<div class="legend-heading sub">Edge color = connection type</div>
					{#each (Object.keys(reasonColors) as DiscoverReason[]) as reason}
						<div class="legend-row">
							<span class="swatch" style:background={reasonColors[reason]}></span>
							<span>{REASON_LABELS[reason]}</span>
						</div>
					{/each}
				{/if}

				{#if $discoverSpaceStore.lens === 'source'}
					<div class="legend-heading sub">Node ring = source</div>
					{#each Object.entries(sourceColors) as [src, col]}
						<div class="legend-row">
							<span class="swatch" style:background={col}></span>
							<span>{src}</span>
						</div>
					{/each}
				{/if}

				{#if $discoverSpaceStore.lens === 'confidence'}
					<div class="legend-heading sub">Opacity = confidence</div>
					<div class="legend-row"><span class="swatch bright"></span><span>High confidence</span></div>
					<div class="legend-row"><span class="swatch dim"></span><span>Cold start</span></div>
				{/if}
			</div>

			<div class="legend-section">
				<div class="legend-heading">Nodes</div>
				<div class="legend-row"><span class="swatch seed"></span><span>Seed (Anchor Star)</span></div>
				<div class="legend-row"><span class="swatch playing"></span><span>Playing</span></div>
				<div class="legend-row"><span class="swatch playlist"></span><span>In playlist</span></div>
			</div>
		</div>
	{/if}
</div>

<style>
	.lens-wrap {
		display: flex;
		flex-direction: column;
		gap: 6px;
		align-items: flex-start;
	}
	.lens-control {
		display: flex;
		gap: 4px;
		background: rgba(0, 0, 0, 0.5);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		padding: 3px;
	}
	.lens-btn {
		padding: 4px 12px;
		border-radius: 999px;
		border: none;
		background: transparent;
		color: rgba(255, 255, 255, 0.5);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
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
	.legend-toggle {
		padding: 4px 8px;
	}
	.legend-body {
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: rgba(10, 10, 20, 0.85);
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		padding: 8px 12px;
		min-width: 160px;
		max-width: 200px;
		font-size: var(--font-size-xs);
	}
	.legend-section { display: flex; flex-direction: column; gap: 4px; }
	.legend-heading { color: rgba(255,255,255,0.4); text-transform: uppercase; letter-spacing: 0.08em; font-size: var(--font-size-2xs); margin-bottom: 2px; }
	.legend-heading.sub { font-size: var(--font-size-2xs); margin-top: 4px; }
	.legend-row { display: flex; align-items: center; gap: 6px; color: rgba(255,255,255,0.65); }
	.swatch {
		width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;
		background: rgba(120,120,140,0.5);
	}
	.swatch.seed { background: #5060e0; box-shadow: 0 0 6px #5060e0; }
	.swatch.playing { background: #9080e0; box-shadow: 0 0 6px #9080e0; }
	.swatch.playlist { background: rgba(255,200,50,0.9); box-shadow: 0 0 4px rgba(255,200,50,0.5); }
	.swatch.bright { background: rgba(200,200,255,0.9); }
	.swatch.dim { background: rgba(80,80,100,0.4); border: 1px solid rgba(140,140,160,0.3); }
</style>
