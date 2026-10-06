import { describe, expect, test } from 'vitest';
import type { DjDeckStatus, DjStatusResponse, DjTransitionProgram } from '$lib/api/client';
import { curveFraction, dropPreviewExplanation, executionLabel, hasBassAutomation, parameterAt, sourceSeconds, sceneMarkers, seekBlockedEventId, storySnapshot, strategyDescription, transitionExplanation, transitionOriginMs, transitionProgress } from './transition_scene';

const program: DjTransitionProgram = {
	template: 'ClubMix', tier: 'FullBlend', sample_rate: 1000, channels: 2,
	deck_a_start_frame: 0, deck_b_start_frame: 2000, sync_start: 0, intro_start: 0,
	swap_start: 5000, fade_start: 5000, resolve_at: 10000, loops: [],
	automation: [
		{ param: { DeckGain: 'A' }, start_sample: 0, end_sample: 10000, from: 1, to: 0, curve: 'EqualPowerOut' },
		{ param: { DeckGain: 'B' }, start_sample: 0, end_sample: 10000, from: 0, to: 1, curve: 'EqualPowerIn' },
		{ param: { PlaybackRate: 'B' }, start_sample: 0, end_sample: 10000, from: 1.02, to: 1.02, curve: 'Linear' }
	]
};

function deck(id: number): DjDeckStatus {
	return { media_ref_kind: 'library_track', media_ref_id: String(id), title: `Track ${id}`,
		profile_ready: true, profile_status: 'ready', waveform_status: 'ready', waveform_peaks: [],
		beat_markers_ms: [2000, 3000, 4000], downbeat_markers_ms: [2000], phrase_markers_ms: [2000],
		drop_markers_ms: [], manual_drop_markers_ms: [], mix_in_markers_ms: [], mix_out_markers_ms: [],
		analysis_scope_ms: 90000, beat_confidence: 0.9, safe_crossfade_only: false };
}

function activeStatus(): DjStatusResponse {
	return { ...status(), current: deck(2), next: deck(3), playback_position_ms: 24000,
		active_transition: { event_id: 7, outgoing: deck(1), incoming: deck(2), program,
			start_ms: 100000, actual_start_ms: 100368, elapsed_ms: 4000 } };
}

function status(): DjStatusResponse {
	return { enabled: true, current: deck(1), next: deck(2), planning_status: 'armed', transition_plan: program,
		planned_start_ms: 100000, timing_quality: 'unknown', timing_direction: 'pending', rejected_alternatives: [],
		profile_confidence_floor: 0.65, recent_timing_events: [], drop_preview: { status: 'skipped' },
		timing_history_summary: { event_count: 0, tight_count: 0, usable_count: 0, loose_count: 0, bad_count: 0, late_count: 0, missed_count: 0 } };
}

describe('the visual represents the audio program', () => {
	test('skipped previews explain actual marker, harmonic, analysis and source limitations', () => {
		expect(dropPreviewExplanation({ status: 'skipped', reason: 'no_safe_mid_song_marker' })).toContain('opening alone does not map the full song');
		expect(dropPreviewExplanation({ status: 'skipped', reason: 'missing_incoming_drop' })).toContain('incoming track');
		expect(dropPreviewExplanation({ status: 'skipped', reason: 'harmonic_incompatible' })).toContain('unknown or unsuitable');
		for (const reason of ['safe_crossfade_only', 'profile_low_confidence', 'current_profile_missing', 'next_profile_missing', 'current_source_unavailable', 'next_source_unavailable']) {
			expect(dropPreviewExplanation({ status: 'skipped', reason })).toMatch(/^Drop preview skipped:/);
			expect(dropPreviewExplanation({ status: 'skipped', reason })).not.toContain(reason);
		}
		expect(dropPreviewExplanation({ status: 'armed', reason: 'no_safe_mid_song_marker' })).toBeNull();
		expect(dropPreviewExplanation({ status: 'fired' })).toBeNull();
		expect(dropPreviewExplanation(undefined)).toBeNull();
	});
	test('a seek blocks only audio already executed, preserving an armed event future fire', () => {
		const armed = { ...status(), last_transition_event_id: 7, timing_status: 'armed' };
		expect(seekBlockedEventId(armed)).toBeNull();
		const live = activeStatus();
		expect(seekBlockedEventId(live)).toBe(7);
		const shown = storySnapshot(live, { id: 2, tidal_id: null }, 24000, seekBlockedEventId(armed));
		expect(transitionProgress(shown)).toBe(0.4);
		expect(seekBlockedEventId({ ...armed, timing_status: 'fired', runtime_rendered_dj_mixer: true,
			runtime_renderer_status: 'rendered_overlay' })).toBe(7);
	});
	test('TIDAL identity matches the promoted library track independently of library ID', () => {
		const live = activeStatus();
		live.active_transition!.incoming = { ...deck(2), media_ref_kind: 'tidal_track', media_ref_id: '91002' };
		const shown = storySnapshot(live, { id: 2, tidal_id: 91002 }, 24000);
		expect(shown?.next?.media_ref_id).toBe('91002');
		expect(transitionProgress(shown)).toBe(0.4);
		expect(storySnapshot(live, { id: 2, tidal_id: 91003 }, 24000)?.active_transition).toBeUndefined();
	});
	test('safety causes are explained and active audio ignores the next pair planning cause', () => {
		const safe = { ...program, template: 'SafeCrossfade' };
		expect(transitionExplanation({ ...status(), transition_plan: safe,
			fallback_reason: 'beat_sync_unverified' })).toContain('decoded overlap');
		expect(transitionExplanation({ ...status(), runtime_renderer_status: 'legacy_overlap',
			runtime_renderer_reason: 'lookahead_pair_mismatch' })).toContain('different queue pair');
		const shown = storySnapshot({ ...activeStatus(), fallback_reason: 'missing_next_profile' },
			{ id: 2, tidal_id: null }, 24000);
		expect(transitionExplanation(shown)).toBe(strategyDescription('ClubMix'));
		expect(strategyDescription('bass_swap')).toContain('outgoing bass');
		expect(strategyDescription('drop_swap')).toContain('analysed drop');
	});
	test('equal-power curves agree with the mixer and held events retain their endpoints', () => {
		expect(curveFraction('EqualPowerOut', 0.5)).toBeCloseTo(1 - Math.SQRT1_2);
		const a = parameterAt(program, 'DeckGain', 'A', 5000), b = parameterAt(program, 'DeckGain', 'B', 5000);
		expect(a * a + b * b).toBeCloseTo(1);
		expect(parameterAt(program, 'DeckGain', 'A', 20000)).toBe(0);
		expect(parameterAt(program, 'LowGain', 'A', 5000)).toBe(1);
	});
	test('incoming marker positions include the real source cue and playback rate', () => {
		expect(sourceSeconds(program, 'B', 10000)).toBeCloseTo(12.2);
		const markers = sceneMarkers(program, 'B', deck(2));
		expect(markers[0].kind).toBe('phrase');
		expect(markers[1].fraction).toBeCloseTo(0.1 / 1.02);
		expect(sceneMarkers(program, 'A', deck(1), 100000)).toEqual([]);
	});
	test('seeking past an armed start never fires the visual or fabricates completion', () => {
		expect(transitionProgress(null, 0)).toBeNull();
		expect(transitionProgress(status(), 105000)).toBeNull();
		expect(transitionProgress(status(), 190000)).toBeNull();
		expect(transitionProgress({ ...status(), timing_status: 'missed' }, 105000)).toBeNull();
	});
	test('queue promotion keeps the original audible pair with incoming output progress', () => {
		const s = activeStatus();
		const shown = storySnapshot(s, { id: 2, tidal_id: null }, 25020);
		expect(shown?.current?.media_ref_id).toBe('1');
		expect(shown?.next?.media_ref_id).toBe('2');
		expect(transitionProgress(shown, shown?.playback_position_ms)).toBe(0.5);
		expect(shown?.actual_start_ms).toBe(100368);
		expect(transitionOriginMs(shown)).toBe(100000);
		const staleTrack = storySnapshot(s, { id: 9, tidal_id: null }, 9000);
		expect(staleTrack?.active_transition).toBeUndefined();
		expect(staleTrack?.current).toBeUndefined();
		expect(transitionProgress(staleTrack, staleTrack?.playback_position_ms)).toBeNull();
	});
	test('large position jumps and accepted seek revisions clear the cached active event', () => {
		const s = activeStatus();
		for (const localPosition of [9000, 50000]) {
			const shown = storySnapshot(s, { id: 2, tidal_id: null }, localPosition);
			expect(shown?.active_transition).toBeUndefined();
			expect(shown?.current?.media_ref_id).toBe('2');
			expect(transitionProgress(shown, shown?.playback_position_ms)).toBeNull();
		}
		const sought = storySnapshot(s, { id: 2, tidal_id: null }, 24100, 7);
		expect(sought?.active_transition).toBeUndefined();
		expect(transitionProgress(sought, sought?.playback_position_ms)).toBeNull();
	});
	test('handoff output progress ends instead of keeping an old pair at 100 percent', () => {
		const s = activeStatus();
		s.active_transition!.elapsed_ms = 9500;
		const shown = storySnapshot(s, { id: 2, tidal_id: null }, 25020);
		expect(shown?.current?.media_ref_id).toBe('2');
		expect(shown?.next?.media_ref_id).toBe('3');
		expect(transitionProgress(shown, shown?.playback_position_ms)).toBeNull();
		expect(storySnapshot({ ...s, enabled: false }, { id: 2, tidal_id: null }, 24000)?.active_transition).toBeUndefined();
	});
	test('an overlay advances only after its actual rendered fire and never during a fallback', () => {
		const s = { ...status(), actual_start_ms: 100368, runtime_planned_start_ms: 100000,
			timing_status: 'fired', runtime_rendered_dj_mixer: true, runtime_renderer_status: 'rendered_overlay' };
		expect(transitionProgress(s, 105000)).toBe(0.5);
		expect(transitionProgress({ ...s, runtime_renderer_status: 'legacy_overlap' }, 105000)).toBeNull();
		expect(transitionProgress({ ...s, runtime_renderer_status: 'boundary_fallback' }, 105000)).toBeNull();
		expect(transitionProgress({ ...s, runtime_renderer_reason: 'manual_seek_suppressed' }, 105000)).toBeNull();
		expect(transitionProgress({ ...s, enabled: false }, 105000)).toBeNull();
		expect(transitionProgress(s, 110000)).toBeNull();
		expect(transitionProgress({ ...s, runtime_renderer_status: 'rendered_handoff' }, 105000)).toBeNull();
		const sought = storySnapshot({ ...s, last_transition_event_id: 7 }, { id: 1, tidal_id: null }, 105000, 7);
		expect(transitionProgress(sought, sought?.playback_position_ms)).toBeNull();
	});
	test('synthetic and weak grids never draw phase alignment, but manual cues remain visible', () => {
		const p = { ...deck(2), grid_is_synthetic: true, manual_drop_markers_ms: [3000] };
		expect(sceneMarkers(program, 'B', p).map((marker) => marker.kind)).toEqual(['drop']);
		expect(sceneMarkers(program, 'B', { ...p, grid_is_synthetic: false, beat_confidence: 0.2 }).map((marker) => marker.kind)).toEqual(['drop']);
	});
	test('safe crossfades claim only the automation that is actually present', () => {
		const safe = { ...program, template: 'SafeCrossfade' };
		expect(hasBassAutomation(safe)).toBe(false);
		expect(executionLabel(safe)).toBe('Gain crossfade');
		const duck = { ...safe, automation: [...safe.automation,
			{ param: { LowGain: 'B' as const }, start_sample: 0, end_sample: 5000, from: 0.6, to: 1, curve: 'Linear' as const }] };
		expect(hasBassAutomation(duck, 'A')).toBe(false);
		expect(hasBassAutomation(duck, 'B')).toBe(true);
		expect(executionLabel(duck)).toBe('Gain crossfade + bass duck');
		expect(executionLabel({ ...duck, template: 'BassSwap16' })).toBe('Gain + bass handoff');
		expect(executionLabel({ ...safe, template: 'SlamCut' })).toBe('Clean cut');
	});
});
