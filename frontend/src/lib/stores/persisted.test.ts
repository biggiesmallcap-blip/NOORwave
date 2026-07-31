import { describe, it, expect, vi, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { createPersistedStore, createPersistedJsonStore, oneOf, readPersisted } from './persisted';

// `library.viewMode.test.ts` covers the boot-crash regression against a real
// consumer. These cover the helper's own contract: a full or blocked storage
// degrades to memory rather than throwing, and a stored value that no longer
// parses falls back to the default instead of reaching the app malformed.

function stubStorage(overrides: Partial<Storage> = {}) {
	const backing = new Map<string, string>();
	vi.stubGlobal('localStorage', {
		getItem: (k: string) => backing.get(k) ?? null,
		setItem: (k: string, v: string) => void backing.set(k, v),
		removeItem: (k: string) => void backing.delete(k),
		key: () => null,
		clear: () => backing.clear(),
		get length() {
			return backing.size;
		},
		...overrides,
	});
	return backing;
}

describe('createPersistedStore', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('hydrates from storage and writes later changes', () => {
		const backing = stubStorage();
		backing.set('k', 'list');
		const store = createPersistedStore('k', 'grid');
		expect(get(store)).toBe('list');
		store.set('grid');
		expect(backing.get('k')).toBe('grid');
	});

	it('never writes the initial value on the synchronous first emission', () => {
		const writes: string[] = [];
		stubStorage({ setItem: (k: string) => void writes.push(k) });
		createPersistedStore('k', 'grid');
		expect(writes).toEqual([]);
	});

	it('survives a storage that throws on write', () => {
		stubStorage({
			setItem: () => {
				throw new DOMException('exceeded the quota', 'QuotaExceededError');
			},
		});
		const store = createPersistedStore('k', 'grid');
		expect(() => store.set('list')).not.toThrow();
		// The choice still applies in memory for this session.
		expect(get(store)).toBe('list');
	});

	it('survives a storage that throws on read', () => {
		// Storage present but blocked by policy throws rather than returning null.
		stubStorage({
			getItem: () => {
				throw new DOMException('access denied', 'SecurityError');
			},
		});
		expect(() => createPersistedStore('k', 'grid')).not.toThrow();
		expect(get(createPersistedStore('k', 'grid'))).toBe('grid');
	});

	it('falls back to the default when localStorage is absent', () => {
		vi.stubGlobal('localStorage', undefined);
		const store = createPersistedStore('k', 'grid');
		expect(get(store)).toBe('grid');
		expect(() => store.set('list')).not.toThrow();
	});

	it('rejects a stored value that is no longer valid', () => {
		// The shape a build change causes: the key holds a mode that was removed.
		const backing = stubStorage();
		backing.set('sort', 'by_vibes');
		const store = createPersistedStore('sort', 'recent_update', {
			parse: oneOf(['recent_update', 'name'] as const),
		});
		expect(get(store)).toBe('recent_update');
	});
});

describe('createPersistedJsonStore', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('round-trips a non-string value', () => {
		const backing = stubStorage();
		const store = createPersistedJsonStore('k', { a: 1 });
		store.set({ a: 2 });
		expect(backing.get('k')).toBe('{"a":2}');
		expect(get(createPersistedJsonStore('k', { a: 1 }))).toEqual({ a: 2 });
	});

	it('falls back to the default on malformed JSON', () => {
		const backing = stubStorage();
		backing.set('k', 'not json');
		expect(get(createPersistedJsonStore('k', { a: 1 }))).toEqual({ a: 1 });
	});
});

describe('readPersisted', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('returns the fallback for a missing key', () => {
		stubStorage();
		expect(readPersisted('nope', 'fallback')).toBe('fallback');
	});
});
