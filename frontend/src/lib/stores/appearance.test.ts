import { afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

function stubStorage(saved: Record<string, string> = {}) {
	const values = new Map(Object.entries(saved));
	vi.stubGlobal('localStorage', {
		getItem: (key: string) => values.get(key) ?? null,
		setItem: (key: string, value: string) => { values.set(key, value); },
	});
	return values;
}

async function appearance() {
	const [palette, surface, wallpaper] = await Promise.all([
		import('./palette'), import('./surfaceMode'), import('./wallpaper'),
	]);
	return { ...palette, ...surface, ...wallpaper };
}

afterEach(() => {
	vi.resetModules();
	vi.unstubAllGlobals();
});

describe('appearance preferences', () => {
	it('starts fresh with clay, light parchment, and a calm background without writing defaults', async () => {
		const values = stubStorage();
		const stores = await appearance();
		expect(get(stores.palette)).toBe('clay');
		expect(get(stores.surfaceMode)).toBe('light');
		expect(get(stores.wallpaper)).toBe('none');
		expect(values.size).toBe(0);
	});

	it.each(['dark', 'light', 'system'])('respects a saved %s surface and existing palette and wallpaper', async (mode) => {
		const saved = {
			'noor-palette': 'iris',
			'noor-theme': mode,
			'noor-wallpaper': 'aurora',
		};
		const values = stubStorage(saved);
		const stores = await appearance();
		expect(get(stores.palette)).toBe('iris');
		expect(get(stores.surfaceMode)).toBe(mode);
		expect(get(stores.wallpaper)).toBe('aurora');
		expect(Object.fromEntries(values)).toEqual(saved);
	});

	it('persists an explicit clay choice and restores it after a reload', async () => {
		const values = stubStorage({ 'noor-palette': 'futuro' });
		const stores = await appearance();
		stores.setPalette('clay');
		stores.surfaceMode.set('dark');
		stores.setWallpaper('none');
		expect(values.get('noor-palette')).toBe('clay');
		vi.resetModules();
		const restored = await appearance();
		expect(get(restored.palette)).toBe('clay');
		expect(get(restored.surfaceMode)).toBe('dark');
		expect(get(restored.wallpaper)).toBe('none');
	});

	it('falls back safely when stored appearance choices are invalid', async () => {
		stubStorage({
			'noor-palette': 'removed-palette',
			'noor-theme': 'invalid-mode',
			'noor-wallpaper': 'removed-wallpaper',
		});
		const stores = await appearance();
		expect(get(stores.palette)).toBe('clay');
		expect(get(stores.surfaceMode)).toBe('light');
		expect(get(stores.wallpaper)).toBe('none');
	});

	it.each(['missing', 'blocked'])('uses the parchment defaults when storage is %s', async (state) => {
		vi.stubGlobal('localStorage', state === 'missing' ? undefined : {
			getItem: () => { throw new Error('Storage blocked'); },
			setItem: () => { throw new Error('Storage blocked'); },
		});
		const stores = await appearance();
		expect(get(stores.palette)).toBe('clay');
		expect(get(stores.surfaceMode)).toBe('light');
		expect(get(stores.wallpaper)).toBe('none');
		expect(() => stores.setPalette('iris')).not.toThrow();
		expect(get(stores.palette)).toBe('iris');
	});
});
