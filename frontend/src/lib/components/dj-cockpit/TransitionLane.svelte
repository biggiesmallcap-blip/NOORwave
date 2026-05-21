<script lang="ts">
	import type { DjStatusResponse } from '$lib/api/client';

	let {
		status,
		onFeedback,
		debugOpen = false,
		onToggleDebug,
	}: {
		status: DjStatusResponse | null;
		onFeedback: (rating: 'good' | 'bad' | 'too_safe' | 'too_bold') => void;
		debugOpen?: boolean;
		onToggleDebug: () => void;
	} = $props();

	let fallback = $derived(status?.fallback_reason ?? null);
	let selectedProgram = $derived(status?.selected_program ?? 'Legacy path');
	let transitionId = $derived(status?.last_transition_event_id ?? null);
</script>

<section class="transition-lane" aria-labelledby="dj-transition-heading">
	<header>
		<div>
			<p class="eyebrow">Transition lane</p>
			<h2 id="dj-transition-heading">{selectedProgram}</h2>
		</div>
		<span class="event-id">{transitionId ? `Event ${transitionId}` : 'No event armed'}</span>
	</header>

	{#if fallback}
		<p class="fallback" role="status">Fallback reason: {fallback}</p>
	{:else}
		<p class="success">Transition ready. Detailed successful-transition reasons are hidden by default.</p>
	{/if}

	<div class="lane-actions" aria-label="Transition feedback">
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('good')}>Good</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('bad')}>Bad</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('too_safe')}>Too safe</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('too_bold')}>Too bold</button>
	</div>

	<button class="debug-toggle" type="button" aria-expanded={debugOpen} onclick={onToggleDebug}>
		Debug planner facts
	</button>
	{#if debugOpen}
		<div class="debug-panel">
			<dl>
				<div>
					<dt>Selected program</dt>
					<dd>{status?.selected_program ?? 'none'}</dd>
				</div>
				<div>
					<dt>Fallback</dt>
					<dd>{status?.fallback_reason ?? 'none'}</dd>
				</div>
				<div>
					<dt>Confidence floor</dt>
					<dd>{status?.profile_confidence_floor ?? 0}</dd>
				</div>
			</dl>
		</div>
	{/if}
</section>

<style>
	.transition-lane {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4);
		border: 1px solid var(--border-muted);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-raised) 90%, transparent);
	}

	header {
		display: flex;
		align-items: start;
		justify-content: space-between;
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
		margin-top: var(--space-1);
		font-size: var(--font-size-2xl);
		line-height: var(--line-height-tight);
	}

	.event-id {
		flex-shrink: 0;
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.fallback,
	.success {
		padding: var(--space-3);
		border-radius: var(--radius-sm);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.fallback {
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
	}

	.success {
		border: 1px solid color-mix(in srgb, var(--state-success) 28%, transparent);
		background: color-mix(in srgb, var(--state-success) 9%, transparent);
		color: var(--text-secondary);
	}

	.lane-actions {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--space-2);
	}

	button {
		min-height: 2.75rem;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 86%, transparent);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	button:hover,
	button:focus-visible {
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	button:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.debug-toggle {
		justify-self: start;
		padding: 0 var(--space-3);
	}

	.debug-panel {
		padding: var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 78%, transparent);
	}

	dl {
		display: grid;
		gap: var(--space-2);
	}

	dl div {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
	}

	dt {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	dd {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-align: right;
	}

	@media (max-width: 760px) {
		header,
		dl div {
			display: grid;
		}

		.lane-actions {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
