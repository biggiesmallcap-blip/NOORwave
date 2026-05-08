<script lang="ts">
	/**
	 * Dev-only preview route for tuning the ListenRidgeline component visually.
	 *
	 * Loads the fixture at frontend/src/lib/fixtures/analytics-signals.json and exposes
	 * a sigma slider so we can lock RIDGE_SIGMA in ListenRidgeline.svelte against real
	 * data. Slider value persists to localStorage[noor:analytics:preview:sigma].
	 *
	 * Gated to import.meta.env.DEV so it never ships in production builds.
	 */

	import ListenRidgeline from '$lib/components/charts/ListenRidgeline.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import { generateDemoRidgeline, type DemoProfile } from '$lib/fixtures/demo-ridgeline';
	import { onMount } from 'svelte';

	const SIGMA_KEY = 'noor:analytics:preview:sigma';
	const SIGMA_MIN = 0.3;
	const SIGMA_MAX = 1.5;
	const SIGMA_STEP = 0.05;
	const SIGMA_DEFAULT = 0.6;

	let sigma = $state(SIGMA_DEFAULT);
	let mode: 'hero' | 'solo' = $state('hero');
	let variant: 'default' | 'single-day' = $state('default');
	type DataSource = 'captured' | DemoProfile;
	let dataSource = $state<DataSource>('routine');
	let demoDays = $state(60);

	const captured = (() => {
		const signals = (fixture as { signals: any }).signals;
		return {
			rows: signals.ridgeline as any[],
			heroStats: signals.kpis.hero_stats,
			ridgeAmpMax: (signals.tempo?.ridge_amp_max as number | null | undefined) ?? null,
		};
	})();

	const demoData = $derived(
		dataSource === 'captured' ? null : generateDemoRidgeline(dataSource, demoDays),
	);

	const rows = $derived(dataSource === 'captured' ? captured.rows : demoData!.ridgeline);
	const heroStats = $derived(
		dataSource === 'captured' ? captured.heroStats : demoData!.heroStats,
	);
	const ridgeAmpMax = $derived(
		dataSource === 'captured' ? captured.ridgeAmpMax : demoData!.ridgeAmpMax,
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
			<h1>Listening pulse — preview</h1>
			<p class="subtitle">Visual tuning sandbox for <code>ListenRidgeline.svelte</code>. Dev-only route.</p>
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
					<option value="routine">Demo · Routine listener</option>
					<option value="weekend-heavy">Demo · Weekend-heavy</option>
					<option value="late-night-owl">Demo · Late-night owl</option>
					<option value="casual">Demo · Casual sparse</option>
				</select>
			</label>

			<label>
				<span class="control-label">Days</span>
				<select bind:value={demoDays} disabled={dataSource === 'captured'}>
					<option value={30}>30</option>
					<option value={60}>60</option>
					<option value={90}>90</option>
					<option value={120}>120</option>
					<option value={180}>180</option>
				</select>
			</label>

			<label>
				<span class="control-label">Mode</span>
				<select bind:value={mode}>
					<option value="hero">hero</option>
					<option value="solo">solo</option>
				</select>
			</label>

			<label>
				<span class="control-label">Spine variant</span>
				<select bind:value={variant} disabled={mode !== 'hero'}>
					<option value="default">default (4 stats)</option>
					<option value="single-day">single-day (24h)</option>
				</select>
			</label>

			<button onclick={() => (sigma = SIGMA_DEFAULT)} class="reset">Reset sigma</button>
		</div>

		<section class="card glass">
			<ListenRidgeline {rows} {heroStats} {mode} {variant} {sigma} {ridgeAmpMax} />
		</section>

		<aside class="info">
			<h2>Source data</h2>
			<dl>
				<dt>Daily rows in</dt><dd>{rows.length}</dd>
				<dt>Granularity</dt><dd>{rows.length >= 90 ? 'weekly (auto)' : 'daily'}</dd>
				<dt>Rendered rows</dt><dd>{rows.length >= 90 ? Math.ceil(rows.length / 7) : rows.length}</dd>
				<dt>ridge_amp_max</dt><dd>{ridgeAmpMax}</dd>
				<dt>Peak hour</dt><dd>{heroStats.peak_hour ?? '--'}</dd>
				<dt>Rhythm</dt><dd>{heroStats.rhythm ?? '--'}</dd>
				<dt>Night share</dt><dd>{heroStats.night_share ?? '--'}</dd>
				<dt>Morning share</dt><dd>{heroStats.morning_share ?? '--'}</dd>
			</dl>
			<p class="hint">
				Sigma is locked at 0.6 in both daily and weekly modes inside <code>ListenRidgeline.svelte</code>.
				The slider here overrides for tuning. At 90+ daily rows the component auto-aggregates
				to weekly (each row = 7 days summed) so the chart stays legible at long windows.
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

	.controls select {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		color: var(--text-primary);
		padding: 4px 8px;
		border-radius: var(--radius-xs);
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

	/* .glass (from app.css) supplies the background, border, blur, and shadow.
	   Local .card just adds overflow clipping so ridges can't poke past the corners. */
	.card {
		overflow: hidden;
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
