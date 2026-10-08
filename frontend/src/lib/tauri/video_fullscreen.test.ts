import { afterEach, describe, expect, test, vi } from 'vitest';

import { hasNativeVideoFullscreen, setNativeVideoFullscreen } from './video_fullscreen';

const invoke = vi.fn<(cmd: string, args: unknown) => Promise<void>>(() => Promise.resolve());

vi.mock('@tauri-apps/api/core', () => ({ invoke }));

describe('setNativeVideoFullscreen', () => {
	afterEach(() => {
		invoke.mockClear();
		vi.unstubAllGlobals();
	});

	test('asks the desktop app to switch the window', async () => {
		vi.stubGlobal('window', { __TAURI_INTERNALS__: { invoke: vi.fn() } });

		expect(hasNativeVideoFullscreen()).toBe(true);
		expect(await setNativeVideoFullscreen(true)).toBe(true);
		expect(invoke).toHaveBeenCalledWith('set_video_fullscreen', { on: true });
	});

	test('reports failure instead of throwing', async () => {
		vi.stubGlobal('window', { __TAURI_INTERNALS__: { invoke: vi.fn() } });
		invoke.mockRejectedValueOnce(new Error('denied'));
		vi.spyOn(console, 'warn').mockImplementation(() => {});

		expect(await setNativeVideoFullscreen(false)).toBe(false);
	});

	test('does nothing outside the desktop app', async () => {
		vi.stubGlobal('window', {});

		expect(hasNativeVideoFullscreen()).toBe(false);
		expect(await setNativeVideoFullscreen(true)).toBe(false);
		expect(invoke).not.toHaveBeenCalled();
	});
});
