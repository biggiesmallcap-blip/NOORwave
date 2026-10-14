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
			title="What the colours mean"
		>
			<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6" /><path d="M8 7.2v3.6M8 5.1v.1" /></svg>
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
		background: color-mix(in srgb, var(--bg-base) 72%, transparent);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		padding: 3px;
	}
	.lens-btn {
		padding: 4px 10px;
		border-radius: 999px;
		border: none;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast);
	}
	.lens-btn:hover {
		color: var(--text-primary);
	}
	.lens-btn.active {
		background: var(--accent-line);
		color: var(--text-primary);
	}
	.legend-toggle {
		display: inline-grid;
		place-items: center;
		padding: 4px 8px;
	}
	.legend-toggle svg { width: 14px; height: 14px; fill: none; stroke: currentColor; stroke-width: 1.5; stroke-linecap: round; }
	.legend-body {
		display: flex;
		flex-direction: column;
		gap: 10px;
		background: color-mix(in srgb, var(--bg-surface-strong) 90%, transparent);
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
	.legend-heading { color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em; font-size: var(--font-size-2xs); margin-bottom: 2px; }
	.legend-heading.sub { font-size: var(--font-size-2xs); margin-top: 4px; }
	.legend-row { display: flex; align-items: center; gap: 6px; color: var(--text-secondary); }
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
