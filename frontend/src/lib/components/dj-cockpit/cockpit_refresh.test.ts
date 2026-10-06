import { describe, expect, test, vi } from 'vitest';
import type { DjStatusResponse } from '$lib/api/client';
import { cockpitPollInterval, createCockpitRefresh, newlyConfirmedCut } from './cockpit_refresh';

function client() {
	return {
		getDjEnabled: vi.fn(async () => ({ enabled: true })),
		getDjPolicy: vi.fn(async () => ({ mix_intent: 'balanced' as const, transition_speed_bias: 'neutral' as const, preferred_strategy: 'adaptive' as const })),
		getDjStatus: vi.fn(async () => ({ enabled: true, planning_status: 'waiting_for_window' } as DjStatusResponse)),
	};
}

describe('cockpit polling through partial failures', () => {
	test('fast polls are bounded to foreground playback near an actual opportunity', () => {
		const armed = { enabled: true, planned_start_ms: 100000, drop_preview: { status: 'skipped' } } as DjStatusResponse;
		expect(cockpitPollInterval(armed, 95000, true, true)).toBe(2000);
		expect(cockpitPollInterval(armed, 97000, true, true)).toBe(500);
		expect(cockpitPollInterval(armed, 97000, false, true)).toBe(2000);
		expect(cockpitPollInterval(armed, 97000, true, false)).toBe(2000);
		expect(cockpitPollInterval(armed, 102000, true, true)).toBe(2000);
		expect(cockpitPollInterval({ ...armed, active_transition: {} as never }, 2000, true, true)).toBe(500);
		expect(cockpitPollInterval({ ...armed, drop_preview: { status: 'armed', planned_fire_ms: 60000 } }, 57000, true, true)).toBe(500);
	});
	test('a new confirmed cut is acknowledged without replaying old history or planned cuts', () => {
		const previous = { recent_timing_events: [] } as unknown as DjStatusResponse;
		const cut = { event_id: 7, planned_template: 'SlamCut', renderer_template: 'SlamCut',
			actual_start_ms: 100010, timing_status: 'fired', runtime_rendered_dj_mixer: true,
			runtime_renderer_status: 'rendered_handoff', from_title: 'A', to_title: 'B' } as const;
		const next = { recent_timing_events: [cut] } as unknown as DjStatusResponse;
		expect(newlyConfirmedCut(previous, next)).toBe('Cut fired · A → B');
		expect(newlyConfirmedCut(null, next)).toBeNull();
		expect(newlyConfirmedCut(next, next)).toBeNull();
		expect(newlyConfirmedCut(previous, { ...next, recent_timing_events: [{ ...cut, renderer_template: 'SafeCrossfade' }] } as DjStatusResponse)).toBeNull();
		expect(newlyConfirmedCut(previous, { ...next, recent_timing_events: [{ ...cut, actual_start_ms: undefined }] } as DjStatusResponse)).toBeNull();
	});
	test('a failed status request retains usable enabled and policy responses', async () => {
		const api = client();
		api.getDjStatus.mockRejectedValue(new Error('HTTP 500'));
		const snapshot = await createCockpitRefresh(api).refresh();
		expect(snapshot?.enabled?.enabled).toBe(true);
		expect(snapshot?.policy?.preferred_strategy).toBe('adaptive');
		expect(snapshot?.status).toBeUndefined();
		expect(snapshot?.error).toBe('Transition status: HTTP 500');
	});
	test('failures back off, a manual retry bypasses the delay and success resets it', async () => {
		const api = client();
		let time = 0;
		api.getDjStatus.mockRejectedValue(new Error('Offline'));
		const refresh = createCockpitRefresh(api, () => time);
		await refresh.refresh();
		time = 1999;
		expect(await refresh.refresh()).toBeNull();
		time = 2000;
		await refresh.refresh();
		time = 5999;
		expect(await refresh.refresh()).toBeNull();
		expect(api.getDjStatus).toHaveBeenCalledTimes(2);
		api.getDjStatus.mockResolvedValue({ enabled: true } as DjStatusResponse);
		expect((await refresh.refresh(true))?.error).toBe('');
		expect((await refresh.refresh())?.status?.enabled).toBe(true);
	});
	test('a slow poll cannot create overlapping request groups or update after unmount', async () => {
		const api = client();
		let finish!: (status: DjStatusResponse) => void;
		api.getDjStatus.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
		const refresh = createCockpitRefresh(api);
		const pending = refresh.refresh();
		expect(await refresh.refresh(true)).toBeNull();
		expect(api.getDjStatus).toHaveBeenCalledTimes(1);
		refresh.dispose();
		finish({ enabled: true } as DjStatusResponse);
		expect(await pending).toBeNull();
		expect(await refresh.refresh(true)).toBeNull();
	});
	test('fast status polls do not repeat policy and enabled requests within two seconds', async () => {
		const api = client();
		let time = 0;
		const refresh = createCockpitRefresh(api, () => time);
		await refresh.refresh();
		for (time = 500; time < 2000; time += 500) await refresh.refresh();
		expect(api.getDjStatus).toHaveBeenCalledTimes(4);
		expect(api.getDjEnabled).toHaveBeenCalledTimes(1);
		expect(api.getDjPolicy).toHaveBeenCalledTimes(1);
		await refresh.refresh();
		expect(api.getDjEnabled).toHaveBeenCalledTimes(2);
		expect(api.getDjPolicy).toHaveBeenCalledTimes(2);
	});
});
