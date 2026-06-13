import { get, writable, type Readable, type Writable } from 'svelte/store';

export type CacheKeyInput = string | readonly unknown[];

export interface CacheState<T> {
	data: T | undefined;
	loading: boolean;
	refreshing: boolean;
	error: unknown | null;
	lastUpdated: number | null;
	stale: boolean;
	hydrated: boolean;
}

export interface CacheStorage {
	getItem(key: string): string | null;
	setItem(key: string, value: string): void;
	removeItem(key: string): void;
}

export interface PersistOptions {
	storageKey?: string;
	maxAgeMs?: number;
	storage?: CacheStorage;
	namespace?: string | (() => string);
}

export interface QueryOptions {
	staleMs?: number;
	persist?: PersistOptions;
	revalidate?: boolean;
	returnStale?: boolean;
}

export interface CachedQuery<T> extends Readable<CacheState<T>> {
	key: string;
	refresh(): Promise<T>;
	invalidate(): void;
	patch(updater: (current: T | undefined) => T | undefined): void;
	getSnapshot(): CacheState<T>;
}

interface CacheEntry<T> {
	key: string;
	store: Writable<CacheState<T>>;
	fetcher?: () => Promise<T>;
	options: QueryOptions;
	inflight?: Promise<T>;
	persistHydrated: boolean;
}

interface PersistedEntry<T> {
	version: 1;
	lastUpdated: number;
	data: T;
}

const DEFAULT_STALE_MS = 60 * 1000;
const PERSIST_PREFIX = 'noor.query.';

function defaultState<T>(): CacheState<T> {
	return {
		data: undefined,
		loading: false,
		refreshing: false,
		error: null,
		lastUpdated: null,
		stale: true,
		hydrated: false,
	};
}

function defaultStorage(): CacheStorage | null {
	if (typeof localStorage === 'undefined') return null;
	return localStorage;
}

function persistNamespace(options: PersistOptions): string {
	try {
		const value = typeof options.namespace === 'function' ? options.namespace() : options.namespace;
		return value ? `${value}.` : '';
	} catch {
		return '';
	}
}

function persistKey(key: string, options: PersistOptions): string {
	const scopedKey = `${persistNamespace(options)}${options.storageKey ?? key}`;
	return `${PERSIST_PREFIX}${scopedKey}`;
}

function normalizeError(error: unknown): unknown {
	if (error instanceof Error) return error;
	return error;
}

export function stableStringify(value: unknown): string {
	if (value === null || typeof value !== 'object') return JSON.stringify(value);
	if (value instanceof Date) return JSON.stringify(value.toISOString());
	if (Array.isArray(value)) return `[${value.map((item) => stableStringify(item)).join(',')}]`;
	const entries = Object.entries(value as Record<string, unknown>)
		.filter(([, item]) => item !== undefined)
		.sort(([a], [b]) => a.localeCompare(b));
	return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${stableStringify(item)}`).join(',')}}`;
}

export function stableCacheKey(input: CacheKeyInput): string {
	if (typeof input === 'string') return input;
	return input
		.map((part) => (typeof part === 'string' ? part : stableStringify(part)))
		.join('|');
}

function keyMatchesPrefix(key: string, prefix: string): boolean {
	return key === prefix || key.startsWith(`${prefix}|`);
}

export class QueryCache {
	private entries = new Map<string, CacheEntry<unknown>>();
	private readonly now: () => number;

	constructor(options: { now?: () => number } = {}) {
		this.now = options.now ?? (() => Date.now());
	}

	query<T>(
		keyInput: CacheKeyInput,
		fetcher: () => Promise<T>,
		options: QueryOptions = {},
	): CachedQuery<T> {
		const key = stableCacheKey(keyInput);
		const entry = this.getEntry<T>(key);
		entry.fetcher = fetcher;
		entry.options = options;
		this.hydratePersisted(entry, options);
		this.refreshStaleFlag(entry, options);
		const snapshot = get(entry.store);
		if (options.revalidate !== false && (snapshot.data === undefined || snapshot.stale)) {
			void this.fetchEntry(entry, fetcher, options).catch(() => undefined);
		}
		return {
			key,
			subscribe: entry.store.subscribe,
			refresh: () => this.fetchEntry(entry, fetcher, options, true),
			invalidate: () => this.invalidateKey(key),
			patch: (updater) => this.patch(key, updater),
			getSnapshot: () => get(entry.store),
		};
	}

	fetchQuery<T>(
		keyInput: CacheKeyInput,
		fetcher: () => Promise<T>,
		options: QueryOptions = {},
	): Promise<T> {
		const key = stableCacheKey(keyInput);
		const entry = this.getEntry<T>(key);
		entry.fetcher = fetcher;
		entry.options = options;
		this.hydratePersisted(entry, options);
		this.refreshStaleFlag(entry, options);
		const snapshot = get(entry.store);
		if (snapshot.data !== undefined) {
			if (!snapshot.stale || options.revalidate === false) {
				return Promise.resolve(snapshot.data);
			}
			if (options.returnStale) {
				void this.fetchEntry(entry, fetcher, options).catch(() => undefined);
				return Promise.resolve(snapshot.data);
			}
		}
		return this.fetchEntry(entry, fetcher, options);
	}

	prefetch<T>(
		keyInput: CacheKeyInput,
		fetcher: () => Promise<T>,
		options: QueryOptions = {},
	): Promise<T> {
		return this.fetchQuery(keyInput, fetcher, options);
	}

	peek<T>(keyInput: CacheKeyInput): T | undefined {
		const entry = this.entries.get(stableCacheKey(keyInput));
		return entry ? (get(entry.store).data as T | undefined) : undefined;
	}

	getState<T>(keyInput: CacheKeyInput): CacheState<T> | null {
		const entry = this.entries.get(stableCacheKey(keyInput));
		return entry ? (get(entry.store) as CacheState<T>) : null;
	}

	prime<T>(
		keyInput: CacheKeyInput,
		data: T,
		options: QueryOptions = {},
		lastUpdated = this.now(),
	): void {
		const key = stableCacheKey(keyInput);
		const entry = this.getEntry<T>(key);
		entry.options = options;
		entry.store.set({
			data,
			loading: false,
			refreshing: false,
			error: null,
			lastUpdated,
			stale: false,
			hydrated: true,
		});
		this.savePersisted(key, data, lastUpdated, options);
	}

	invalidateKey(keyInput: CacheKeyInput, options: { refetch?: boolean } = {}): void {
		const key = stableCacheKey(keyInput);
		const entry = this.entries.get(key);
		if (!entry) return;
		entry.store.update((state) => ({ ...state, stale: true }));
		if (options.refetch && entry.fetcher) {
			void this.fetchEntry(entry, entry.fetcher, entry.options).catch(() => undefined);
		}
	}

	invalidatePrefix(prefixInput: CacheKeyInput, options: { refetch?: boolean } = {}): void {
		const prefix = stableCacheKey(prefixInput);
		for (const [key, entry] of this.entries) {
			if (!keyMatchesPrefix(key, prefix)) continue;
			entry.store.update((state) => ({ ...state, stale: true }));
			if (options.refetch && entry.fetcher) {
				void this.fetchEntry(entry, entry.fetcher, entry.options).catch(() => undefined);
			}
		}
	}

	invalidateWhere(predicate: (key: string) => boolean, options: { refetch?: boolean } = {}): void {
		for (const [key, entry] of this.entries) {
			if (!predicate(key)) continue;
			entry.store.update((state) => ({ ...state, stale: true }));
			if (options.refetch && entry.fetcher) {
				void this.fetchEntry(entry, entry.fetcher, entry.options).catch(() => undefined);
			}
		}
	}

	patch<T>(keyInput: CacheKeyInput, updater: (current: T | undefined) => T | undefined): void {
		const key = stableCacheKey(keyInput);
		const entry = this.getEntry<T>(key);
		let nextData: T | undefined;
		let nextLastUpdated = this.now();
		entry.store.update((state) => {
			nextData = updater(state.data);
			if (nextData === undefined) return state;
			nextLastUpdated = this.now();
			return {
				...state,
				data: nextData,
				error: null,
				lastUpdated: nextLastUpdated,
				stale: false,
				hydrated: true,
			};
		});
		if (nextData !== undefined) this.savePersisted(key, nextData, nextLastUpdated, entry.options);
	}

	patchWhere<T>(predicate: (key: string) => boolean, updater: (current: T | undefined, key: string) => T | undefined): void {
		for (const [key, entry] of this.entries) {
			if (!predicate(key)) continue;
			this.patch<T>(key, (current) => updater(current, key));
		}
	}

	deleteKey(keyInput: CacheKeyInput): void {
		const key = stableCacheKey(keyInput);
		const entry = this.entries.get(key);
		if (entry?.options.persist) {
			const storage = entry.options.persist.storage ?? defaultStorage();
			try {
				storage?.removeItem(persistKey(key, entry.options.persist));
			} catch {}
		}
		this.entries.delete(key);
	}

	clear(): void {
		this.entries.clear();
	}

	/// Remove persisted entries that belong to a DIFFERENT namespace than `namespace`.
	/// `clear()` only drops in-memory entries, so without this a token/api-base change
	/// would orphan every `noor.query.<oldns>.*` key in localStorage forever - a
	/// permanent leak that also eats into the storage quota and can silently break
	/// instant-paint once the quota is hit.
	sweepForeignPersisted(namespace: string): void {
		if (typeof localStorage === 'undefined') return;
		const keepPrefix = `${PERSIST_PREFIX}${namespace}.`;
		try {
			const stale: string[] = [];
			for (let index = 0; index < localStorage.length; index += 1) {
				const key = localStorage.key(index);
				if (key && key.startsWith(PERSIST_PREFIX) && !key.startsWith(keepPrefix)) {
					stale.push(key);
				}
			}
			for (const key of stale) localStorage.removeItem(key);
		} catch {}
	}

	private getEntry<T>(key: string): CacheEntry<T> {
		const existing = this.entries.get(key);
		if (existing) return existing as CacheEntry<T>;
		const entry: CacheEntry<T> = {
			key,
			store: writable(defaultState<T>()),
			options: {},
			persistHydrated: false,
		};
		this.entries.set(key, entry as CacheEntry<unknown>);
		return entry;
	}

	private isStale<T>(state: CacheState<T>, options: QueryOptions): boolean {
		if (state.lastUpdated === null) return true;
		return this.now() - state.lastUpdated >= (options.staleMs ?? DEFAULT_STALE_MS);
	}

	private refreshStaleFlag<T>(entry: CacheEntry<T>, options: QueryOptions): void {
		entry.store.update((state) => ({
			...state,
			stale: this.isStale(state, options),
		}));
	}

	private hydratePersisted<T>(entry: CacheEntry<T>, options: QueryOptions): void {
		if (entry.persistHydrated) return;
		entry.persistHydrated = true;
		if (!options.persist) return;
		const storage = options.persist.storage ?? defaultStorage();
		if (!storage) return;
		try {
			const raw = storage.getItem(persistKey(entry.key, options.persist));
			if (!raw) return;
			const parsed = JSON.parse(raw) as PersistedEntry<T>;
			if (parsed.version !== 1 || parsed.lastUpdated == null || !('data' in parsed)) return;
			if (
				options.persist.maxAgeMs !== undefined &&
				this.now() - parsed.lastUpdated > options.persist.maxAgeMs
			) return;
			entry.store.set({
				data: parsed.data,
				loading: false,
				refreshing: false,
				error: null,
				lastUpdated: parsed.lastUpdated,
				stale: this.isStale(
					{ ...defaultState<T>(), data: parsed.data, lastUpdated: parsed.lastUpdated },
					options,
				),
				hydrated: true,
			});
		} catch {}
	}

	private savePersisted<T>(key: string, data: T, lastUpdated: number, options: QueryOptions): void {
		if (!options.persist) return;
		const storage = options.persist.storage ?? defaultStorage();
		if (!storage) return;
		try {
			const payload: PersistedEntry<T> = { version: 1, lastUpdated, data };
			storage.setItem(persistKey(key, options.persist), JSON.stringify(payload));
		} catch (error) {
			// Quota exceeded (or storage disabled): the prior persisted value, if any,
			// still hydrates, so a surface can silently keep painting stale content
			// instead of failing loudly. Surface it rather than swallowing in silence.
			console.warn(`[noor.cache] failed to persist "${key}"; instant-paint may degrade`, error);
		}
	}

	private fetchEntry<T>(
		entry: CacheEntry<T>,
		fetcher: () => Promise<T>,
		options: QueryOptions,
		force = false,
	): Promise<T> {
		if (entry.inflight) return entry.inflight;
		const before = get(entry.store);
		entry.store.update((state) => ({
			...state,
			loading: state.data === undefined,
			refreshing: state.data !== undefined,
			error: force ? null : state.error,
		}));
		entry.inflight = fetcher()
			.then((data) => {
				const lastUpdated = this.now();
				entry.store.set({
					data,
					loading: false,
					refreshing: false,
					error: null,
					lastUpdated,
					stale: false,
					hydrated: true,
				});
				this.savePersisted(entry.key, data, lastUpdated, options);
				return data;
			})
			.catch((error) => {
				const normalized = normalizeError(error);
				entry.store.update((state) => ({
					...state,
					loading: false,
					refreshing: false,
					error: normalized,
					stale: true,
				}));
				throw error;
			})
			.finally(() => {
				entry.inflight = undefined;
			});
		return entry.inflight;
	}
}

export const dataCache = new QueryCache();
