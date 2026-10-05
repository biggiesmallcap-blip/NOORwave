import { describe, expect, test, vi } from 'vitest';
import type { DjStatusResponse } from '$lib/api/client';
import { createCockpitRefresh } from './cockpit_refresh';

function client() {
	return {
		getDjEnabled: vi.fn(async () => ({ enabled: true })),
		getDjPolicy: vi.fn(async () => ({ mix_intent: 'balanced' as const, transition_speed_bias: 'neutral' as const, preferred_strategy: 'adaptive' as const })),
		getDjStatus: vi.fn(async () => ({ enabled: true, planning_status: 'waiting_for_window' } as DjStatusResponse)),
	};
}

describe('cockpit polling through partial failures', () => {
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
		expect((await refresh.refresh())?.enabled?.enabled).toBe(true);
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
});
