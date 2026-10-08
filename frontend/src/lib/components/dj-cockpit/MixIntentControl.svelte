<script lang="ts">
	// How DJ mixes: a preferred transition style, a mix intent and a speed.
	// All three are saved DJ policy and only matter while DJ is on, so they sit
	// together on the Mix page.
	import type { DjMixIntent, DjTransitionSpeedBias, DjStrategy } from '$lib/api/client';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import { strategyDescription, strategyLabels } from './transition_scene';

	let {
		intent,
		speed,
		strategy,
		disabled = false,
		onIntentChange,
		onSpeedChange,
		onStrategyChange,
	}: {
		intent: DjMixIntent;
		speed: DjTransitionSpeedBias;
		strategy: DjStrategy;
		disabled?: boolean;
		onIntentChange: (intent: DjMixIntent) => void;
		onSpeedChange: (speed: DjTransitionSpeedBias) => void;
		onStrategyChange: (strategy: DjStrategy) => void;
	} = $props();

	const intents: Array<{ value: DjMixIntent; label: string }> = [
		{ value: 'safe', label: 'Conservative' },
		{ value: 'balanced', label: 'Balanced' },
		{ value: 'bold', label: 'Adventurous' },
	];

	const strategies = (
		['adaptive', 'smooth_blend', 'club_mix', 'quick_mix', 'energy_lift', 'energy_reset', 'drop_swap', 'bass_swap', 'cut', 'wildcard'] as const
	).map((value) => ({ value, label: strategyLabels[value] }));

	const speeds: Array<{ value: DjTransitionSpeedBias; label: string }> = [
		{ value: 'slower', label: 'Slower' },
		{ value: 'neutral', label: 'Neutral' },
		{ value: 'faster', label: 'Faster' },
	];
</script>

<div class="policy-controls">
	<div class="control-block">
		<span class="control-label">Transition style</span>
		<Dropdown label="Transition style" options={strategies} value={strategy} {disabled} full onchange={onStrategyChange} />
		<span class="style-note">A preference; the safest suitable plan still wins.</span>
		<span class="style-note">{strategyDescription(strategy)}</span>
	</div>
	<div class="control-block">
		<span class="control-label">Mix intent</span>
		<Segmented label="Mix intent" options={intents} value={intent} {disabled} full onchange={onIntentChange} />
	</div>

	<div class="control-block">
		<span class="control-label">Transition speed</span>
		<Segmented label="Transition speed" options={speeds} value={speed} {disabled} full onchange={onSpeedChange} />
	</div>
</div>
<p class="style-note">The main handoff usually happens near the outgoing track’s ending. Separately, DJ can tease a compatible incoming drop around the middle when analysis identifies a safe phrase and drop marker.</p>

<style>
	.style-note { font-size: var(--font-size-2xs); color: var(--text-tertiary); }
	p.style-note { margin: 0; }
	.policy-controls {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-3);
		align-items: end;
	}

	.control-block {
		display: grid;
		gap: var(--space-1);
		min-width: min(100%, 17rem);
		max-width: 24rem;
	}

	.control-label {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}
</style>
