import { afterEach, describe, expect, test, vi } from 'vitest';

import { setWebviewZoom } from './webview_zoom';

const setZoom = vi.fn<() => Promise<void>>(() => Promise.resolve());

vi.mock('@tauri-apps/api/webview', () => ({
	getCurrentWebview: () => ({ setZoom })
}));

describe('setWebviewZoom', () => {
	afterEach(() => {
		setZoom.mockClear();
		vi.unstubAllGlobals();
	});

	test('uses the official Tauri webview zoom API inside Tauri', async () => {
		vi.stubGlobal('window', { __TAURI_INTERNALS__: { invoke: vi.fn() } });

		await setWebviewZoom(1.25);

		expect(setZoom).toHaveBeenCalledWith(1.25);
	});

	test('does nothing outside Tauri', async () => {
		vi.stubGlobal('window', {});

		await setWebviewZoom(1.25);

		expect(setZoom).not.toHaveBeenCalled();
	});
});
