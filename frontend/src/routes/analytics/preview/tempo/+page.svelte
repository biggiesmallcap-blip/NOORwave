<script lang="ts">
	/**
	 * Dev-only preview route for tuning TempoRidges visually against the captured fixture.
	 * Gated by import.meta.env.DEV so it 404s in production.
	 */

	import TempoRidges from '$lib/components/charts/TempoRidges.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import { generateDemoTempo, type TempoProfile } from '$lib/fixtures/demo-tempo';
	import type { TempoView } from '$lib/api/client';
	import { onMount } from 'svelte';

	const SIGMA_KEY = 'noor:analytics:preview:tempo-sigma';
	const SIGMA_MIN = 0.3;
	const SIGMA_MAX = 1.5;
	const SIGMA_STEP = 0.05;
	const SIGMA_DEFAULT = 0.6;

	let sigma = $state(SIGMA_DEFAULT);
	type DataSource = 'captured' | TempoProfile;
	let dataSource = $state<DataSource>('house-techno');
	let demoDays = $state(60);

	const captured = (fixture as { signals: any }).signals.tempo as TempoView;
	const tempo = $derived<TempoView>(
		dataSource === 'captured' ? captured : generateDemoTempo(dataSource, demoDays),
	);

	onMount(() => {
		const stored = localStorage.getItem(SIGMA_KEY);
		if (stored) {
			const parsed = Number(stored);
			if (Number.isFinite(parsed) && parsed >= SIGMA_MIN && parsed <= SIGMA_MAX) {
				sigma = parsed;
			}
		}
	});

	$effect(() => {
		localStorage.setItem(SIGMA_KEY, String(sigma));
	});
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Tempo ridges — preview</h1>
			<p class="subtitle">Visual tuning sandbox for <code>TempoRidges.svelte</code>. Dev-only route.</p>
		</header>

		<div class="controls">
			<label>
				<span class="control-label">Sigma</span>
				<input
					type="range"
					min={SIGMA_MIN}
					max={SIGMA_MAX}
					step={SIGMA_STEP}
					bind:value={sigma}
				/>
				<span class="value">{sigma.toFixed(2)}</span>
			</label>

			<label>
				<span class="control-label">Data</span>
				<select bind:value={dataSource}>
					<option value="captured">Captured (real DB)</option>
					<option value="house-techno">Demo · House / Techno</option>
					<option value="downtempo">Demo · Downtempo</option>
					<option value="eclectic">Demo · Eclectic spread</option>
					<option value="pop-radio">Demo · Pop radio</option>
				</select>
			</label>

			<label>
				<span class="control-label">Days</span>
				<select bind:value={demoDays} disabled={dataSource === 'captured'}>
					<option value={30}>30 (daily)</option>
					<option value={60}>60 (weekly)</option>
					<option value={90}>90 (weekly)</option>
					<option value={180}>180 (monthly)</option>
					<option value={365}>365 (monthly)</option>
				</select>
			</label>

			<button onclick={() => (sigma = SIGMA_DEFAULT)} class="reset">Reset sigma</button>
		</div>

		<section class="card glass">
			<div class="card-header">
				<span class="eyebrow">Tempo</span>
				<h2>Tempo ridges</h2>
			</div>
			<TempoRidges {tempo} {sigma} />
		</section>

		<aside class="info">
			<h2>Source data</h2>
			<dl>
				<dt>Rows</dt><dd>{tempo.rows.length}</dd>
				<dt>Granularity</dt><dd>{tempo.rows[0]?.granularity ?? '--'}</dd>
				<dt>Bucket axis</dt>
				<dd>
					{tempo.bucket_axis.min}–{tempo.bucket_axis.max} step {tempo.bucket_axis.step}
				</dd>
				<dt>Median</dt><dd>{tempo.stats.median ?? '--'}</dd>
				<dt>Mode</dt><dd>{tempo.stats.mode ?? '--'}</dd>
				<dt>Sigma (BPM)</dt><dd>{tempo.stats.sigma ?? '--'}</dd>
				<dt>Coverage</dt><dd>{tempo.coverage.analyzed} / {tempo.coverage.total_listened}</dd>
				<dt>ridge_amp_max</dt><dd>{tempo.ridge_amp_max}</dd>
			</dl>
			<p class="hint">
				Sigma is in bin-widths (each bucket = 4 BPM). 0.6 default matches the ridgeline.
				If the BPM peaks read sharper than the ridgeline at the same sigma, that's expected —
				BPM has fewer source bins (36 vs 24) and tracks usually cluster around a few tempos.
			</p>
		</aside>
	</div>
{:else}
	<div class="not-found">
		<h1>404</h1>
		<p>This route is dev-only.</p>
	</div>
{/if}

<style>
	.preview {
		max-width: var(--content-width);
		margin: 0 auto;
		padding: var(--space-5) var(--space-5) var(--space-7);
		display: flex;
		flex-direction: column;
		gap: var(--space-5);
	}

	header h1 {
		font-family: var(--font-display);
		font-size: 1.6rem;
		font-weight: 600;
		margin: 0 0 var(--space-1);
	}

	.subtitle {
		font-family: var(--font-body);
		color: var(--text-secondary);
		margin: 0;
	}

	.controls {
		display: flex;
		gap: var(--space-5);
		align-items: center;
		flex-wrap: wrap;
		padding: var(--space-3) var(--space-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}

	.controls label {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}

	.control-label {
		font-family: var(--font-mono);
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
	}

	.controls input[type='range'] {
		width: 240px;
	}

	.controls .value {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		color: var(--text-primary);
		min-width: 3.5em;
	}

	.reset {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		background: transparent;
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		padding: 6px 12px;
		border-radius: var(--radius-xs);
		cursor: pointer;
	}

	.reset:hover {
		color: var(--text-primary);
		border-color: var(--border-strong);
	}

	/* .glass (from app.css) supplies the surface; .card just adds inner padding. */
	.card {
		padding: var(--space-4);
	}

	.card-header {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding-bottom: var(--space-3);
		margin-bottom: var(--space-3);
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.card-header h2 {
		font-family: var(--font-display);
		font-size: 1.4rem;
		font-weight: 500;
		margin: 0;
		color: var(--text-primary);
	}

	.info {
		display: grid;
		grid-template-columns: 1fr;
		gap: var(--space-3);
		padding: var(--space-4);
		background: var(--bg-elevated);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
	}

	.info h2 {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
		margin: 0;
	}

	.info dl {
		display: grid;
		grid-template-columns: max-content 1fr;
		gap: var(--space-2) var(--space-4);
		margin: 0;
		font-family: var(--font-mono);
		font-size: 0.78rem;
	}

	.info dt {
		color: var(--text-tertiary);
	}

	.info dd {
		margin: 0;
		color: var(--text-primary);
	}

	.info .hint {
		margin: 0;
		font-family: var(--font-body);
		color: var(--text-secondary);
		font-size: 0.85rem;
	}

	.not-found {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		min-height: 60vh;
		gap: var(--space-3);
	}
</style>
