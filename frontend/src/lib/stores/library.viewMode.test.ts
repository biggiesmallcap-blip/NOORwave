import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';

// Regression: a full/blocked localStorage must never crash module init. The
// viewMode store persists via store.subscribe(), which fires synchronously at
// import time; an unguarded setItem there threw QuotaExceededError out of
// module init and took the whole app down to a bare SvelteKit 500 on boot.

function stubStorage(setItem: (k: string, v: string) => void) {
	const backing = new Map<string, string>();
	vi.stubGlobal('localStorage', {
		getItem: (k: string) => backing.get(k) ?? null,
		setItem,
		removeItem: (k: string) => backing.delete(k),
		key: () => null,
		clear: () => backing.clear(),
		get length() {
			return backing.size;
		},
	});
	return backing;
}

describe('library viewMode store', () => {
	beforeEach(() => {
		vi.resetModules();
	});
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it('does not throw at import when localStorage.setItem exceeds quota', async () => {
		stubStorage(() => {
			throw new DOMException('exceeded the quota', 'QuotaExceededError');
		});
		// The import itself runs createViewMode(); it must not throw.
		const mod = await import('./library');
		expect(get(mod.viewMode)).toBe('grid');
		// A later user change must also degrade gracefully, not throw.
		expect(() => mod.viewMode.set('list')).not.toThrow();
		expect(get(mod.viewMode)).toBe('list');
	});

	it('persists real changes but never writes the initial value on subscribe', async () => {
		const writes: Array<[string, string]> = [];
		const backing = stubStorage((k, v) => {
			writes.push([k, v]);
			backing.set(k, v);
		});
		const mod = await import('./library');
		// The synchronous initial emission must not persist anything.
		expect(writes).toHaveLength(0);
		mod.viewMode.set('list');
		expect(writes).toEqual([['library.viewMode', 'list']]);
		expect(backing.get('library.viewMode')).toBe('list');
	});

	it('hydrates the initial value from a saved choice', async () => {
		const backing = stubStorage((k, v) => backing.set(k, v));
		backing.set('library.viewMode', 'list');
		const mod = await import('./library');
		expect(get(mod.viewMode)).toBe('list');
	});
});
