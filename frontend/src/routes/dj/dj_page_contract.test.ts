import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const page = readFileSync(join(root, 'routes/dj/+page.svelte'), 'utf8');
const cockpit = readFileSync(join(root, 'lib/components/dj-cockpit/DjCockpit.svelte'), 'utf8');
const transitionLane = readFileSync(
	join(root, 'lib/components/dj-cockpit/TransitionLane.svelte'),
	'utf8',
);
const transitionWaveform = readFileSync(
	join(root, 'lib/components/dj-cockpit/TransitionWaveform.svelte'),
	'utf8',
);
const queuePair = readFileSync(
	join(root, 'lib/components/dj-cockpit/QueuePairPanel.svelte'),
	'utf8',
);
const corrections = readFileSync(
	join(root, 'lib/components/dj-cockpit/ProfileCorrectionPanel.svelte'),
	'utf8',
);
const guardrails = readFileSync(
	join(root, 'lib/components/dj-cockpit/SafetyGuardrailPanel.svelte'),
	'utf8',
);
const layout = readFileSync(join(root, 'routes/+layout.svelte'), 'utf8');
const navigation = readFileSync(join(root, 'lib/routes/navigation-data.json'), 'utf8');
const registry = readFileSync(join(root, 'lib/routes/registry-data.json'), 'utf8');
const playerBar = readFileSync(join(root, 'lib/shell/PlayerBar.svelte'), 'utf8');
const remotePage = readFileSync(join(root, 'routes/remote/+page.svelte'), 'utf8');

describe('dj cockpit page contract', () => {
	test('dj_page_is_linked_from_main_navigation', () => {
		expect(navigation).toContain('"dj"');
		expect(registry).toContain('/dj');
		expect(page).toContain('DjCockpit');
	});

	test('player_bar_does_not_expose_open_dj_cockpit_action', () => {
		expect(playerBar).not.toContain('Open DJ Cockpit');
		expect(playerBar).not.toContain('np-dj-btn');
		expect(playerBar).not.toContain('onOpenDjCockpit');
		expect(layout).not.toContain('onOpenDjCockpit');
	});

	test('remote_ui_does_not_expose_dj_controls', () => {
		expect(remotePage).not.toContain('DJ Cockpit');
		expect(remotePage).not.toContain('/dj');
	});

	test('settings_page_does_not_duplicate_dj_controls', () => {
		const settings = readFileSync(join(root, 'routes/settings/+page.svelte'), 'utf8');
		expect(settings).not.toContain('DJ Cockpit');
		expect(settings).not.toContain('safe_crossfade_only');
	});

	test('dj_toggle_round_trips_server_config', () => {
		expect(cockpit).toContain('api.getDjEnabled()');
		expect(cockpit).toContain('api.setDjEnabled(next)');
	});

	test('dj_disabled_state_explains_legacy_path', () => {
		expect(cockpit).toContain('role="switch"');
		expect(cockpit).toContain('aria-checked={enabled}');
		expect(cockpit).toContain('DJ transitions');
		expect(cockpit).toContain("{enabled ? 'On' : 'Off'}");
		expect(cockpit).toContain('Enable DJ transitions');
		expect(cockpit).toContain('Disable DJ transitions');
		expect(cockpit).not.toContain("'DJ transitions on'");
		expect(cockpit).not.toContain('Use legacy playback');
		expect(cockpit).toContain('Playback is using the legacy path');
		expect(cockpit).toContain('DJ lookahead and transition planning are stopped');
		expect(cockpit).toContain('next eligible current-plus-next pair');
	});

	test('dj_page_renders_current_and_next_pair', () => {
		expect(cockpit).toContain('QueuePairPanel');
		expect(cockpit).toContain('current={status?.current}');
		expect(cockpit).toContain('next={status?.next}');
	});

	test('dj_page_exposes_global_policy_controls', () => {
		expect(cockpit).toContain('MixIntentControl');
		expect(cockpit).toContain('api.setDjMixIntent');
		expect(cockpit).toContain('api.setDjPolicy');
	});

	test('dj_page_shows_fallback_reason', () => {
		expect(transitionLane).toContain('Fallback reason');
	});

	test('dj_page_shows_transition_waveform_visual', () => {
		expect(transitionLane).toContain('TransitionWaveform');
		expect(transitionWaveform).toContain('Transition visual');
		expect(transitionWaveform).toContain('Mix window');
		expect(transitionWaveform).toContain('Read-only transition waveform');
		expect(transitionWaveform).toContain('waveform_status');
		expect(transitionWaveform).toContain('waveform_peaks');
		expect(transitionWaveform).toContain('Planned fire');
		expect(transitionWaveform).toContain('Actual fire');
		expect(transitionWaveform).toContain('Fire delta');
		expect(transitionWaveform).toContain('missed');
		expect(transitionWaveform).not.toContain('<canvas');
		expect(transitionWaveform).not.toContain('style:');
	});

	test('dj_page_separates_profile_readiness_from_transition_armed', () => {
		expect(transitionLane).toContain('Analyzing profiles');
		expect(transitionLane).toContain('Waiting for mix window');
		expect(transitionLane).toContain('Ready to plan');
		expect(transitionLane).toContain('Transition armed');
		expect(transitionLane).toContain('planning_status');
		expect(transitionLane).not.toContain('Transition ready');
	});

	test('dj_page_surfaces_profile_decode_failures', () => {
		expect(transitionLane).toContain('Profile analysis failed');
		expect(transitionLane).toContain('current_profile_decode_failed');
		expect(queuePair).toContain('Analysis failed');
		expect(queuePair).toContain('Retrying analysis');
		expect(queuePair).toContain('Retrying analysis in');
		expect(queuePair).toContain('TIDAL asset unavailable');
		expect(queuePair).toContain('Analyzing');
		expect(queuePair).toContain('profile_error');
		expect(queuePair).toContain('profile_retry_after_ms');
		expect(queuePair).toContain('profile_retry_reason');
		expect(queuePair).toContain('Passive DSP retrying');
		expect(queuePair).toContain('passive_analysis_status');
		expect(queuePair).toContain('passive_analysis_reason');
	});

	test('dj_page_debug_details_use_planner_facts', () => {
		expect(transitionLane).toContain('Debug planner facts');
		expect(transitionLane).toContain('Current event');
		expect(transitionLane).toContain('Recent timing');
		expect(transitionLane).toContain('Recent timing (last 5)');
		expect(transitionLane).toContain('slice(0, 5)');
		expect(transitionLane).toContain('Timing direction');
		expect(transitionLane).toContain('formatTrackLabel');
		expect(transitionLane).toContain('formatTimingDirection');
		expect(transitionLane).toContain('formatTimingPair');
		expect(transitionLane).toContain('formatTimingState');
		expect(transitionLane).toContain('formatActualTiming');
		expect(transitionLane).toContain('formatEventDelta');
		expect(transitionLane).toContain('Planning reason');
		expect(transitionLane).toContain('Readiness block');
		expect(transitionLane).toContain('Decision');
		expect(transitionLane).toContain('Rejected alternatives');
		expect(transitionLane).toContain('formatRejectedAlternative');
		expect(transitionLane).toContain('formatRejectedReason');
		expect(transitionLane).toContain('Bold mode selected FilterSweep');
		expect(transitionLane).not.toContain('alternative.score');
		expect(transitionLane).toContain('Avg delta');
		expect(transitionLane).toContain('Avg abs');
		expect(transitionLane).toContain('Missed');
		expect(transitionLane).toContain('Plan');
		expect(transitionLane).toContain('Planned fire');
		expect(transitionLane).toContain('Actual fire');
		expect(transitionLane).toContain('Planned template');
		expect(transitionLane).toContain('Renderer mode');
		expect(transitionLane).toContain('Fire delta');
		expect(transitionLane).toContain('Sync source');
		expect(transitionLane).toContain('Timing status');
		expect(transitionLane).toContain('Runtime rendered');
		expect(transitionLane).toContain('Runtime status');
		expect(transitionLane).toContain('Runtime reason');
		expect(transitionLane).toContain('Cause');
		expect(transitionLane).toContain('runtime_rendered_dj_mixer');
		expect(transitionLane).toContain('runtime_renderer_status');
		expect(transitionLane).toContain('runtime_renderer_reason');
		expect(transitionLane).toContain('formatRuntimeRendered');
		expect(transitionLane).toContain('formatRuntimeReason');
		expect(transitionLane).toContain('formatActualFire');
		expect(transitionLane).toContain('formatFireDelta');
		expect(transitionLane).toContain('next_decode_late_at_fire');
		expect(transitionLane).toContain('next_deck_missing_at_fire');
		expect(transitionLane).toContain('transition_plan_missing_at_fire');
		expect(transitionLane).toContain('sync_window_not_signaled');
		expect(transitionLane).toContain('Decode late');
		expect(transitionLane).toContain('Next deck missing');
		expect(transitionLane).toContain('Plan missing');
		expect(transitionLane).toContain('Sync missed');
		expect(transitionLane).toContain("'pending'");
		expect(transitionLane).toContain('overlay_details');
		expect(transitionLane).toContain('Overlay status');
		expect(transitionLane).toContain('Overlay tempo');
		expect(transitionLane).toContain('Deck B start frame');
		expect(transitionLane).toContain('Drop source');
		expect(transitionLane).toContain('formatTempoRatio');
		expect(transitionLane).toContain('Planning status');
	});

	test('dj_page_timing_history_is_first_class_and_reports_played_audio', () => {
		// The history list lives outside the debug drawer and every row says
		// what the runtime actually played, not just what the planner picked.
		expect(transitionLane).toContain('playedOutcome');
		expect(transitionLane).toContain('playedIsFallback');
		expect(transitionLane).toContain('Fallback crossfade (planned ');
		expect(transitionLane).toContain('Clean cut at boundary (planned ');
		expect(transitionLane).toContain('Rendered ${planned}');
		expect(transitionLane).toContain('qualityRail');
		expect(transitionLane).toContain('rail-tight');
		expect(transitionLane).toContain('rail-missed');
		expect(transitionLane).toContain('handoff_seam_too_late');
		expect(transitionLane).toContain('Joined too late');
	});

	test('dj_page_hero_shows_sync_mode_and_fallback_state', () => {
		expect(transitionLane).toContain('formatSyncBadge');
		expect(transitionLane).toContain('Beat-locked (downbeat)');
		expect(transitionLane).toContain('Beat-locked (grid)');
		expect(transitionLane).toContain('Track-end timing');
		expect(transitionLane).toContain('Fallback audio');
		expect(transitionLane).toContain('hero-chip');
	});

	test('dj_page_does_not_show_non_renderable_templates_as_main_label', () => {
		expect(transitionLane).toContain('DJ overlap armed');
		expect(transitionLane).toContain('renderer_template');
		expect(transitionLane).toContain('planned_template');
		expect(transitionLane).not.toContain("status?.selected_program ?? 'Transition armed'");
	});

	test('dj_page_updates_bpm_multiplier_correction', () => {
		expect(corrections).toContain('Transition rules');
		expect(corrections).not.toContain('Profile overrides');
		expect(corrections).toContain('bpm_multiplier');
		expect(corrections).toContain('BPM multiplier');
		expect(corrections).toContain('Rules change planning');
		expect(corrections).toContain('Forces SafeCrossfade');
		expect(corrections).toContain('Requests faster handoff');
	});

	test('dj_page_updates_downbeat_and_phrase_corrections', () => {
		expect(corrections).toContain('downbeat_offset_beats');
		expect(corrections).toContain('phrase_offset_bars');
	});

	test('dj_page_clears_profile_correction', () => {
		expect(corrections).toContain('Clear override');
		expect(cockpit).toContain('clearCorrection');
	});

	test('dj_page_rebuild_profile_action_is_async', () => {
		expect(corrections).toContain('Rebuild profile');
		expect(cockpit).toContain('rebuildDjProfile');
		expect(cockpit).toContain('Profile rebuild accepted');
		expect(cockpit).toContain('Profile rebuild already running');
		expect(cockpit).toContain('Profile source unavailable');
	});

	test('dj_page_shows_correction_applies_next_when_transition_armed', () => {
		expect(corrections).toContain('Changes apply to the next transition');
		expect(cockpit).toContain('transitionArmed');
	});

	test('dj_page_accepts_safe_crossfade_suggestion_only_on_user_action', () => {
		expect(guardrails).toContain('Accept safe-only suggestion');
		expect(guardrails).toContain('Planning state');
		expect(cockpit).toContain('acceptSafeOnlySuggestion');
	});

	test('dj_page_keeps_waveform_read_only', () => {
		expect(page + cockpit + transitionLane + transitionWaveform).not.toContain('<canvas');
		expect(transitionWaveform).not.toContain('draggable');
		expect(transitionWaveform).not.toContain('onpointermove');
	});

	test('dj_page_controls_have_accessible_names', () => {
		expect(cockpit).toContain('aria-label');
		expect(transitionLane).toContain('aria-label="Transition feedback"');
		expect(corrections).toContain('aria-label="Correction target"');
	});
});
