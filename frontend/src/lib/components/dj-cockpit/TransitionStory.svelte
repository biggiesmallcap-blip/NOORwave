<script lang="ts">
	import type { DjStatusResponse } from '$lib/api/client';
	import { currentTrack, isPlaying, playbackSeekRevision, position } from '$lib/stores/player';
	import TransitionScene from './TransitionScene.svelte';
	import { energyLabel, executionLabel, hasBassAutomation, isPlaybackFallback, storySnapshot, strategyLabel, timeLabel, transitionDurationLabel, transitionOriginMs, transitionProgress } from './transition_scene';

	let { status, compact = false, enabled = null }: { status: DjStatusResponse | null; compact?: boolean; enabled?: boolean | null } = $props();
	let lastSeekRevision = $state($playbackSeekRevision);
	let blockedEventId = $state<number | null>(null);
	$effect(() => {
		if ($playbackSeekRevision !== lastSeekRevision) {
			blockedEventId = status?.active_transition?.event_id ?? status?.last_transition_event_id ?? null;
			lastSeekRevision = $playbackSeekRevision;
		}
	});
	let shown = $derived(storySnapshot(status, $currentTrack, $position, blockedEventId));
	let positionMs = $derived(shown?.playback_position_ms);
	let plan = $derived(shown?.transition_plan);
	let progress = $derived(transitionProgress(shown, positionMs));
	let startMs = $derived(transitionOriginMs(shown));
	let countdown = $derived(startMs != null && positionMs != null ? Math.max(0, (startMs - positionMs) / 1000) : null);
	let fallback = $derived(isPlaybackFallback(shown));
	let label = $derived(fallback ? 'Safe playback fallback' : strategyLabel(plan?.template ?? shown?.renderer_template ?? shown?.selected_program));
	let confidence = $derived(plan?.decision?.confidence);
	let decision = $derived(plan?.decision);
	let timingCopy = $derived((status?.enabled ?? enabled) === false ? 'Turn DJ on to shape the next transition'
		: shown?.timing_status === 'missed' || shown?.runtime_renderer_reason === 'manual_seek_suppressed' ? 'Waiting for the next opportunity'
		: !status ? 'Waiting for DJ information'
		: fallback ? 'Safe playback fallback'
		: !plan ? 'Listening for a suitable transition'
		: progress != null ? `${$isPlaying ? 'Mixing now' : 'Mix paused'} · ${Math.round(progress * 100)}%`
		: countdown != null && countdown > 0
			? `${shown?.timing_source === 'fallback_overlap' && shown?.runtime_planned_start_ms == null ? 'Estimated' : 'Scheduled'} in ${Math.ceil(countdown)}s`
			: 'Scheduled · waiting for audio');
	let safeExplanation = $derived(status?.planning_reason === 'profile_low_confidence'
		? 'A dependable crossfade keeps this pair flowing while analysis is limited.'
		: status?.planning_reason === 'safety_override_safe'
			? 'Your safe-only correction protects this pair.'
			: plan?.template === 'SafeCrossfade' ? `This is a gain crossfade${hasBassAutomation(plan) ? ' with a modest bass duck' : ''}. It does not assume beat alignment or a bass swap.`
				: hasBassAutomation(plan) ? 'The audio program controls both track levels and the bass handoff.'
					: 'The audio program controls the track levels and incoming entry.');
</script>

<section class="transition-story" class:compact aria-label="Outgoing to incoming transition">
	<div class="track-flow">
		<div class="track-copy outgoing">
			<span class="overline">Outgoing track</span>
			<h2 title={shown?.current?.title ?? $currentTrack?.title}>{shown?.current?.title ?? $currentTrack?.title ?? 'Choose a track'}</h2>
			<p>{shown?.current?.artist ?? $currentTrack?.artist_name ?? 'Music starts here'}</p>
		</div>
		<div class="mix-copy">
			<span class="flow-arrow" aria-hidden="true">→</span>
			<strong>{label}</strong>
			<span>{plan ? `${transitionDurationLabel(plan)} · ${executionLabel(plan)}` : 'Adaptive selection'}</span>
		</div>
		<div class="track-copy incoming">
			<span class="overline">Incoming track</span>
			<h2 title={shown?.next?.title}>{shown?.next?.title ?? 'Waiting for the queue'}</h2>
				<p>{shown?.next?.artist ?? 'The next track joins here'}</p>
		</div>
	</div>
	<div class="story-strip">
		<span class="live-state" class:mixing={progress != null && $isPlaying}>{timingCopy}</span>
		<span>{energyLabel(shown)}</span>
		{#if confidence != null}<span>{Math.round(confidence * 100)}% plan confidence</span>{/if}
	</div>
	{#if !compact}<TransitionScene status={shown} progress={fallback ? null : progress} playing={$isPlaying} />{/if}
	{#if progress != null && !fallback}
		<progress max="1" value={progress ?? 0} aria-label="Transition progress"></progress>
	{/if}
	{#if !compact}
		<details class="why">
			<summary>Why this transition?</summary>
			<p>{decision?.reason ?? safeExplanation}</p>
			{#if decision}
				<div class="reason-facts">
					<span>{decision.duration_beats.toFixed(0)} beats</span>
					<span>Entry {timeLabel(decision.incoming_entry_seconds)}</span>
					<span>{decision.outgoing_window.replaceAll('_', ' ')}</span>
				</div>
				{#if decision.candidates.length > 0}
					<details class="candidate-details">
						<summary>Compare suitable choices</summary>
						<ul>
							{#each decision.candidates as candidate}
								<li>
									<div><strong>{strategyLabel(candidate.strategy)}</strong><span>Score {candidate.score.toFixed(2)} · {candidate.duration_seconds.toFixed(1)}s</span></div>
									<p>{candidate.reason}</p>
									<div class="score-components">{#each candidate.components as component}<span>{component.name.replaceAll('_', ' ')} {component.value.toFixed(2)} × {component.weight.toFixed(2)}</span>{/each}</div>
								</li>
							{/each}
						</ul>
					</details>
				{/if}
			{/if}
		</details>
	{/if}
</section>

<style>
	.transition-story { display: grid; gap: var(--space-3); padding: var(--space-5); border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: var(--bg-elevated); }
	.track-flow { display: grid; grid-template-columns: minmax(0, 1fr) minmax(8rem, 0.7fr) minmax(0, 1fr); gap: var(--space-4); align-items: center; }
	.track-copy { min-width: 0; }
	.overline { color: var(--text-tertiary); font-size: var(--font-size-2xs); font-weight: var(--font-weight-semibold); text-transform: uppercase; }
	h2 { margin: var(--space-2) 0 var(--space-1); font-size: var(--font-size-xl); line-height: var(--line-height-snug); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	p { margin: 0; color: var(--text-secondary); font-size: var(--font-size-sm); line-height: var(--line-height-normal); }
	.incoming { text-align: right; }
	.mix-copy { display: grid; justify-items: center; gap: var(--space-1); text-align: center; }
	.mix-copy strong { font-size: var(--font-size-md); font-weight: var(--font-weight-semibold); color: var(--accent-strong); }
	.mix-copy > span:last-child { font-size: var(--font-size-xs); color: var(--text-tertiary); }
	.flow-arrow { color: var(--accent); font-size: var(--font-size-2xl); line-height: 1; }
	.story-strip { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space-3); padding-top: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-xs); }
	.live-state { margin-right: auto; }
	.live-state::before { content: ''; display: inline-block; width: 0.45rem; height: 0.45rem; border-radius: 50%; margin-right: var(--space-2); background: var(--text-tertiary); }
	.mixing::before { background: var(--state-success); }
	progress { width: 100%; height: 3px; border: 0; border-radius: 999px; background: var(--bg-raised); accent-color: var(--accent); }
	progress::-webkit-progress-bar { background: var(--bg-raised); border-radius: 999px; }
	progress::-webkit-progress-value { background: var(--accent); border-radius: 999px; }
	summary { cursor: pointer; padding: var(--space-2) 0; color: var(--text-secondary); font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); }
	summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; border-radius: var(--radius-xs); }
	.why { border-top: 1px solid var(--border-subtle); }
	.why > p { padding: var(--space-2) 0; }
	.reason-facts, .score-components { display: flex; flex-wrap: wrap; gap: var(--space-2); font-size: var(--font-size-xs); color: var(--text-tertiary); }
	.candidate-details { margin-top: var(--space-3); }
	ul { list-style: none; padding: 0; margin: var(--space-2) 0 0; display: grid; gap: var(--space-3); }
	li { padding: var(--space-3); background: var(--bg-surface); border-radius: var(--radius-sm); }
	li > div:first-child { display: flex; justify-content: space-between; gap: var(--space-2); font-size: var(--font-size-xs); }
	li > div:first-child span { color: var(--text-tertiary); }
	li p { margin: var(--space-2) 0; font-size: var(--font-size-xs); }
	.compact { padding: var(--space-4); }
	@media (max-width: 760px) { .transition-story { padding: var(--space-3); } .track-flow { gap: var(--space-2); grid-template-columns: minmax(0, 1fr) minmax(6rem, 0.65fr) minmax(0, 1fr); } h2 { font-size: var(--font-size-md); } .mix-copy strong { font-size: var(--font-size-sm); } }
</style>
