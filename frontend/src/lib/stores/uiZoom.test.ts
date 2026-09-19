import { afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

const setWebviewZoom = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock('$lib/tauri/webview_zoom', () => ({ setWebviewZoom }));

afterEach(() => {
	vi.unstubAllGlobals();
	vi.clearAllMocks();
	vi.resetModules();
});

describe('UI zoom with unavailable storage', () => {
	it('boots at the default and still applies zoom when reads and writes are blocked', async () => {
		vi.stubGlobal('localStorage', {
			getItem() { throw new DOMException('Blocked', 'SecurityError'); },
			setItem() { throw new DOMException('Blocked', 'SecurityError'); },
		});
		const zoom = await import('./uiZoom');
		expect(get(zoom.uiZoom)).toBe(zoom.DEFAULT);
		expect(() => zoom.zoomIn()).not.toThrow();
		expect(get(zoom.uiZoom)).toBe(1.1);
		expect(setWebviewZoom).toHaveBeenCalledWith(1.1);
	});

	it('still applies a restored preference change when storage becomes full', async () => {
		vi.stubGlobal('localStorage', {
			getItem: () => '1.25',
			setItem() { throw new DOMException('Full', 'QuotaExceededError'); },
		});
		const zoom = await import('./uiZoom');
		expect(get(zoom.uiZoom)).toBe(1.25);
		expect(() => zoom.resetZoom()).not.toThrow();
		expect(get(zoom.uiZoom)).toBe(zoom.DEFAULT);
		expect(setWebviewZoom).toHaveBeenCalledWith(zoom.DEFAULT);
	});

	it('survives a SecurityError from accessing localStorage itself', async () => {
		vi.stubGlobal('localStorage', undefined);
		Object.defineProperty(globalThis, 'localStorage', {
			configurable: true,
			get() { throw new DOMException('Blocked origin', 'SecurityError'); },
		});
		const zoom = await import('./uiZoom');
		expect(get(zoom.uiZoom)).toBe(zoom.DEFAULT);
		expect(() => zoom.setZoom(1.5)).not.toThrow();
		expect(setWebviewZoom).toHaveBeenCalledWith(1.5);
	});
});
