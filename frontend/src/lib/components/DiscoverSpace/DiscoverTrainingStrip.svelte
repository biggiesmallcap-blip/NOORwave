<script lang="ts">
	import { training } from '$lib/stores/training';
	import { TRAINING_PHASES } from './discover_space_story';

	// Rotate phase copy during training
	let phaseIndex = $state(0);
	let phaseInterval: ReturnType<typeof setInterval> | null = null;

	$effect(() => {
		if ($training?.isRunning) {
			phaseInterval = setInterval(() => {
				phaseIndex = (phaseIndex + 1) % TRAINING_PHASES.length;
			}, 2200);
		} else {
			if (phaseInterval) { clearInterval(phaseInterval); phaseInterval = null; }
			phaseIndex = 0;
		}
		return () => { if (phaseInterval) clearInterval(phaseInterval); };
	});

	// Hide when no training data is available
	let visible = $derived(
		$training?.isRunning === true ||
		($training != null && typeof $training.tracks_total === 'number' && $training.tracks_total > 0)
	);

	let progress = $derived(
		$training && $training.tracks_total > 0
			? ($training.tracks_done ?? 0) / $training.tracks_total
			: 0
	);
</script>

{#if visible}
	<div class="training-strip" role="status" aria-live="polite">
		<span class="training-dot" aria-hidden="true"></span>
		<span class="training-label">
			{$training?.isRunning ? TRAINING_PHASES[phaseIndex] : 'Discovery map updated'}
		</span>
		<div class="training-bar" role="progressbar" aria-valuenow={Math.round(progress * 100)} aria-valuemin={0} aria-valuemax={100}>
			<div class="training-fill" style:width="{progress * 100}%"></div>
		</div>
		{#if $training?.tracks_total}
			<span class="training-count">{$training.tracks_done ?? 0} / {$training.tracks_total}</span>
		{/if}
	</div>
{/if}

<style>
	.training-strip {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 6px 14px;
		background: rgba(10, 10, 20, 0.85);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--accent-line);
		border-radius: 999px;
		font-size: 0.75rem;
		color: rgba(255, 255, 255, 0.6);
	}
	.training-dot {
		width: 6px; height: 6px; border-radius: 50%;
		background: rgba(124, 128, 255, 0.9);
		box-shadow: 0 0 6px rgba(124, 128, 255, 0.7);
		flex-shrink: 0;
		animation: pulse 1.5s ease-in-out infinite;
	}
	.training-label { flex: 1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.training-bar {
		width: 80px; height: 3px;
		background: rgba(255,255,255,0.08);
		border-radius: 999px;
		overflow: hidden;
		flex-shrink: 0;
	}
	.training-fill {
		height: 100%;
		background: rgba(124, 128, 255, 0.7);
		border-radius: 999px;
		transition: width 0.3s ease;
	}
	.training-count { font-size: 0.65rem; color: rgba(255,255,255,0.35); white-space: nowrap; }

	@keyframes pulse {
		0%, 100% { opacity: 1; }
		50% { opacity: 0.4; }
	}
</style>
