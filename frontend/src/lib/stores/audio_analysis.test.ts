import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { api } from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';
vi.mock('$lib/api/client', () => ({ api: { setPassiveDsp: vi.fn() } }));
vi.mock('$lib/cache/api_queries', () => ({ cachedApi: { getPassiveDsp: vi.fn() } }));
beforeEach(() => { vi.resetModules(); vi.clearAllMocks(); });
describe('passive audio analysis settings', () => {
	it('leaves an unavailable setting unknown instead of claiming a saved default', async () => {
		const store = await import('./audio_analysis');
		vi.mocked(cachedApi.getPassiveDsp).mockRejectedValue(new Error('Offline'));
		await store.loadPassiveDspState();
		expect(get(store.passiveDspKnown)).toBe(false);
		expect(get(store.audioAnalysisError)).toContain('unavailable');
	});
	it('restores the previous preference and surfaces a failed save', async () => {
		const store = await import('./audio_analysis');
		vi.mocked(cachedApi.getPassiveDsp).mockResolvedValue({ enabled: true });
		await store.loadPassiveDspState();
		vi.mocked(api.setPassiveDsp).mockRejectedValue(new Error('Offline'));
		await store.setPassiveDspEnabled(false);
		expect(get(store.audioAnalysis).passiveEnabled).toBe(true);
		expect(get(store.passiveDspPending)).toBe(false);
		expect(get(store.audioAnalysisError)).toContain('previous choice');
	});
	it('serializes changes while a save is in flight', async () => {
		const store = await import('./audio_analysis');
		vi.mocked(cachedApi.getPassiveDsp).mockResolvedValue({ enabled: true });
		await store.loadPassiveDspState();
		let finish!: (value: { enabled: boolean }) => void;
		vi.mocked(api.setPassiveDsp).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
		const saving = store.setPassiveDspEnabled(false);
		await store.setPassiveDspEnabled(true);
		expect(api.setPassiveDsp).toHaveBeenCalledTimes(1);
		finish({ enabled: false }); await saving;
		expect(get(store.audioAnalysis).passiveEnabled).toBe(false);
	});
});
