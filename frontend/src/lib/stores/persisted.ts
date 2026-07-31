import { writable, type Writable } from 'svelte/store';

// A writable store whose value survives a reload and a relaunch.
//
// SvelteKit's `Snapshot` is per-history-entry sessionStorage: it restores on
// back/forward and on nothing else, so a user preference stored there resets
// every time the page is reached fresh. Preferences belong here instead.
//
// The three guards below are the whole point of this module, and every one of
// them was learned the hard way:
//
// 1. `typeof localStorage` check. It is absent during SSR and in some embedded
//    webviews.
// 2. try/catch on BOTH read and write. A write can throw QuotaExceededError; a
//    read can throw when storage is disabled by policy rather than merely
//    missing.
// 3. Skip the first `subscribe` emission. `subscribe()` fires synchronously
//    with the initial value at import time, so an unguarded write there escapes
//    module init and takes the whole app down to a bare SvelteKit 500 on boot.
//    See `library.viewMode.test.ts` for the regression test.
//
// Before this existed the same idiom was hand-written in a dozen stores, each
// with its own subset of the guards. Add new preferences here, not there.

export interface PersistedStoreOptions<T> {
	/**
	 * Turn a stored string back into a value. Return `undefined` to reject the
	 * stored string and keep `initial` - that is how you validate a union type
	 * against a key some older build may have written something else into.
	 *
	 * Defaults to passing the raw string through, which is only correct when `T`
	 * is a string type.
	 */
	parse?: (raw: string) => T | undefined;
	/** Turn a value into its stored string. Defaults to `String(value)`. */
	serialize?: (value: T) => string;
}

export function readPersisted<T>(
	key: string,
	fallback: T,
	parse?: (raw: string) => T | undefined,
): T {
	if (typeof localStorage === 'undefined') return fallback;
	let raw: string | null = null;
	try {
		raw = localStorage.getItem(key);
	} catch {
		// Storage blocked (private mode, enterprise policy); use the default.
		return fallback;
	}
	if (raw === null) return fallback;
	const parsed = parse ? parse(raw) : (raw as unknown as T);
	return parsed === undefined ? fallback : parsed;
}

export function writePersisted(key: string, value: string): void {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(key, value);
	} catch {
		// Quota exceeded or storage blocked; keep the choice in memory only.
	}
}

export function createPersistedStore<T>(
	key: string,
	initial: T,
	options: PersistedStoreOptions<T> = {},
): Writable<T> {
	const { parse, serialize } = options;
	const store = writable<T>(readPersisted(key, initial, parse));
	let firstEmission = true;
	store.subscribe((value) => {
		if (firstEmission) {
			firstEmission = false;
			return;
		}
		writePersisted(key, serialize ? serialize(value) : String(value));
	});
	return store;
}

/**
 * `createPersistedStore` for values that are not strings. Rejects a stored
 * string that no longer parses, so a shape change in a later build degrades to
 * the default instead of handing the app a malformed object.
 */
export function createPersistedJsonStore<T>(
	key: string,
	initial: T,
	validate?: (value: unknown) => value is T,
): Writable<T> {
	return createPersistedStore<T>(key, initial, {
		parse: (raw) => {
			try {
				const value: unknown = JSON.parse(raw);
				if (validate && !validate(value)) return undefined;
				return value as T;
			} catch {
				return undefined;
			}
		},
		serialize: (value) => JSON.stringify(value),
	});
}

/**
 * Build a `parse` that only accepts one of a known set of strings. The common
 * case: a string-union preference such as a sort mode or a view mode.
 */
export function oneOf<T extends string>(allowed: readonly T[]): (raw: string) => T | undefined {
	return (raw) => (allowed as readonly string[]).includes(raw) ? (raw as T) : undefined;
}
