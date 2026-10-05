<script lang="ts">
	import type { DjStatusResponse, DjTransitionProgram } from '$lib/api/client';
	import { consumedFrames, durationMs, executionLabel, hasBassAutomation, isPlaybackFallback, parameterAt, sceneMarkers, sourceSeconds, timeLabel, transitionDurationLabel, transitionOriginMs, transitionProgress } from './transition_scene';

	let { status, progress = null, playing = false }: { status: DjStatusResponse | null; progress?: number | null; playing?: boolean } = $props();
	let plan = $derived(status?.transition_plan);
	let liveProgress = $derived(transitionProgress(status, status?.playback_position_ms) == null ? null : progress);
	const sceneId = $props.id();
	let isFallback = $derived(isPlaybackFallback(status));
	let showPlan = $derived(Boolean(plan && durationMs(plan) > 0 && status?.enabled && !isFallback));
	let bassHandoff = $derived(hasBassAutomation(plan) && plan?.template !== 'SafeCrossfade');
	let showGrid = $derived(showPlan && plan?.template !== 'SafeCrossfade');
	let markersA = $derived(plan && showGrid ? sceneMarkers(plan, 'A', status?.current, transitionOriginMs(status)) : []);
	let markersB = $derived(plan && showGrid ? sceneMarkers(plan, 'B', status?.next) : []);
	let energyDelta = $derived(status?.current?.energy != null && status?.next?.energy != null ? status.next.energy - status.current.energy : 0);
	let frame = $derived(plan ? (liveProgress ?? 0) * plan.resolve_at : 0);
	let gainA = $derived(plan ? parameterAt(plan, 'DeckGain', 'A', frame) : 1);
	let gainB = $derived(plan ? parameterAt(plan, 'DeckGain', 'B', frame) : 0);

	// Both lanes share a left-to-right time axis; depth and height express level.
	function point(t: number, deck: 'A' | 'B', gain = 0) {
		const depth = 0.65 + 0.35 * t;
		const lane = deck === 'A' ? -1 : 1;
		return { x: 100 + 720 * t, y: 122 + lane * (38 - 16 * Math.sin(Math.PI * t)) + 12 * t
			- gain * 18 * depth - (deck === 'B' ? energyDelta * 12 * t : 0), depth };
	}

	function laneShape(deck: 'A' | 'B') {
		const edge = (side: number, t: number) => {
			const p = point(t, deck);
			return `${p.x.toFixed(2)},${(p.y + side * 13 * p.depth).toFixed(2)}`;
		};
		return [...Array.from({ length: 49 }, (_, i) => edge(-1, i / 48)), ...Array.from({ length: 49 }, (_, i) => edge(1, 1 - i / 48))].join(' ');
	}

	function envelope(program: DjTransitionProgram, deck: 'A' | 'B', bass = false) {
		return Array.from({ length: 161 }, (_, i) => {
			const t = i / 160;
			const gain = parameterAt(program, 'DeckGain', deck, t * program.resolve_at);
			const prominence = bass ? gain * parameterAt(program, 'LowGain', deck, t * program.resolve_at) : gain;
			const p = point(t, deck, prominence);
			return `${i ? 'L' : 'M'}${p.x.toFixed(2)},${p.y.toFixed(2)}`;
		}).join(' ');
	}

	function exitSeconds(program: DjTransitionProgram) {
		// A starts at the scheduled live position; deck_a_start_frame is often buffer-relative zero.
		const start = transitionOriginMs(status);
		return start == null ? undefined : start / 1000 + consumedFrames(program, 'A', program.resolve_at) / program.sample_rate;
	}
	let description = $derived(plan && showPlan
		? `${liveProgress == null ? 'Scheduled preview' : 'Audible transition'}. ${executionLabel(plan)}. ${(durationMs(plan) / 1000).toFixed(1)} seconds. Incoming entry ${timeLabel(sourceSeconds(plan, 'B', 0))}. A shared time axis runs from start to completion.`
		: 'No active transition. The visual waits for a current audio plan.');
</script>

<div class="scene" class:idle={!showPlan}>
	{#if plan && showPlan}
	<div class="scene-heading"><span>{liveProgress == null ? 'Scheduled preview' : playing ? 'Live audio' : 'Mix paused'}</span><strong>{executionLabel(plan)}</strong></div>
	<svg viewBox="0 0 900 242" role="img" aria-label="Spatial transition lanes" aria-describedby={`${sceneId}-description`}>
		<title>Outgoing music flows into incoming music</title>
		<desc id={`${sceneId}-description`}>{description}</desc>
		<defs>
			<linearGradient id={`${sceneId}-depth`} x1="0" y1="0" x2="1" y2="0"><stop offset="0%" stop-color="currentColor" stop-opacity="0.08" /><stop offset="100%" stop-color="currentColor" stop-opacity="0.27" /></linearGradient>
		</defs>
		{#each [0.2, 0.4, 0.6, 0.8, 1] as depth}
			{@const left = point(depth, 'A')}
			{@const right = point(depth, 'B')}
			<path class="floor-line" d={`M${left.x},${left.y - 20} L${right.x},${right.y + 20}`} />
		{/each}
		<polygon class="lane lane-a" points={laneShape('A')} fill={`url(#${sceneId}-depth)`} />
		<polygon class="lane lane-b" points={laneShape('B')} fill={`url(#${sceneId}-depth)`} />
		<text class="lane-label" x="18" y="88">Outgoing</text>
		<text class="lane-label incoming-label" x="18" y="164">Incoming</text>
		{#if plan}
			{@const handoff = plan.swap_start / plan.resolve_at}
			{@const swapA = point(handoff, 'A')}
			{@const swapB = point(handoff, 'B')}
			{#if bassHandoff}<path class="handoff" d={`M${swapA.x},${swapA.y} Q${swapA.x + 28},122 ${swapB.x},${swapB.y}`} />{/if}
			{#each [{ deck: 'A' as const, markers: markersA }, { deck: 'B' as const, markers: markersB }] as lane}
				{#each lane.markers as marker}
					{@const p = point(marker.fraction, lane.deck)}
					<line class={`marker ${marker.kind}`} x1={p.x} x2={p.x} y1={p.y - 11 * p.depth} y2={p.y + 11 * p.depth} />
				{/each}
				<path class={`envelope deck-${lane.deck}`} d={envelope(plan, lane.deck)} />
				{#if hasBassAutomation(plan, lane.deck)}<path class={`bass-envelope deck-${lane.deck}`} d={envelope(plan, lane.deck, true)} />{/if}
			{/each}
			{#if liveProgress != null}
				{@const a = point(liveProgress, 'A', gainA)}
				{@const b = point(liveProgress, 'B', gainB)}
				<path class="playhead" d={`M${a.x},${a.y - 18} L${b.x},${b.y + 18}`} />
				<circle class="cursor deck-A" cx={a.x} cy={a.y} r={4 + 5 * gainA} />
				<circle class="cursor deck-B" cx={b.x} cy={b.y} r={4 + 5 * gainB} />
			{/if}
		<path class="time-axis" d="M100,210 L820,210" />
		<text class="axis-label" x="100" y="231">Start · 0s</text>
		{#if bassHandoff}<text class="axis-label" x={swapA.x} y="231" text-anchor="middle">Bass handoff · {(plan.swap_start / plan.sample_rate).toFixed(1)}s</text>{/if}
		<text class="axis-label" x="820" y="231" text-anchor="end">Finish · {transitionDurationLabel(plan)}</text>
		{/if}
	</svg>
		<div class="scene-facts">
			<span>Outgoing exit <strong>{timeLabel(exitSeconds(plan))}</strong></span>
			<span>{plan.template === 'SlamCut' ? 'Cut duration' : 'Overlap'} <strong>{transitionDurationLabel(plan)}</strong></span>
			<span>Incoming entry <strong>{timeLabel(sourceSeconds(plan, 'B', 0))}</strong></span>
		</div>
		<div class="legend" aria-label="Transition visual legend">
			<span class="legend-gain">Track level</span>{#if hasBassAutomation(plan)}<span class="legend-bass">Bass level</span>{/if}
			{#if markersA.length || markersB.length}<span>Ticks: beat · downbeat · phrase</span>{/if}
			{#if showGrid && !markersA.length}<span>Outgoing beat grid unavailable here</span>{/if}
		</div>
	{:else}
		<p class="scene-empty">{isFallback ? 'Playback is using a safe fallback. No planned mix animation is shown.' : status?.enabled === false ? 'Turn DJ on to plan the next transition.' : 'Waiting for the next transition plan.'}</p>
	{/if}
</div>

<style>
	.scene { position: relative; overflow: hidden; border-radius: var(--radius-lg); background: radial-gradient(ellipse at 50% 65%, var(--accent-soft), transparent 64%), var(--bg-base); border: 1px solid var(--border-subtle); }
	.scene-heading { display: flex; justify-content: space-between; align-items: center; gap: var(--space-3); padding: var(--space-3) var(--space-4) 0; font-size: var(--font-size-xs); color: var(--text-tertiary); }
	.scene-heading strong { color: var(--text-secondary); font-weight: var(--font-weight-medium); }
	svg { display: block; width: 100%; height: auto; max-height: 18rem; }
	.floor-line { stroke: var(--border-subtle); stroke-width: 1; fill: none; }
	.lane { stroke-width: 1; }
	.lane-label, .axis-label { fill: var(--text-secondary); font-size: var(--font-size-xs); }
	.incoming-label { fill: var(--accent); }
	.axis-label { fill: var(--text-tertiary); }
	.time-axis { stroke: var(--border-muted); stroke-width: 1; fill: none; }
	.lane-a { color: var(--text-secondary); stroke: var(--border-muted); }
	.lane-b { color: var(--accent); stroke: var(--accent-line); }
	.envelope, .bass-envelope, .handoff, .playhead { fill: none; }
	.envelope { stroke-width: 3; stroke-linecap: round; }
	.deck-A { stroke: var(--text-secondary); }
	.deck-B { stroke: var(--accent); }
	.bass-envelope { stroke-width: 1.4; stroke-dasharray: 4 5; opacity: 0.6; }
	.handoff { stroke: var(--accent-line); stroke-width: 2; stroke-dasharray: 3 4; }
	.marker { stroke: var(--border-muted); stroke-width: 1; }
	.downbeat { stroke: var(--text-secondary); stroke-width: 1.5; }
	.phrase { stroke: var(--state-warning); stroke-width: 3; }
	.drop { stroke: var(--state-success); stroke-width: 4; }
	.playhead { stroke: var(--text-primary); stroke-width: 1.2; opacity: 0.5; }
	.cursor { fill: var(--bg-base); stroke-width: 3; }
	.scene-facts { display: flex; justify-content: space-between; gap: var(--space-3); padding: var(--space-3) var(--space-4); border-top: 1px solid var(--border-subtle); color: var(--text-tertiary); font-size: var(--font-size-xs); }
	.scene-facts strong { display: block; color: var(--text-primary); font-size: var(--font-size-md); font-weight: var(--font-weight-semibold); line-height: var(--line-height-normal); }
	.legend { padding: 0 var(--space-4) var(--space-3); display: flex; flex-wrap: wrap; gap: var(--space-3); font-size: var(--font-size-2xs); color: var(--text-tertiary); }
	.legend-gain::before, .legend-bass::before { content: ''; display: inline-block; vertical-align: middle; width: 1.4rem; margin-right: var(--space-1); border-top: 2px solid var(--accent); }
	.legend-bass::before { border-top-style: dashed; }
	.scene-empty { margin: 0; padding: var(--space-5) var(--space-4); color: var(--text-secondary); font-size: var(--font-size-sm); line-height: var(--line-height-snug); }
	@media (max-width: 760px) { .scene-facts { gap: var(--space-2); padding: var(--space-3); } .legend { padding-inline: var(--space-3); } }
	@media (prefers-reduced-motion: reduce) { .cursor { transition: none; } }
</style>
