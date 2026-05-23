<script lang="ts">
	import type { DjStatusResponse } from '$lib/api/client';

	let {
		status,
		onAcceptSafeOnly,
	}: {
		status: DjStatusResponse | null;
		onAcceptSafeOnly: () => void;
	} = $props();

	function formatPlanningStatus(value: string | null | undefined) {
		switch (value) {
			case 'waiting_for_profiles':
				return 'Waiting for profiles';
			case 'profile_failed':
				return 'Profile failed';
			case 'waiting_for_window':
				return 'Waiting for window';
			case 'ready_to_plan':
				return 'Ready to plan';
			case 'pair_missing':
				return 'Pair missing';
			case 'armed':
				return 'Armed';
			case 'missed':
				return 'Missed';
			case 'disabled':
				return 'Disabled';
			default:
				return 'Unknown';
		}
	}
</script>

<section class="guardrail-panel" aria-labelledby="dj-guardrails-heading">
	<header>
		<div>
			<p class="eyebrow">Guardrails</p>
			<h2 id="dj-guardrails-heading">Safety state</h2>
		</div>
	</header>

	<dl>
		<div>
			<dt>Confidence floor</dt>
			<dd>{status?.profile_confidence_floor ?? 0}</dd>
		</div>
		<div>
			<dt>Decode state</dt>
			<dd>{status?.fallback_reason === 'decode_late' ? 'Late' : 'Ready'}</dd>
		</div>
		<div>
			<dt>Analysis state</dt>
			<dd>{status?.fallback_reason === 'analysis_late' ? 'Late' : 'Ready'}</dd>
		</div>
		<div>
			<dt>Pair freshness</dt>
			<dd>{status?.fallback_reason === 'queue_changed' ? 'Stale pair' : 'Current pair'}</dd>
		</div>
		<div>
			<dt>Planning state</dt>
			<dd>{formatPlanningStatus(status?.planning_status)}</dd>
		</div>
	</dl>

	{#if status?.safe_crossfade_suggestion}
		<div class="suggestion">
			<p>
				Repeated bad feedback reached {status.safe_crossfade_suggestion.bad_feedback_count} events for
				{status.safe_crossfade_suggestion.media_ref_kind}:{status.safe_crossfade_suggestion.media_ref_id}.
			</p>
			<button type="button" onclick={onAcceptSafeOnly}>Accept safe-only suggestion</button>
		</div>
	{/if}
</section>

<style>
	.guardrail-panel {
		display: grid;
		gap: var(--space-3);
	}

	.eyebrow,
	h2,
	p,
	dl {
		margin: 0;
	}

	.eyebrow {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	h2 {
		font-size: var(--font-size-xl);
		line-height: var(--line-height-tight);
	}

	dl {
		display: grid;
		gap: var(--space-2);
	}

	dl div,
	.suggestion {
		padding: var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 86%, transparent);
	}

	dl div {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
	}

	dt,
	dd {
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	dt {
		color: var(--text-tertiary);
	}

	dd {
		margin: 0;
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
		text-align: right;
	}

	.suggestion {
		display: grid;
		gap: var(--space-2);
		border-color: color-mix(in srgb, var(--state-warning) 34%, transparent);
	}

	.suggestion p {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	button {
		min-height: 2.75rem;
		border: 1px solid var(--accent-line);
		border-radius: var(--radius-sm);
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
	}

	button:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
</style>
