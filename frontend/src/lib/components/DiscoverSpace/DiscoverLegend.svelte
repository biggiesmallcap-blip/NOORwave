<script lang="ts">
	import { discoverSpaceStore } from './discover_space_store';
	import { REASON_LABELS, LENS_LABELS } from './discover_space_story';
	import type { DiscoverReason } from './discover_space_types';

	let collapsed = $state(false);

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

<div class="legend" class:collapsed>
	<button class="legend-toggle" onclick={() => collapsed = !collapsed} aria-expanded={!collapsed}>
		◈ {collapsed ? 'Legend' : 'Legend ▾'}
	</button>

	{#if !collapsed}
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
	.legend {
		background: rgba(10, 10, 20, 0.85);
		backdrop-filter: blur(10px);
		border: 1px solid rgba(255, 255, 255, 0.08);
		border-radius: 10px;
		padding: 8px 12px;
		min-width: 160px;
		max-width: 200px;
		font-size: 0.72rem;
	}
	.legend-toggle {
		background: none;
		border: none;
		color: rgba(255, 255, 255, 0.6);
		cursor: pointer;
		padding: 0;
		font-size: 0.72rem;
		font-weight: 500;
	}
	.legend-toggle:hover { color: rgba(255,255,255,0.9); }
	.legend-body { margin-top: 8px; display: flex; flex-direction: column; gap: 10px; }
	.legend-section { display: flex; flex-direction: column; gap: 4px; }
	.legend-heading { color: rgba(255,255,255,0.4); text-transform: uppercase; letter-spacing: 0.08em; font-size: 0.65rem; margin-bottom: 2px; }
	.legend-heading.sub { font-size: 0.62rem; margin-top: 4px; }
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
