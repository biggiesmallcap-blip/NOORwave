import { get } from 'svelte/store';
import { afterEach, describe, expect, test, vi } from 'vitest';

function stubStorage(values: Record<string, string>) {
	const store = new Map(Object.entries(values));
	vi.stubGlobal('localStorage', {
		getItem: (key: string) => store.get(key) ?? null,
		setItem: (key: string, value: string) => void store.set(key, value),
		removeItem: (key: string) => void store.delete(key),
	});
}

describe('video fullscreen settings', () => {
	afterEach(() => {
		vi.unstubAllGlobals();
		vi.resetModules();
	});

	test('defaults to grow at 240 ms with a 90 ms dim', async () => {
		stubStorage({});
		const s = await import('./video_fullscreen_style');
		expect(get(s.videoFullscreenStyle)).toBe('grow');
		expect(get(s.videoFullscreenGrowMs)).toBe(240);
		expect(get(s.videoFullscreenDimMs)).toBe(90);
	});

	test('restores saved values, clamped into range', async () => {
		stubStorage({
			'noor-video-fullscreen': 'dim',
			'noor-video-fullscreen-grow-ms': '9000',
			'noor-video-fullscreen-dim-ms': '5',
		});
		const s = await import('./video_fullscreen_style');
		expect(get(s.videoFullscreenStyle)).toBe('dim');
		expect(get(s.videoFullscreenGrowMs)).toBe(s.VIDEO_FULLSCREEN_GROW_MAX);
		expect(get(s.videoFullscreenDimMs)).toBe(s.VIDEO_FULLSCREEN_DIM_MIN);
	});

	test('ignores junk', async () => {
		stubStorage({ 'noor-video-fullscreen': 'sideways', 'noor-video-fullscreen-grow-ms': 'fast' });
		const s = await import('./video_fullscreen_style');
		expect(get(s.videoFullscreenStyle)).toBe('grow');
		expect(get(s.videoFullscreenGrowMs)).toBe(240);
	});
});
