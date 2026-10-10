import { beforeEach, describe, expect, it, vi } from 'vitest';

const setNotForMe = vi.fn();
const showToast = vi.fn();

vi.mock('$lib/api/client', () => ({ api: { setNotForMe: (...args: unknown[]) => setNotForMe(...args) } }));
vi.mock('$lib/stores/toast', () => ({ showToast: (...args: unknown[]) => showToast(...args) }));

import { markNotForMe, notForMeMenuItem } from './not_for_me';

describe('not for me', () => {
	beforeEach(() => {
		setNotForMe.mockReset();
		showToast.mockReset();
	});

	it('marks the entity and confirms', async () => {
		setNotForMe.mockResolvedValue({ kind: 'artist', id: 7, not_for_me: true });
		await markNotForMe('artist', 7);
		expect(setNotForMe).toHaveBeenCalledWith('artist', 7, true);
		expect(showToast).toHaveBeenCalledWith('Got it: this artist will not be suggested again.');
	});

	it('reports a failure instead of claiming success', async () => {
		setNotForMe.mockRejectedValue(new Error('offline'));
		await markNotForMe('track', 3);
		expect(showToast).toHaveBeenCalledWith('Could not save that. Try again.', 'error');
	});

	it('is one entry: the song alone, or a submenu with the artist', () => {
		setNotForMe.mockResolvedValue({});
		const solo = notForMeMenuItem({ id: 3 });
		expect(solo.label).toBe('Not for me');
		solo.onSelect?.();
		expect(setNotForMe).toHaveBeenCalledWith('track', 3, true);

		const both = notForMeMenuItem({ id: 3, artist_id: 9, artist_name: 'John Denver' });
		expect(both.label).toBe('Not for me');
		expect(both.submenu?.map((item) => item.label)).toEqual(['This song', 'Anything by John Denver']);
		both.submenu?.[1].onSelect?.();
		expect(setNotForMe).toHaveBeenCalledWith('artist', 9, true);
	});
});
