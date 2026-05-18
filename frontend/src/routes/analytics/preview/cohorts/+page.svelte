<script lang="ts">
	/**
	 * Dev-only preview for CohortTable — three disjoint cohort rows.
	 *
	 * The captured fixture only has activity in the "New in selected window" cohort
	 * (everything was first-listened recently in the dev DB), so we synthesise
	 * Established + Deep cuts so the visual layout can be inspected without
	 * needing months of listening history.
	 */

	import CohortTable from '$lib/components/analytics/CohortTable.svelte';
	import fixture from '$lib/fixtures/analytics-signals.json';
	import type { CohortRow } from '$lib/api/client';

	type Variant = 'captured' | 'rich';

	let variant = $state<Variant>('rich');

	const captured = (fixture as { signals: any }).signals.cohorts as CohortRow[];

	const rich: CohortRow[] = [
		{
			key: 'new_this_month',
			label: 'New in selected window',
			tracks: 132,
			listened_ms: 24_480_000,
			sessions: 18,
			completion: 0.76,
			skip_rate: 0.24,
			new_artists: 21,
			repeat_rate: 1.4,
		},
		{
			key: 'established',
			label: 'Established',
			tracks: 87,
			listened_ms: 19_200_000,
			sessions: 32,
			completion: 0.82,
			skip_rate: 0.18,
			new_artists: 4,
			repeat_rate: 2.6,
		},
		{
			key: 'deep_cuts',
			label: 'Deep cuts',
			tracks: 42,
			listened_ms: 11_460_000,
			sessions: 14,
			completion: 0.88,
			skip_rate: 0.12,
			new_artists: 0,
			repeat_rate: 4.2,
		},
	];

	const cohorts = $derived<CohortRow[]>(variant === 'captured' ? captured : rich);
</script>

{#if import.meta.env.DEV}
	<div class="preview">
		<header>
			<h1>Cohorts — preview</h1>
			<p class="subtitle">
				<code>CohortTable</code> — disjoint partition of listened tracks into
				<em>New in selected window</em> / <em>Established</em> / <em>Deep cuts</em>. Dev-only route.
			</p>
		</header>

		<div class="controls">
			<label>
				<span class="control-label">Data</span>
				<select bind:value={variant}>
					<option value="rich">Synthesised (all 3 cohorts populated)</option>
					<option value="captured">Captured (real DB — only "New in selected window" has data)</option>
				</select>
			</label>
		</div>

		<CohortTable {cohorts} />

		<aside class="info">
			<h2>Cohort definitions (locked, disjoint)</h2>
			<dl>
				<dt>New in selected window</dt>
				<dd>First-ever listen falls inside the current window.</dd>
				<dt>Established</dt>
				<dd>First-listen older than 30 days, but not (older than 180 days AND lifetime ≥ 5 listens).</dd>
				<dt>Deep cuts</dt>
				<dd>First-listen older than 180 days AND lifetime listens ≥ 5.</dd>
			</dl>
			<p class="hint">
				Sums of <code>tracks</code> across the three rows equal the window's total
				listened-tracks count — proves the disjoint definitions partition the universe.
				<code>repeat_rate</code> is <code>total_listens / unique_tracks</code> within
				the cohort; <code>new_artists</code> counts artists first listened to inside
				the window. Cells with no data render <code>--</code> per the formatter rule.
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
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
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
		font-size: var(--font-size-xs);
		background: var(--input-bg);
		border: 1px solid var(--input-border);
		color: var(--text-primary);
		padding: 4px 8px;
		border-radius: var(--radius-xs);
	}

	.control-label {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.12em;
		color: var(--text-tertiary);
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
		font-size: var(--font-size-xs);
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
		font-family: var(--font-body);
		font-size: var(--font-size-sm);
	}

	.info dt {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
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
		font-size: var(--font-size-sm);
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
