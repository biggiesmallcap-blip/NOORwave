<script lang="ts">
	/**
	 * Dev-only preview for the top-of-page strip — TimeRangePills + KpiStrip composed
	 * the way they'll appear above the hero in the rewritten analytics page.
	 */

	import KpiStrip from '$lib/components/analytics/KpiStrip.svelte';
	import TimeRangePills from '$lib/components/analytics/TimeRangePills.svelte';
	import type { TimeRange } from '$lib/components/analytics/TimeRangePills.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import { generateDemoKpis, type KpiProfile } from '$lib/fixtures/demo-kpis';
	import type { SignalsKpis } from '$lib/api/client';

	type DataSource = 'captured' | KpiProfile;
	let dataSource = $state<DataSource>('casual');
	let demoDays = $state(30);

	const captured = (fixture as { signals: any }).signals.kpis as SignalsKpis;

	const kpis = $derived<SignalsKpis>(
		dataSource === 'captured' ? captured : generateDemoKpis(dataSource, demoDays),
	);

	let range = $state<TimeRange>('30d');
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Top strip — preview</h1>
			<p class="subtitle">
				<code>TimeRangePills</code> + <code>KpiStrip</code> composed the way they'll appear
				above the hero. Dev-only route.
			</p>
		</header>

		<div class="controls">
			<label>
				<span class="control-label">Data</span>
				<select bind:value={dataSource}>
					<option value="captured">Captured (real DB)</option>
					<option value="casual">Demo · Casual (~32/day)</option>
					<option value="heavy">Demo · Heavy (~120/day)</option>
					<option value="sporadic">Demo · Sporadic (irregular)</option>
					<option value="recovering">Demo · Recovering (+drift)</option>
				</select>
			</label>

			<label>
				<span class="control-label">Days</span>
				<select bind:value={demoDays} disabled={dataSource === 'captured'}>
					<option value={7}>7</option>
					<option value={14}>14</option>
					<option value={30}>30</option>
					<option value={60}>60</option>
					<option value={90}>90</option>
				</select>
			</label>
		</div>

		<!-- Mock PageHeader to show how the pills sit on the right -->
		<div class="header-mock">
			<div class="title-block">
				<span class="eyebrow">Analytics</span>
				<h2>Library analytics</h2>
			</div>
			<div class="actions">
				<TimeRangePills bind:value={range} />
				<button class="refresh" type="button" aria-label="Refresh">↻</button>
			</div>
		</div>

		<KpiStrip {kpis} />

		<aside class="info">
			<h2>Source data</h2>
			<dl>
				<dt>Time range</dt><dd>{range}</dd>
				<dt>Listened (cur / prev)</dt><dd>{kpis.listened_ms.current} / {kpis.listened_ms.previous} ms</dd>
				<dt>Sessions (cur / prev)</dt><dd>{kpis.sessions.current} / {kpis.sessions.previous}</dd>
				<dt>Completion (cur / prev)</dt><dd>{kpis.completion.current} / {kpis.completion.previous}</dd>
				<dt>Skip rate (cur / prev)</dt><dd>{kpis.skip_rate.current} / {kpis.skip_rate.previous}</dd>
				<dt>Daily series length</dt><dd>{kpis.daily.length}</dd>
				<dt>Sessions coverage</dt>
				<dd>{kpis.sessions_coverage.tracked} tracked · {kpis.sessions_coverage.untracked} untracked</dd>
			</dl>
			<p class="hint">
				Pill choice persists to <code>localStorage[noor:analytics:days]</code>.
				Reload to verify the picked range comes back. Delta arrows are
				<span style="color: var(--state-success)">+green</span> for positive,
				<span style="color: var(--state-error)">-red</span> for negative,
				neutral mono for &lt;0.5%.
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
		max-width: 1280px;
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

	.controls select {
		font-family: var(--font-mono);
		font-size: 0.78rem;
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		color: var(--text-primary);
		padding: 4px 8px;
		border-radius: var(--radius-xs);
	}

	.control-label {
		font-family: var(--font-mono);
		font-size: 0.65rem;
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
	}

	.header-mock {
		display: flex;
		justify-content: space-between;
		align-items: flex-end;
		gap: var(--space-4);
		padding-bottom: var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
	}

	.title-block {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: 0.7rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.title-block h2 {
		font-family: var(--font-display);
		font-size: 1.5rem;
		font-weight: 500;
		margin: 0;
	}

	.actions {
		display: flex;
		gap: var(--space-2);
		align-items: center;
	}

	.refresh {
		font-family: var(--font-mono);
		font-size: 0.9rem;
		background: transparent;
		border: 1px solid var(--input-border);
		color: var(--text-secondary);
		width: 32px;
		height: 32px;
		border-radius: var(--radius-xs);
		cursor: pointer;
	}

	.refresh:hover {
		color: var(--text-primary);
		border-color: var(--border-strong);
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
