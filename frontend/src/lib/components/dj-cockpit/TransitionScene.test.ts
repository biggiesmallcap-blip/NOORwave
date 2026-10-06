import { render } from 'svelte/server';
import { describe, expect, test } from 'vitest';
import type { DjStatusResponse, DjTransitionProgram } from '$lib/api/client';
import TransitionScene from './TransitionScene.svelte';

const program: DjTransitionProgram = {
	template: 'SafeCrossfade', tier: 'SafeCrossfade', sample_rate: 1000, channels: 2,
	deck_a_start_frame: 0, deck_b_start_frame: 0, sync_start: 0, intro_start: 0,
	swap_start: 3000, fade_start: 3000, resolve_at: 6000, loops: [],
	automation: [
		{ param: { DeckGain: 'A' }, start_sample: 0, end_sample: 6000, from: 1, to: 0, curve: 'EqualPowerOut' },
		{ param: { DeckGain: 'B' }, start_sample: 0, end_sample: 6000, from: 0, to: 1, curve: 'EqualPowerIn' }
	]
};
const status = { enabled: true, transition_plan: program, planned_start_ms: 100000 } as DjStatusResponse;

describe('spatial transition presentation', () => {
	test('an armed crossfade is a stationary preview without a fictional bass swap', () => {
		const { body } = render(TransitionScene, { props: { status, progress: 0.5 } });
		expect(body).toContain('Scheduled preview');
		expect(body).toContain('Gain crossfade');
		expect(body).toContain('Start · 0s');
		expect(body).toContain('Finish · 6.0s');
		expect(body).not.toContain('class="cursor');
		expect(body).not.toContain('bass-envelope deck-');
		expect(body).not.toContain('Bass handoff');
	});
	test('real bass duck automation appears without certifying a phase-aligned bass swap', () => {
		const duck = { ...program, automation: [...program.automation,
			{ param: { LowGain: 'B' as const }, start_sample: 0, end_sample: 3000, from: 0.6, to: 1, curve: 'Linear' as const }] };
		const { body } = render(TransitionScene, { props: { status: { ...status, transition_plan: duck } } });
		expect(body).toContain('Gain crossfade + bass duck');
		expect(body).toContain('bass-envelope deck-B');
		expect(body).not.toContain('bass-envelope deck-A');
		expect(body).not.toContain('Bass handoff');
	});
	test('fallbacks and disabled DJ hide the execution graphic', () => {
		for (const s of [{ ...status, enabled: false }, { ...status, runtime_renderer_status: 'boundary_fallback' }]) {
			const { body } = render(TransitionScene, { props: { status: s, progress: 0.5 } });
			expect(body).not.toContain('<svg');
			expect(body).not.toContain('Live audio');
		}
	});
	test('a confirmed audible handoff alone shows live playback cursors', () => {
		const live = { ...status, runtime_rendered_dj_mixer: true, timing_status: 'fired',
			runtime_renderer_status: 'rendered_handoff', actual_start_ms: 100368,
			active_transition: { event_id: 7, outgoing: {} as never, incoming: {} as never,
				program, start_ms: 100000, actual_start_ms: 100368, elapsed_ms: 3000 } };
		const { body } = render(TransitionScene, { props: { status: live, progress: 0.5, playing: true } });
		expect(body).toContain('Live audio');
		expect(body).toContain('class="cursor deck-A');
		expect(body).toContain('class="cursor deck-B');
	});
});
