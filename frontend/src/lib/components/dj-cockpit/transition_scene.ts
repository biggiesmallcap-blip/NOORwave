import type { DjAutomationEvent, DjDeckStatus, DjStatusResponse, DjTransitionProgram } from '$lib/api/client';

export const strategyLabels: Record<string, string> = {
	adaptive: 'Adaptive', wildcard: 'Wildcard', smooth_blend: 'Smooth blend', club_mix: 'Club mix',
	quick_mix: 'Quick mix', energy_lift: 'Energy lift', energy_reset: 'Energy reset',
	drop_swap: 'Drop swap', bass_swap: 'Bass swap', cut: 'Cut / slam',
	SafeCrossfade: 'Safe crossfade', LongHarmonicBlend: 'Smooth blend', ClubMix: 'Club mix',
	QuickMix: 'Quick mix', EnergyLift: 'Energy lift', EnergyReset: 'Energy reset', DropSwap: 'Drop swap',
	BassSwap16: 'Bass swap', BassSwap32: 'Bass swap', SlamCut: 'Cut / slam',
	DropTease16: 'Drop tease', FilterSweep: 'Filter sweep',
};

export function strategyLabel(value?: string) {
	return value ? strategyLabels[value] ?? value.replaceAll('_', ' ') : 'Finding the next mix';
}

export function curveFraction(curve: DjAutomationEvent['curve'], fraction: number) {
	const t = Math.max(0, Math.min(1, fraction));
	if (curve === 'EqualPowerIn') return Math.sin(t * Math.PI / 2);
	if (curve === 'EqualPowerOut') return 1 - Math.cos(t * Math.PI / 2);
	if (curve === 'Cosine') return (1 - Math.cos(t * Math.PI)) / 2;
	return t;
}

// Match noor-mix's ordered event fold, including held endpoints and unity defaults.
export function parameterAt(program: DjTransitionProgram, param: keyof DjAutomationEvent['param'], deck: 'A' | 'B', frame: number) {
	return program.automation.reduce((value, event) => {
		if (event.param[param] !== deck || frame < event.start_sample) return value;
		if (frame >= event.end_sample) return event.to;
		const t = (frame - event.start_sample) / (event.end_sample - event.start_sample);
		return event.from + (event.to - event.from) * curveFraction(event.curve, t);
	}, 1);
}

export function durationMs(program?: DjTransitionProgram) {
	return program && program.sample_rate > 0 ? program.resolve_at / program.sample_rate * 1000 : 0;
}

export function transitionDurationLabel(program?: DjTransitionProgram) {
	const ms = durationMs(program);
	return ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
}

export function consumedFrames(program: DjTransitionProgram, deck: 'A' | 'B', outputFrame: number) {
	const end = Math.max(0, outputFrame);
	const points = [0, end, ...program.automation
		.filter((event) => event.param.PlaybackRate === deck)
		.flatMap((event) => [event.start_sample, event.end_sample])
		.filter((frame) => frame > 0 && frame < end)].sort((a, b) => a - b);
	return points.slice(1).reduce((sum, point, index) =>
		sum + (point - points[index]) * parameterAt(program, 'PlaybackRate', deck, points[index]), 0);
}

export function sourceSeconds(program: DjTransitionProgram, deck: 'A' | 'B', outputFrame: number) {
	const start = deck === 'A' ? program.deck_a_start_frame : program.deck_b_start_frame;
	let sourceFrame = start + consumedFrames(program, deck, outputFrame);
	const loop = program.loops.find((region) => region.deck === deck && region.end_frame > region.start_frame);
	if (loop && sourceFrame >= loop.end_frame) {
		sourceFrame = loop.start_frame + (sourceFrame - loop.start_frame) % (loop.end_frame - loop.start_frame);
	}
	return sourceFrame / program.sample_rate;
}

export type SceneMarker = { fraction: number; kind: 'beat' | 'downbeat' | 'phrase' | 'drop' };

export function sceneMarkers(program: DjTransitionProgram, deck: 'A' | 'B', profile?: DjDeckStatus, outgoingStartMs?: number): SceneMarker[] {
	if (!profile || !program.sample_rate || !program.resolve_at) return [];
	// Looped previews require repeated marker mapping; omit markers rather than invent timing.
	if (program.loops.some((region) => region.deck === deck)) return [];
	const sourceOffset = deck === 'A' && outgoingStartMs != null ? outgoingStartMs - sourceSeconds(program, deck, 0) * 1000 : 0;
	const start = sourceSeconds(program, deck, 0) * 1000 + sourceOffset;
	const end = sourceSeconds(program, deck, program.resolve_at) * 1000 + sourceOffset;
	const coverage = profile.analysis_scope_ms ?? profile.beat_markers_ms.at(-1) ?? 0;
	const markers = new Map<number, SceneMarker['kind']>();
	const trustedGrid = !profile.grid_is_synthetic && (profile.beat_confidence ?? 0) >= 0.55;
	for (const [kind, values] of [
		['beat', trustedGrid ? profile.beat_markers_ms : []], ['downbeat', trustedGrid ? profile.downbeat_markers_ms : []],
		['phrase', trustedGrid ? profile.phrase_markers_ms : []], ['drop', trustedGrid ? profile.drop_markers_ms : []],
	] as const) {
		for (const ms of values) if (ms >= start && ms <= end && ms <= coverage) markers.set(ms, kind);
	}
	for (const ms of profile.manual_drop_markers_ms) if (ms >= start && ms <= end) markers.set(ms, 'drop');
	return [...markers].sort((a, b) => a[0] - b[0]).slice(0, 96).map(([ms, kind]) => {
		// Invert source position with rate changes included, using bounded bisection.
		let low = 0, high = program.resolve_at;
		for (let i = 0; i < 24; i++) {
			const middle = (low + high) / 2;
			if (sourceSeconds(program, deck, middle) * 1000 + sourceOffset < ms) low = middle;
			else high = middle;
		}
		return { fraction: (low + high) / (2 * program.resolve_at), kind };
	});
}

export function transitionProgress(status: DjStatusResponse | null, positionMs?: number) {
	const plan = status?.transition_plan;
	if (!status?.enabled || !plan || status.actual_start_ms == null
		|| !['fired', 'late'].includes(status.timing_status ?? '')
		|| !status.runtime_rendered_dj_mixer
		|| !['rendered_handoff', 'rendered_overlay'].includes(status.runtime_renderer_status ?? '')
		|| status.runtime_renderer_reason === 'manual_seek_suppressed') return null;
	if (!status.active_transition && status.runtime_renderer_status !== 'rendered_overlay') return null;
	const duration = durationMs(plan);
	// A handoff's output clock already includes frames skipped at a late join.
	const elapsed = status.active_transition?.elapsed_ms
		?? (positionMs == null ? NaN : positionMs - (status.runtime_planned_start_ms ?? status.planned_start_ms ?? status.actual_start_ms));
	return duration > 0 && Number.isFinite(elapsed) && elapsed >= 0 && elapsed < duration ? elapsed / duration : null;
}

export function hasBassAutomation(program?: DjTransitionProgram, deck?: 'A' | 'B') {
	return Boolean(program?.automation.some((event) => event.param.LowGain != null
		&& (deck == null || event.param.LowGain === deck) && (event.from !== 1 || event.to !== 1)));
}

export function executionLabel(program?: DjTransitionProgram) {
	if (!program) return 'Waiting for a plan';
	if (program.template === 'SlamCut') return 'Clean cut';
	if (program.template === 'SafeCrossfade') return hasBassAutomation(program) ? 'Gain crossfade + bass duck' : 'Gain crossfade';
	if (hasBassAutomation(program)) return 'Gain + bass handoff';
	return 'Gain crossfade';
}

export function isPlaybackFallback(status: DjStatusResponse | null) {
	return status?.renderer_mode === 'legacy_overlap' || status?.runtime_renderer_status === 'legacy_overlap'
		|| status?.runtime_renderer_status === 'boundary_fallback';
}

export function transitionOriginMs(status: DjStatusResponse | null) {
	return status?.active_transition?.start_ms ?? status?.runtime_planned_start_ms
		?? status?.planned_start_ms ?? status?.actual_start_ms;
}

type PlayingTrack = { id: number; tidal_id: number | null };
function trackMatches(deck: DjDeckStatus | undefined, track: PlayingTrack | null) {
	return Boolean(deck && track && (
		(deck.media_ref_kind === 'library_track' && deck.media_ref_id === String(track.id)) ||
		(deck.media_ref_kind === 'tidal_track' && deck.media_ref_id === String(track.tidal_id))
	));
}

// A rendered handoff promotes the queue at fire; preserve its audible original pair.
export function storySnapshot(status: DjStatusResponse | null, track: PlayingTrack | null, localPositionMs: number, blockedEventId?: number | null) {
	if (!status) return null;
	const active = status.active_transition;
	const localDelta = status.playback_position_ms == null ? NaN : localPositionMs - status.playback_position_ms;
	// Interpolate only a fresh, identity-matched output clock, never a scrubber jump.
	if (status.enabled && active && active.event_id !== blockedEventId && trackMatches(active.incoming, track)
		&& Number.isFinite(localDelta) && localDelta >= -750 && localDelta <= 2500) {
		const incomingRate = parameterAt(active.program, 'PlaybackRate', 'B', active.elapsed_ms / 1000 * active.program.sample_rate);
		if (!Number.isFinite(incomingRate) || incomingRate <= 0) return { ...status, active_transition: undefined };
		const elapsed = active.elapsed_ms + Math.max(0, localDelta) / incomingRate;
		if (elapsed < 0 || elapsed >= durationMs(active.program)) return { ...status, active_transition: undefined };
		return { ...status, current: active.outgoing, next: active.incoming, transition_plan: active.program,
			active_transition: { ...active, elapsed_ms: elapsed },
			planned_start_ms: active.start_ms, actual_start_ms: active.actual_start_ms ?? active.start_ms,
			playback_position_ms: active.start_ms + elapsed, timing_status: 'fired', renderer_template: active.program.template,
			renderer_mode: (active.program.tier === 'FullBlend' ? 'dj_full_program' : 'dj_gain_program') as DjStatusResponse['renderer_mode'],
			runtime_rendered_dj_mixer: true, runtime_renderer_status: 'rendered_handoff', runtime_renderer_reason: 'none', planning_status: 'mixing' };
	}
	if (!trackMatches(status.current, track)) return { ...status, current: undefined, next: undefined,
		active_transition: undefined, transition_plan: undefined, selected_program: undefined, renderer_template: undefined,
		actual_start_ms: undefined, planned_start_ms: undefined, playback_position_ms: undefined, runtime_rendered_dj_mixer: false };
	if (blockedEventId != null && status.last_transition_event_id === blockedEventId) return { ...status,
		active_transition: undefined, actual_start_ms: undefined, timing_status: undefined,
		runtime_rendered_dj_mixer: false, playback_position_ms: localPositionMs };
	return { ...status, active_transition: undefined, playback_position_ms: localPositionMs };
}

export function energyLabel(status: DjStatusResponse | null) {
	const direction = status?.transition_plan?.decision?.energy_direction;
	if (direction === 'lift' || direction === 'up' || direction === 'rising') return 'Energy rises';
	if (direction === 'reset' || direction === 'down' || direction === 'falling') return 'Room to breathe';
	const a = status?.current?.energy, b = status?.next?.energy;
	if (a == null || b == null) return 'Energy not analysed';
	if (b - a > 0.1) return 'Energy rises';
	if (a - b > 0.1) return 'Room to breathe';
	return 'Steady energy';
}

export function timeLabel(seconds?: number) {
	if (seconds == null || !Number.isFinite(seconds)) return 'Pending';
	const safe = Math.max(0, Math.round(seconds));
	return `${Math.floor(safe / 60)}:${String(safe % 60).padStart(2, '0')}`;
}
