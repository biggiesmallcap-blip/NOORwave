import { afterEach, describe, expect, test, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
	isTauri: vi.fn(),
	openUrl: vi.fn(),
}));

vi.mock('@tauri-apps/api/core', () => ({
	isTauri: mocks.isTauri,
}));

vi.mock('@tauri-apps/plugin-opener', () => ({
	openUrl: mocks.openUrl,
}));

import { openExternal } from './external';

describe('openExternal', () => {
	afterEach(() => {
		mocks.isTauri.mockReset();
		mocks.openUrl.mockReset();
		vi.unstubAllGlobals();
	});

	test('opens URLs with the Tauri opener inside Tauri', async () => {
		mocks.isTauri.mockReturnValue(true);
		mocks.openUrl.mockResolvedValue(undefined);

		const result = await openExternal('https://login.tidal.com/authorize');

		expect(mocks.openUrl).toHaveBeenCalledWith('https://login.tidal.com/authorize');
		expect(result).toEqual({ ok: true, method: 'tauri' });
	});

	test('reports Tauri opener failures instead of swallowing them', async () => {
		mocks.isTauri.mockReturnValue(true);
		mocks.openUrl.mockRejectedValue(new Error('shell denied'));

		const result = await openExternal('https://login.tidal.com/authorize');

		expect(result).toEqual({
			ok: false,
			method: 'tauri',
			error: 'shell denied',
		});
	});

	test('reports blocked browser popups outside Tauri', async () => {
		const open = vi.fn(() => null);
		mocks.isTauri.mockReturnValue(false);
		vi.stubGlobal('window', { open });

		const result = await openExternal('https://login.tidal.com/authorize');

		expect(open).toHaveBeenCalledWith('https://login.tidal.com/authorize', '_blank');
		expect(result).toEqual({
			ok: false,
			method: 'browser',
			error: 'Browser blocked the sign-in window.',
		});
	});

	test('clears opener on successful browser opens', async () => {
		const opened = { opener: {} };
		const open = vi.fn(() => opened);
		mocks.isTauri.mockReturnValue(false);
		vi.stubGlobal('window', { open });

		const result = await openExternal('https://login.tidal.com/authorize');

		expect(open).toHaveBeenCalledWith('https://login.tidal.com/authorize', '_blank');
		expect(opened.opener).toBeNull();
		expect(result).toEqual({ ok: true, method: 'browser' });
	});
});
