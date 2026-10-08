<script lang="ts">
	// The DJ parts of the Mix page. part="live": the engine notice, what DJ is
	// about to do and feedback on the last transition. part="controls": how it
	// mixes (style, intent, speed), shown under This session. The switch lives
	// in the Mix header and the planner details in its Diagnostics
	// (DjDiagnostics); all of them read dj_engine.ts.
	import { onMount } from 'svelte';
	import MixIntentControl from './MixIntentControl.svelte';
	import TransitionStory from './TransitionStory.svelte';
	import {
		djEnabled as enabled,
		djFiredCut as firedCut,
		djLoadError as loadError,
		djLoading as loading,
		djMixIntent as mixIntent,
		djSaving as saving,
		djSpeedBias as speedBias,
		djStatus as status,
		djStrategy as strategy,
		recordDjFeedback,
		refreshDj,
		setDjIntent,
		setDjSpeed,
		setDjStrategy,
		startDjPolling,
		type DjFeedbackRating,
	} from './dj_engine';

	let { part = 'live' }: { part?: 'live' | 'controls' } = $props();

	onMount(() => startDjPolling());

	const ratings: Array<{ value: DjFeedbackRating; label: string }> = [
		{ value: 'good', label: 'Good' },
		{ value: 'bad', label: 'Bad' },
		{ value: 'too_safe', label: 'Too safe' },
		{ value: 'too_bold', label: 'Too bold' },
	];
</script>

{#if part === 'controls'}
	<MixIntentControl
		intent={$mixIntent}
		speed={$speedBias}
		strategy={$strategy}
		disabled={$loading || $saving}
		onIntentChange={(next) => void setDjIntent(next)}
		onSpeedChange={(next) => void setDjSpeed(next)}
		onStrategyChange={(next) => void setDjStrategy(next)}
	/>
{:else}
<section class="dj-cockpit" aria-label="DJ transitions">
	{#if $loadError}
		<div class="load-status" role="status">
			<p>DJ information is temporarily unavailable. {$status ? 'Showing the last update while retrying.' : 'Retrying automatically.'}</p>
			<button class="btn btn-glass" type="button" onclick={() => void refreshDj(false, true)}>Retry now</button>
			<details><summary>Request details</summary><p>{$loadError}</p></details>
		</div>
	{/if}
	{#if $enabled === false}
		<p class="disabled-note">
			Playback is using the legacy path. DJ lookahead and transition planning are stopped.
		</p>
	{:else if $enabled}
		<p class="enabled-note">
			DJ is planning the next eligible current-plus-next pair.
		</p>
	{/if}

	<TransitionStory status={$status} enabled={$enabled} />
	{#if $firedCut}<p class="enabled-note" role="status">{$firedCut}</p>{/if}
	<div class="feedback" role="group" aria-label="Rate the last played transition">
		<span>Last transition</span>
		{#each ratings as item (item.value)}
			<button class="btn btn-glass" type="button" disabled={!$status?.feedback_transition_event_id} onclick={() => void recordDjFeedback(item.value)}>{item.label}</button>
		{/each}
	</div>
</section>
{/if}

<style>
	.load-status { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: var(--space-3); color: var(--text-secondary); background: var(--bg-surface); }
	.load-status > p { margin: 0; flex: 1; min-width: 15rem; font-size: var(--font-size-sm); }
	.load-status details { width: 100%; overflow-wrap: anywhere; font-size: var(--font-size-xs); }
	.feedback { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-sm); }
	summary { cursor: pointer; font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); padding: var(--space-2); }
	summary:focus-visible { outline: 2px solid var(--accent-strong); outline-offset: 2px; }

	.dj-cockpit {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	p {
		margin: 0;
	}

	.disabled-note,
	.enabled-note {
		padding: var(--space-3);
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.enabled-note {
		border-color: color-mix(in srgb, var(--state-success) 32%, transparent);
		background: color-mix(in srgb, var(--state-success) 8%, transparent);
		color: var(--text-secondary);
	}
</style>
