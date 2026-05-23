<script lang="ts">
	import type { DjMixIntent, DjTransitionSpeedBias } from '$lib/api/client';

	let {
		intent,
		speed,
		disabled = false,
		onIntentChange,
		onSpeedChange,
	}: {
		intent: DjMixIntent;
		speed: DjTransitionSpeedBias;
		disabled?: boolean;
		onIntentChange: (intent: DjMixIntent) => void;
		onSpeedChange: (speed: DjTransitionSpeedBias) => void;
	} = $props();

	const intents: Array<{ value: DjMixIntent; label: string }> = [
		{ value: 'safe', label: 'Safe' },
		{ value: 'balanced', label: 'Balanced' },
		{ value: 'bold', label: 'Bold' },
	];

	const speeds: Array<{ value: DjTransitionSpeedBias; label: string }> = [
		{ value: 'slower', label: 'Slower' },
		{ value: 'neutral', label: 'Neutral' },
		{ value: 'faster', label: 'Faster' },
	];
</script>

<div class="policy-controls">
	<div class="control-block">
		<span class="control-label">Mix intent</span>
		<div class="segmented" role="group" aria-label="Mix intent">
			{#each intents as item}
				<button
					type="button"
					class:active={intent === item.value}
					aria-pressed={intent === item.value}
					disabled={disabled}
					onclick={() => onIntentChange(item.value)}
				>
					{item.label}
				</button>
			{/each}
		</div>
	</div>

	<div class="control-block">
		<span class="control-label">Transition speed</span>
		<div class="segmented" role="group" aria-label="Transition speed">
			{#each speeds as item}
				<button
					type="button"
					class:active={speed === item.value}
					aria-pressed={speed === item.value}
					disabled={disabled}
					onclick={() => onSpeedChange(item.value)}
				>
					{item.label}
				</button>
			{/each}
		</div>
	</div>
</div>

<style>
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
	}

	.control-label {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.segmented {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--space-1);
		padding: var(--space-1);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 88%, transparent);
	}

	button {
		min-height: 2.75rem;
		border: 1px solid transparent;
		border-radius: var(--radius-xs);
		background: transparent;
		color: var(--text-secondary);
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
		border-color: var(--border-muted);
		color: var(--text-primary);
	}

	button:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	button.active {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}
</style>
