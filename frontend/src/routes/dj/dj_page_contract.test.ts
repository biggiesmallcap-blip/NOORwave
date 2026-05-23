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

	test('player_or_queue_exposes_open_dj_cockpit_action', () => {
		expect(playerBar).toContain('Open DJ Cockpit');
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
		expect(cockpit).toContain('Playback is using the legacy path');
		expect(cockpit).toContain('DJ lookahead and transition planning are stopped');
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

	test('dj_page_separates_profile_readiness_from_transition_armed', () => {
		expect(transitionLane).toContain('Analyzing profiles');
		expect(transitionLane).toContain('Ready to plan');
		expect(transitionLane).toContain('Transition armed');
		expect(transitionLane).not.toContain('Transition ready');
	});

	test('dj_page_surfaces_profile_decode_failures', () => {
		expect(transitionLane).toContain('Profile analysis failed');
		expect(transitionLane).toContain('current_profile_decode_failed');
		expect(queuePair).toContain('Analysis failed');
		expect(queuePair).toContain('profile_error');
	});

	test('dj_page_debug_details_use_planner_facts', () => {
		expect(transitionLane).toContain('Debug planner facts');
		expect(transitionLane).toContain('Current event');
		expect(transitionLane).toContain('Recent timing');
		expect(transitionLane).toContain('Quality');
		expect(transitionLane).toContain('Timing direction');
		expect(transitionLane).toContain('formatTrackLabel');
		expect(transitionLane).toContain('formatTimingDirection');
		expect(transitionLane).toContain('formatTimingPair');
		expect(transitionLane).toContain('formatActualTiming');
		expect(transitionLane).toContain('formatEventDelta');
		expect(transitionLane).toContain('Planning reason');
		expect(transitionLane).toContain('Readiness block');
		expect(transitionLane).toContain('Avg delta');
		expect(transitionLane).toContain('Avg abs');
		expect(transitionLane).toContain('Missed');
		expect(transitionLane).toContain('Pair');
		expect(transitionLane).toContain('Plan');
		expect(transitionLane).toContain('Planned');
		expect(transitionLane).toContain('Actual');
		expect(transitionLane).toContain('Planned template');
		expect(transitionLane).toContain('Renderer mode');
		expect(transitionLane).toContain('Current planned');
		expect(transitionLane).toContain('Current fire');
		expect(transitionLane).toContain('Current delta');
		expect(transitionLane).toContain('Sync source');
		expect(transitionLane).toContain('Timing status');
		expect(transitionLane).toContain("'pending'");
	});

	test('dj_page_does_not_show_non_renderable_templates_as_main_label', () => {
		expect(transitionLane).toContain('DJ overlap armed');
		expect(transitionLane).toContain('renderer_template');
		expect(transitionLane).toContain('planned_template');
		expect(transitionLane).not.toContain("status?.selected_program ?? 'Transition armed'");
	});

	test('dj_page_updates_bpm_multiplier_correction', () => {
		expect(corrections).toContain('bpm_multiplier');
		expect(corrections).toContain('BPM multiplier');
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
		expect(cockpit).toContain('acceptSafeOnlySuggestion');
	});

	test('dj_page_does_not_show_waveform_canvas', () => {
		expect(page + cockpit + transitionLane).not.toContain('<canvas');
	});

	test('dj_page_controls_have_accessible_names', () => {
		expect(cockpit).toContain('aria-label');
		expect(transitionLane).toContain('aria-label="Transition feedback"');
		expect(corrections).toContain('aria-label="Correction target"');
	});
});
