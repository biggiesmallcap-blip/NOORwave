<script lang="ts">
	import { discoverSpaceStore, setLens } from './discover_space_store';
	import { LENS_LABELS, REASON_LABELS, SOURCE_LABELS } from './discover_space_story';
	import { EDGE_COLORS, SOURCE_RING, energyColor } from './discover_space_renderer';
	import { nodeFamily, UNKNOWN_FILL, type GenreFamily } from './discover_space_genre';
	import type { DiscoverLens, DiscoverReason } from './discover_space_types';

	const lenses: DiscoverLens[] = ['energy', 'reason', 'confidence', 'source', 'genre'];

	// The legend lives inside the lens control (one overlay instead of two);
	// its key follows the active lens and uses the renderer's own colours.
	let showLegend = $state(false);

	const reasonOrder: DiscoverReason[] = ['similar', 'behavioral', 'harmonic', 'bpm', 'artist', 'genre', 'energy', 'external', 'unknown'];
	const sourceOrder = ['library', 'lastfm', 'engine', 'external'] as const;

	// The key lists only what is on the map right now, so it stays short.
	let presentReasons = $derived.by(() => {
		const seen = new Set($discoverSpaceStore.nodes.filter((n) => !n.isSeed).map((n) => n.primaryReason));
		return reasonOrder.filter((r) => seen.has(r));
	});
	let presentSources = $derived.by(() => {
		const seen = new Set<string>($discoverSpaceStore.nodes.filter((n) => !n.isSeed).map((n) => n.source));
		return sourceOrder.filter((s) => seen.has(s));
	});
	const energyGradient = `linear-gradient(90deg, ${energyColor(0)}, ${energyColor(0.5)}, ${energyColor(1)})`;

	// Genre key: only the families on the map right now, biggest first.
	let families = $derived.by(() => {
		const counts = new Map<string, { family: GenreFamily; n: number }>();
		for (const node of $discoverSpaceStore.nodes) {
			const family = nodeFamily(node);
			if (!family) continue;
			const entry = counts.get(family.key);
			if (entry) entry.n++;
			else counts.set(family.key, { family, n: 1 });
		}
		return [...counts.values()].sort((a, b) => b.n - a.n).slice(0, 8).map((e) => e.family);
	});
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
			<p class="legend-encoding">Closer ranks higher. Bigger is surer. Direction is genre.</p>

			<div class="legend-section" class:grid={$discoverSpaceStore.lens !== 'energy' && $discoverSpaceStore.lens !== 'confidence'}>
				<div class="legend-heading">Color: {LENS_LABELS[$discoverSpaceStore.lens]}</div>
				{#if $discoverSpaceStore.lens === 'energy'}
					<div class="scale" style:background={energyGradient}></div>
					<div class="scale-labels"><span>Calm</span><span>Intense</span></div>
					<div class="legend-row"><span class="swatch" style:background={UNKNOWN_FILL}></span><span>No audio analysis yet</span></div>
				{:else if $discoverSpaceStore.lens === 'reason'}
					{#each presentReasons as reason}
						<div class="legend-row">
							<span class="swatch" style:background={EDGE_COLORS[reason]}></span>
							<span>{REASON_LABELS[reason]}</span>
						</div>
					{/each}
				{:else if $discoverSpaceStore.lens === 'source'}
					{#each presentSources as src}
						<div class="legend-row">
							<span class="swatch" style:background={SOURCE_RING[src]}></span>
							<span>{SOURCE_LABELS[src]}</span>
						</div>
					{/each}
				{:else if $discoverSpaceStore.lens === 'confidence'}
					<div class="legend-row"><span class="swatch bright"></span><span>Well supported</span></div>
					<div class="legend-row"><span class="swatch dim"></span><span>Cold start</span></div>
				{:else}
					{#each families as family (family.key)}
						<div class="legend-row">
							<span class="swatch" style:background={family.color}></span>
							<span>{family.label}</span>
						</div>
					{/each}
					<div class="legend-row"><span class="swatch" style:background={UNKNOWN_FILL}></span><span>No genre yet</span></div>
				{/if}
			</div>

			<div class="legend-inline">
				<span class="legend-row"><span class="swatch seed"></span>Anchor</span>
				<span class="legend-row"><span class="swatch playing"></span>Playing</span>
				<span class="legend-row"><span class="swatch playlist"></span>Playlist</span>
			</div>
		</div>
	{/if}
</div>

<style>
	/* The legend drops over the stage instead of growing the toolbar, so the
	   filter bar beside it stays at the top. */
	.lens-wrap {
		position: relative;
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
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		z-index: 12;
		display: flex;
		flex-direction: column;
		gap: 8px;
		background: color-mix(in srgb, var(--bg-surface-strong) 90%, transparent);
		backdrop-filter: var(--blur-overlay);
		-webkit-backdrop-filter: var(--blur-overlay);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		padding: 8px 12px;
		min-width: 220px;
		max-width: 290px;
		font-size: var(--font-size-xs);
	}
	.legend-section { display: flex; flex-direction: column; gap: 4px; }
	.legend-section.grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px 10px; }
	.legend-section.grid .legend-heading { grid-column: 1 / -1; }
	.legend-section.grid .legend-row span:last-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.legend-heading { color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.08em; font-size: var(--font-size-2xs); margin-bottom: 2px; }
	.legend-row { display: flex; align-items: center; gap: 6px; color: var(--text-secondary); }
	.legend-encoding { margin: 0; color: var(--text-tertiary); font-size: var(--font-size-2xs); line-height: var(--line-height-snug); }
	.legend-inline { display: flex; flex-wrap: wrap; gap: 4px 10px; padding-top: 6px; border-top: 1px solid var(--border-subtle); }
	.scale { height: 8px; border-radius: 999px; }
	.scale-labels { display: flex; justify-content: space-between; color: var(--text-tertiary); font-size: var(--font-size-2xs); }
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
