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

export function strategyDescription(value?: string): string {
	const label = strategyLabel(value);
	const descriptions: Record<string, string> = {
		Adaptive: 'Chooses among suitable overlaps, bass handoffs, energy changes and cuts for this pair.',
		Wildcard: 'Favours a less familiar suitable transition while retaining audio safety checks.',
		'Smooth blend': 'A longer overlap gradually trades track levels, favouring compatible harmony and space for both tracks.',
		'Club mix': 'Overlaps rhythmic phrases and hands over the bass. Longer mixes require verified tempo and beat phase.',
		'Quick mix': 'A short overlap brings the incoming track forward quickly and limits competing vocals or rhythms.',
		'Energy lift': 'Keeps the outgoing track prominent during the build, then gives the incoming track and its bass a stronger arrival.',
		'Energy reset': 'Withdraws the outgoing track and its bass earlier, letting a calmer incoming section create breathing room.',
		'Drop swap': 'Introduces the incoming build quietly, then trades track levels and bass at its analysed drop.',
		'Bass swap': 'Lets both tracks overlap rhythmically while removing the outgoing bass before bringing in the incoming bass.',
		'Cut / slam': 'Uses a very short handoff rather than a prolonged overlap, with downbeat timing when analysis supports it.',
		'Drop tease': 'An automatic mid-song preview briefly plays a compatible incoming drop over a safe outgoing phrase. It requires analysed drop and mid-song markers.',
		'Filter sweep': 'Changes tone through a short overlap to clear space for the incoming phrase.',
		'Safe crossfade': 'Trades track levels without assuming beat alignment. It protects playback when a more involved mix cannot be verified.',
	};
	return descriptions[label] ?? 'The chosen audio programme sets the overlap, track levels and incoming entry.';
}

export function transitionExplanation(status: DjStatusResponse | null): string {
	const program = status?.transition_plan;
	const reasons: Record<string, string> = {
		profile_low_confidence: 'The available analysis is not confident enough for a more involved mix.',
		safety_override_safe: 'A saved safe-only correction protects this pair.',
		beat_sync_unverified: 'The decoded overlap did not provide a stable enough beat match for the planned rhythmic mix.',
		lookahead_pair_mismatch: 'The prepared audio belongs to a different queue pair, so playback uses a protected handoff.',
		prepared_mixer_missing: 'The planned mix was not prepared in time, so playback uses the available safe handoff.',
		next_deck_not_decoded: 'Not enough incoming audio was decoded in time for the planned overlap.',
		next_decode_late_at_fire: 'The incoming audio arrived after the planned start.',
		active_deck_not_decoded: 'Not enough outgoing audio was available to prepare the planned overlap.',
		next_deck_missing_at_fire: 'The incoming deck was not ready at the planned start.',
		handoff_seam_too_late: 'The prepared overlap could no longer be joined cleanly, so playback protected the handoff.',
		manual_seek_suppressed: 'A seek cancelled the earlier mix window. The next confirmed handoff will animate when it starts.',
		current_profile_decode_failed: 'Analysis could not decode the outgoing track.',
		next_profile_decode_failed: 'Analysis could not decode the incoming track.',
		missing_current_profile: 'The outgoing track does not yet have a usable DJ profile.',
		missing_next_profile: 'The incoming track does not yet have a usable DJ profile.',
	};
	const decisionReason = program?.decision?.reason;
	// During a handoff, status can already contain the NEXT pair's planning cause.
	// Its cause must not explain the programme currently audible on the old pair.
	if (status?.active_transition) return decisionReason ?? strategyDescription(program?.template);
	const cause = [status?.fallback_reason, status?.downgrade_reason, status?.runtime_renderer_reason, status?.planning_reason]
		.find((reason) => reason && reason !== 'none' && reasons[reason]);
	if (cause && decisionReason) return `${reasons[cause]} ${decisionReason}`;
	if (cause) return reasons[cause];
	if (decisionReason) return decisionReason;
	return strategyDescription(program?.template);
}

export function dropPreviewExplanation(preview?: DjStatusResponse['drop_preview']): string | null {
	if (preview?.status !== 'skipped' || !preview.reason || preview.reason === 'none') return null;
	const reasons: Record<string, string> = {
		no_safe_mid_song_marker: 'No measured safe beat or phrase marker near the middle of the outgoing track is available. Analysis of the opening alone does not map the full song.',
		missing_incoming_drop: 'No usable drop marker was found in the incoming track.',
		harmonic_incompatible: 'The keys are unknown or unsuitable for an overlapping drop preview.',
		safe_crossfade_only: 'A saved safe-only correction prevents an overlapping drop preview.',
		profile_low_confidence: 'The available profile confidence is too low for a drop preview.',
		current_profile_missing: 'The outgoing DJ profile is not available yet.',
		next_profile_missing: 'The incoming DJ profile is not available yet.',
		current_source_unavailable: 'The outgoing audio source is unavailable.',
		next_source_unavailable: 'The incoming audio source is unavailable.',
		beat_sync_unverified: 'The preview audio did not provide a stable enough beat match. The outgoing track keeps playing.',
	};
	return `Drop preview skipped: ${reasons[preview.reason] ?? preview.reason.replaceAll('_', ' ')}`;
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

// A seek invalidates audio already mixing, not an armed event's future fire.
// The runtime deliberately reuses an armed event after seeking within a track.
export function seekBlockedEventId(status: DjStatusResponse | null): number | null {
	if (status?.active_transition) return status.active_transition.event_id;
	return status?.runtime_rendered_dj_mixer && status.runtime_renderer_status === 'rendered_overlay'
		&& ['fired', 'late'].includes(status.timing_status ?? '')
		? status.last_transition_event_id ?? null : null;
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
