import { beforeEach, describe, expect, test, vi } from 'vitest';
import { VideoDiscoverySettings } from './videoDiscovery.svelte';

const mocks = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn(), status: vi.fn() }));
vi.mock('$lib/api/client', () => ({
	api: { getVideoDiscoverySettings: mocks.get, setVideoDiscoverySettings: mocks.set, getVideoDiscoveryStatus: mocks.status },
}));

beforeEach(() => vi.resetAllMocks());
describe('video discovery setting', () => {
	test('edits wait until the server value is known', async () => {
		const settings = new VideoDiscoverySettings();
		await settings.save('off');
		expect(mocks.set).not.toHaveBeenCalled();
		mocks.get.mockResolvedValue({ setting: 'limited' });
		mocks.status.mockRejectedValue(new Error('offline'));
		await settings.load();
		expect(settings.known).toBe(true);
		expect(settings.setting).toBe('limited');
		expect(settings.status).toBeNull();
		expect(settings.error).toBe('');
	});

	test('a failed save rolls back to the previous setting', async () => {
		const settings = new VideoDiscoverySettings();
		mocks.get.mockResolvedValue({ setting: 'full' });
		mocks.status.mockResolvedValue({
			setting: 'full', mode: 'idle', calls_last_hour: 1, calls_today: 1, artists_with_videos: 2, catalog_videos: 3,
		});
		await settings.load();
		mocks.set.mockRejectedValueOnce(new Error('offline')).mockResolvedValue({ setting: 'off' });
		await settings.save('off');
		expect(settings.setting).toBe('full');
		expect(settings.error).toContain('Could not save');
		await settings.save('off');
		expect(settings.setting).toBe('off');
		expect(mocks.set).toHaveBeenLastCalledWith('off');
	});
});
