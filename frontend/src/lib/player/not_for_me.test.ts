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

	it('builds a menu item that calls through', () => {
		setNotForMe.mockResolvedValue({});
		const item = notForMeMenuItem('track', 3, 'Not for me');
		expect(item.label).toBe('Not for me');
		item.onSelect?.();
		expect(setNotForMe).toHaveBeenCalledWith('track', 3, true);
	});
});
