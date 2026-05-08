<script lang="ts">
	/**
	 * CohortTable — disjoint partition of the window's listened tracks into three
	 * cohorts:
	 *   - New this month: first-listen falls inside current window
	 *   - Established:    first-listen older than 30 days, NOT (older than 180d AND lifetime ≥ 5 listens)
	 *   - Deep cuts:      first-listen older than 180 days AND lifetime ≥ 5 listens
	 *
	 * Columns: COHORT · TRACKS · LISTENED · SESSIONS · COMPLETION · SKIP RATE · NEW ARTISTS · REPEAT RATE.
	 *
	 * Spec: C:\Users\Felix\.claude\plans\lets-revision-analytics-stats-crystalline-melody.md
	 */

	import type { CohortRow } from '$lib/api/client';
	import { formatCount, formatDuration, formatMultiplier, formatPercent } from '$lib/utils/format';

	interface Props {
		cohorts: CohortRow[];
	}

	let { cohorts }: Props = $props();

	const isEmpty = $derived(cohorts.every((c) => c.tracks === 0));
</script>

<section class="cohort glass" aria-label="Listening cohorts">
	<header class="head">
		<span class="eyebrow">Cohorts</span>
	</header>

	{#if isEmpty}
		<p class="empty">No tracks in any cohort yet.</p>
	{:else}
		<div class="scroll">
			<table>
				<thead>
					<tr>
						<th class="col-label">Cohort</th>
						<th>Tracks</th>
						<th>Listened</th>
						<th>Sessions</th>
						<th>Completion</th>
						<th>Skip rate</th>
						<th>New artists</th>
						<th>Repeat rate</th>
					</tr>
				</thead>
				<tbody>
					{#each cohorts as c (c.key)}
						<tr class:zero={c.tracks === 0}>
							<td class="col-label">{c.label}</td>
							<td>{formatCount(c.tracks)}</td>
							<td>{formatDuration(c.listened_ms)}</td>
							<td>{formatCount(c.sessions)}</td>
							<td>{formatPercent(c.completion, { decimals: 0 })}</td>
							<td>{formatPercent(c.skip_rate, { decimals: 0 })}</td>
							<td>{formatCount(c.new_artists)}</td>
							<td>{formatMultiplier(c.repeat_rate)}</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	.cohort {
		padding: var(--space-4);
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.head {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
	}

	.eyebrow {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
	}

	.empty {
		font-family: var(--font-body);
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		margin: 0;
	}

	.scroll {
		overflow-x: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-family: var(--font-mono);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
	}

	thead th {
		text-align: right;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-medium);
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-tertiary);
		padding: var(--space-2) var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		white-space: nowrap;
	}

	thead th.col-label {
		text-align: left;
	}

	tbody td {
		text-align: right;
		padding: var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-primary);
		white-space: nowrap;
	}

	tbody tr:last-child td {
		border-bottom: none;
	}

	tbody td.col-label {
		text-align: left;
		font-family: var(--font-display);
		font-size: var(--font-size-md);
		color: var(--text-primary);
	}

	tbody tr.zero td {
		color: var(--text-tertiary);
	}

	tbody tr.zero td.col-label {
		color: var(--text-secondary);
	}
</style>
