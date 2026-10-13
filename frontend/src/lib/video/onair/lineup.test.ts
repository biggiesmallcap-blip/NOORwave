import { afterEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api } from '$lib/api/client';
import { byNumber, channelHue, channelLineup, ensureLineup, exploreMode, forYouTonight, setExploreMode, setLineupForTest } from './lineup';
import { fixtureChannels } from './fixtures';

vi.mock('$lib/api/client', () => ({
	api: {
		getVideoChannels: vi.fn(),
		getVideoExplore: vi.fn(),
		putVideoExplore: vi.fn(),
	},
}));

afterEach(() => {
	setLineupForTest(null);
	vi.clearAllMocks();
});

describe('the dial', () => {
	test('"For you tonight" is Your Channel, then scenes in the server\'s appetite order', () => {
		const picks = forYouTonight(fixtureChannels.channels);
		expect(picks.map((channel) => channel.id)).toEqual(['your', 'scene:7', 'scene:3']);
	});

	test('it only offers channels with something on, never locked ones', () => {
		const picks = forYouTonight(fixtureChannels.channels, 6);
		expect(picks.every((channel) => !channel.locked && channel.now)).toBe(true);
	});

	test('few scenes: fixed channels fill the strip to three', () => {
		const fixedOnly = fixtureChannels.channels.filter((channel) => channel.scene_id == null);
		expect(forYouTonight(fixedOnly).map((channel) => channel.id)).toEqual(['your', 'concerts']);
	});

	test('every channel in number order', () => {
		expect(byNumber(fixtureChannels.channels).map((channel) => channel.no)).toEqual([1, 6, 7, 12, 14, 21]);
	});

	test('coming-up scenes (no 0) follow the numbered channels in server order', () => {
		const dial = [
			{ id: 'scene:9', no: 0 },
			{ id: 'scene:3', no: 12 },
			{ id: 'your', no: 1 },
			{ id: 'scene:4', no: 0 },
		];
		expect(byNumber(dial).map((channel) => channel.id)).toEqual(['your', 'scene:3', 'scene:9', 'scene:4']);
	});

	test('scene hue when there is one, a stable hue per fixed channel otherwise', () => {
		const [bay, your] = fixtureChannels.channels;
		expect(channelHue(bay)).toBe(32);
		expect(channelHue(your)).toBe(350);
		expect(channelHue({ id: 'scene:1', hue: -30, no: 10 })).toBe(330);
	});
});

describe('lineup and explore', () => {
	test('a fresh lineup is reused; a stale one is fetched once', async () => {
		vi.mocked(api.getVideoChannels).mockResolvedValue(fixtureChannels);
		await ensureLineup();
		await ensureLineup();
		expect(api.getVideoChannels).toHaveBeenCalledTimes(1);
		expect(get(channelLineup)?.data.channels).toHaveLength(6);
		expect(get(exploreMode)).toBe('mixed');
	});

	test('explore changes save on the server and roll back when the save fails', async () => {
		vi.mocked(api.putVideoExplore).mockResolvedValueOnce({ explore: 'adventurous' });
		expect(await setExploreMode('adventurous')).toBe(true);
		expect(api.putVideoExplore).toHaveBeenCalledWith('adventurous');
		expect(get(exploreMode)).toBe('adventurous');

		vi.mocked(api.putVideoExplore).mockRejectedValueOnce(new Error('offline'));
		expect(await setExploreMode('familiar')).toBe(false);
		expect(get(exploreMode)).toBe('adventurous');
	});
});
