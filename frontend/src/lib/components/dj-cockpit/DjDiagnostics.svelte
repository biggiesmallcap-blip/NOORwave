<script lang="ts">
	// DJ planner details and Fine tune, inside the Mix page's one Diagnostics
	// disclosure. Reads the same status as the cockpit (dj_engine.ts).
	import { onMount } from 'svelte';
	import ProfileCorrectionPanel from './ProfileCorrectionPanel.svelte';
	import QueuePairPanel from './QueuePairPanel.svelte';
	import SafetyGuardrailPanel from './SafetyGuardrailPanel.svelte';
	import TransitionLane from './TransitionLane.svelte';
	import {
		acceptSafeOnlySuggestion,
		clearCorrection,
		djRebuildStatus as rebuildStatus,
		djSaving as saving,
		djStatus as status,
		rebuildProfile,
		recordDjFeedback,
		saveDjCorrection,
		startDjPolling,
	} from './dj_engine';

	onMount(() => startDjPolling());

	let debugOpen = $state(false);
	let transitionArmed = $derived(Boolean($status?.selected_program || $status?.last_transition_event_id));
</script>

<div class="dj-diagnostics">
	<section class="column" aria-labelledby="dj-planner-heading">
		<h3 id="dj-planner-heading" class="t-row-title">DJ planner</h3>
		<TransitionLane
			status={$status}
			{debugOpen}
			onToggleDebug={() => { debugOpen = !debugOpen; }}
			onFeedback={(rating) => void recordDjFeedback(rating)}
		/>
		<QueuePairPanel current={$status?.current} next={$status?.next} />
	</section>

	<section class="column" aria-labelledby="dj-fine-tune-heading">
		<h3 id="dj-fine-tune-heading" class="t-row-title">Fine tune</h3>
		<ProfileCorrectionPanel
			current={$status?.current}
			next={$status?.next}
			{transitionArmed}
			busy={$saving}
			onSave={(correction) => void saveDjCorrection(correction)}
			onClear={clearCorrection}
			onRebuild={(ref) => void rebuildProfile(ref)}
		/>
		{#if $rebuildStatus}
			<p class="rebuild-status" role="status">{$rebuildStatus}</p>
		{/if}
		<SafetyGuardrailPanel status={$status} onAcceptSafeOnly={acceptSafeOnlySuggestion} />
	</section>
</div>

<style>
	.dj-diagnostics {
		display: grid;
		grid-template-columns: minmax(0, 1.55fr) minmax(19rem, 0.85fr);
		gap: var(--space-4);
		align-items: start;
	}

	.column {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	h3 {
		margin: 0;
	}

	.rebuild-status {
		margin: 0;
		padding: var(--space-3);
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	@media (max-width: 980px) {
		.dj-diagnostics {
			grid-template-columns: 1fr;
		}
	}
</style>
