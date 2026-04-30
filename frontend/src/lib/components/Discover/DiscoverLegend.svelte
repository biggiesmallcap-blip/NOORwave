<script lang="ts">
	const STORAGE_KEY = 'discover-legend-collapsed';

	let collapsed = $state(false);

	$effect(() => {
		// Load on mount
		if (typeof localStorage !== 'undefined') {
			const stored = localStorage.getItem(STORAGE_KEY);
			collapsed = stored === '1';
		}
	});

	function toggle() {
		collapsed = !collapsed;
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(STORAGE_KEY, collapsed ? '1' : '0');
		}
	}

	const edgeRows = [
		{ label: 'harmonic', color: '#a89cff' },
		{ label: 'co-listen / behavioural', color: '#5fb1ff' },
		{ label: 'BPM match', color: '#ffc857' },
		{ label: 'artist / album', color: '#ff8866' },
		{ label: 'genre / energy', color: '#9fcf80' },
		{ label: 'external (Tidal)', color: '#5b4ef8' },
	];
</script>

{#if collapsed}
	<button class="legend-pill" onclick={toggle} aria-label="Show legend">
		?
	</button>
{:else}
	<div class="legend-panel">
		<div class="legend-header">
			<span class="legend-title">Legend</span>
			<button class="legend-collapse" onclick={toggle} aria-label="Hide legend">⌃</button>
		</div>

		<div class="legend-section">
			<div class="section-label">NODES</div>
			<div class="energy-bar">
				<span class="energy-min">low energy</span>
				<div class="bar" aria-hidden="true"></div>
				<span class="energy-max">high</span>
			</div>
			<div class="encoding-row"><span class="dot-small"></span><span>size = similarity to seed</span></div>
			<div class="encoding-row"><span class="dot-glow"></span><span>glow = danceability</span></div>
		</div>

		<div class="legend-section">
			<div class="section-label">EDGES</div>
			{#each edgeRows as row}
				<div class="edge-row">
					<span class="edge-swatch" style="background:{row.color}"></span>
					<span class="edge-label">{row.label}</span>
				</div>
			{/each}

			<div class="strength-encoding">
				<div class="strength-label-row">
					<span>weak</span>
					<span>strong</span>
				</div>
				<svg class="strength-bar" viewBox="0 0 140 12" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
					<defs>
						<linearGradient id="nw-strength-alpha" x1="0%" y1="0%" x2="100%" y2="0%">
							<stop offset="0%" stop-color="#c0c0d8" stop-opacity="0.4" />
							<stop offset="100%" stop-color="#c0c0d8" stop-opacity="0.9" />
						</linearGradient>
					</defs>
					<!-- Tapered wedge: thin+faint left → thick+vivid right -->
					<path d="M0,5.6 L140,2.5 L140,9.5 L0,6.4 Z" fill="url(#nw-strength-alpha)" />
				</svg>
				<div class="strength-caption">opacity + width = connection weight</div>
			</div>
		</div>
	</div>
{/if}

<style>
	.legend-pill {
		position: fixed;
		top: 80px;
		right: 16px;
		width: 28px;
		height: 28px;
		border-radius: 999px;
		background: rgba(13, 13, 26, 0.95);
		border: 1px solid #3a3a5c;
		color: #a0a0c0;
		font-size: 14px;
		cursor: pointer;
		z-index: 50;
		display: flex;
		align-items: center;
		justify-content: center;
		transition: background 0.15s, color 0.15s;
	}

	.legend-pill:hover {
		background: rgba(91, 78, 248, 0.2);
		color: #fff;
	}

	.legend-panel {
		position: fixed;
		top: 80px;
		right: 16px;
		width: 220px;
		background: rgba(13, 13, 26, 0.95);
		backdrop-filter: blur(8px);
		border: 1px solid #3a3a5c;
		border-radius: 8px;
		padding: 12px 14px;
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.5);
		font-family: inherit;
		color: #e8e8f0;
		z-index: 50;
		font-size: 11px;
	}

	.legend-header {
		display: flex;
		justify-content: space-between;
		align-items: center;
		margin-bottom: 10px;
	}

	.legend-title {
		font-weight: 700;
		font-size: 12px;
		color: #e8e8f0;
	}

	.legend-collapse {
		background: transparent;
		border: none;
		color: #7b7b9a;
		font-size: 12px;
		cursor: pointer;
		padding: 0 4px;
	}

	.legend-collapse:hover {
		color: #fff;
	}

	.legend-section {
		margin-bottom: 12px;
	}

	.legend-section:last-child {
		margin-bottom: 0;
	}

	.section-label {
		font-size: 9px;
		font-weight: 700;
		letter-spacing: 1.2px;
		color: #5b4ef8;
		margin-bottom: 6px;
	}

	.energy-bar {
		display: flex;
		align-items: center;
		gap: 6px;
		margin-bottom: 6px;
	}

	.energy-bar .bar {
		flex: 1;
		height: 6px;
		border-radius: 3px;
		background: linear-gradient(to right, hsl(220, 70%, 60%), hsl(110, 70%, 60%), hsl(0, 70%, 60%));
	}

	.energy-min, .energy-max {
		font-size: 9px;
		color: #a0a0c0;
		white-space: nowrap;
	}

	.encoding-row {
		display: flex;
		align-items: center;
		gap: 8px;
		color: #c0c0d8;
		margin-top: 4px;
	}

	.dot-small {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: #c0c0d8;
		flex-shrink: 0;
	}

	.dot-glow {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: #c0c0d8;
		box-shadow: 0 0 8px #c0c0d8;
		flex-shrink: 0;
	}

	.edge-row {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-top: 4px;
	}

	.edge-swatch {
		width: 28px;
		height: 2px;
		border-radius: 1px;
		flex-shrink: 0;
	}

	.edge-label {
		color: #c0c0d8;
	}

	.strength-encoding {
		margin-top: 8px;
	}

	.strength-label-row {
		display: flex;
		justify-content: space-between;
		font-size: 9px;
		color: #7b7b9a;
		margin-bottom: 2px;
	}

	.strength-bar {
		width: 100%;
		height: 12px;
		display: block;
		border-radius: 2px;
		overflow: visible;
	}

	.strength-caption {
		font-size: 9px;
		color: #7b7b9a;
		text-align: center;
		margin-top: 3px;
	}
</style>
