import { api } from '$lib/api/client';

/**
 * Svelte action: when the host element scrolls into view, search Tidal once
 * for a track ("artist + title") or artist ("artist") and pass the resulting
 * cover URL back via the `onResolve` callback.
 *
 * The IntersectionObserver is wired up once at mount and never re-attached,
 * so reactive param updates don't keep tearing it down before it can fire.
 *
 * All searches across the page funnel through a small in-flight cap and a
 * memo cache. Without these the trending shelf alone can fire 20+ parallel
 * /api/tidal/search calls, which Tidal rejects with 400 — see the network
 * panel "Bad Request" flood that motivated this guard.
 */
export type LazyTidalArtParams = {
	enabled: boolean;
	query: { artist: string | null | undefined; title?: string | null };
	onResolve: (url: string) => void;
	rootMargin?: string;
};

const MAX_INFLIGHT = 4;
let inflight = 0;
const queue: Array<() => void> = [];
const pending = new Map<string, Promise<string | null>>();

// Circuit breaker: if /api/tidal/search returns the same error several times in
// a row (typically "TIDAL not connected" → 400), pause further searches for a
// short cooldown rather than the rest of the session. A permanent latch meant a
// single transient Tidal blip early in a session blanked every artwork tile
// until a full reload; the cooldown lets tiles recover on their own once Tidal
// comes back. Avoids burning through 25+ requests every render while offline.
const CIRCUIT_OPEN_AFTER = 3;
const CIRCUIT_COOLDOWN_MS = 60 * 1000;
let consecutiveFailures = 0;
let circuitOpenUntil = 0;

function isCircuitOpen(): boolean {
	return Date.now() < circuitOpenUntil;
}

// ── Persistent cache ─────────────────────────────────────────────────────────
// Stash resolved (and "no-result") artwork lookups in localStorage keyed by
// the search query. Hits persist for 30 days since last seen; misses for 24h
// so a track that wasn't on Tidal yesterday gets retried tomorrow.

type CacheEntry = { url: string | null; lastSeen: number };

const STORAGE_KEY = 'noor.lazyTidalArt.v1';
const HIT_TTL_MS = 30 * 24 * 60 * 60 * 1000; // 30 days
const MISS_TTL_MS = 24 * 60 * 60 * 1000; // 24 hours
const PERSIST_DEBOUNCE_MS = 1000;

const cache = new Map<string, CacheEntry>();

function ttlFor(entry: CacheEntry): number {
	return entry.url === null ? MISS_TTL_MS : HIT_TTL_MS;
}

function isFresh(entry: CacheEntry, now = Date.now()): boolean {
	return now - entry.lastSeen <= ttlFor(entry);
}

/**
 * Build the same search query the action resolves against, so callers can look
 * artwork up in the shared cache without duplicating the join rules. Returns
 * null when there's nothing searchable (no artist).
 */
export function composeTidalArtQuery(
	artist: string | null | undefined,
	title?: string | null,
): string | null {
	if (!artist) return null;
	const a = artist.trim();
	if (!a) return null;
	const t = title?.trim() ?? '';
	return t ? `${a} ${t}` : a;
}

/**
 * Synchronous peek into the persistent artwork cache. Returns a resolved cover
 * URL only for a *fresh hit* (cached misses, stale entries, and absent queries
 * all return null). Lets a shelf paint previously-seen artwork on the very
 * first frame instead of flashing a fallback tile until the on-scroll
 * IntersectionObserver lookup fires. Read-only: does not touch the LRU.
 */
export function peekTidalArt(query: string | null | undefined): string | null {
	if (!query) return null;
	const entry = cache.get(query);
	if (entry && isFresh(entry)) return entry.url;
	return null;
}

function hydrateFromStorage(): void {
	if (typeof localStorage === 'undefined') return;
	const raw = localStorage.getItem(STORAGE_KEY);
	if (!raw) return;
	try {
		const parsed = JSON.parse(raw) as Record<string, CacheEntry>;
		const now = Date.now();
		for (const [query, entry] of Object.entries(parsed)) {
			if (!entry || typeof entry.lastSeen !== 'number') continue;
			if (isFresh(entry, now)) cache.set(query, entry);
		}
	} catch {
		// Corrupted blob; nuke it and start clean.
		try {
			localStorage.removeItem(STORAGE_KEY);
		} catch {
			/* ignore */
		}
	}
}

let persistTimer: ReturnType<typeof setTimeout> | null = null;
function schedulePersist(): void {
	if (typeof localStorage === 'undefined') return;
	if (persistTimer !== null) return;
	persistTimer = setTimeout(() => {
		persistTimer = null;
		try {
			const obj: Record<string, CacheEntry> = {};
			for (const [k, v] of cache) obj[k] = v;
			localStorage.setItem(STORAGE_KEY, JSON.stringify(obj));
		} catch {
			// Quota exceeded or storage disabled — keep working in-memory only.
		}
	}, PERSIST_DEBOUNCE_MS);
}

hydrateFromStorage();

async function acquireSlot(): Promise<void> {
	if (inflight < MAX_INFLIGHT) {
		inflight++;
		return;
	}
	await new Promise<void>((resolve) => queue.push(resolve));
	inflight++;
}

function releaseSlot(): void {
	inflight--;
	const next = queue.shift();
	if (next) next();
}

function recordResult(query: string, url: string | null): void {
	cache.set(query, { url, lastSeen: Date.now() });
	schedulePersist();
}

async function lookupArtwork(query: string): Promise<string | null> {
	if (isCircuitOpen()) return null;
	const cached = cache.get(query);
	if (cached !== undefined) {
		if (isFresh(cached)) {
			// Touch lastSeen so active entries stay warm in the LRU.
			cached.lastSeen = Date.now();
			schedulePersist();
			return cached.url;
		}
		cache.delete(query);
	}
	const inProgress = pending.get(query);
	if (inProgress) return inProgress;

	const work = (async () => {
		await acquireSlot();
		if (isCircuitOpen()) {
			releaseSlot();
			return null;
		}
		try {
			const result = await api.searchTidal(query, 1);
			consecutiveFailures = 0;
			const url = result.tracks[0]?.artwork_url ?? null;
			recordResult(query, url);
			return url;
		} catch {
			consecutiveFailures++;
			if (consecutiveFailures >= CIRCUIT_OPEN_AFTER) {
				circuitOpenUntil = Date.now() + CIRCUIT_COOLDOWN_MS;
				consecutiveFailures = 0;
				console.warn(
					`[lazyTidalArt] circuit breaker tripped — pausing Tidal artwork lookups for ` +
						`${CIRCUIT_COOLDOWN_MS / 1000}s. Reconnect Tidal in Settings if this persists.`,
				);
			}
			recordResult(query, null);
			return null;
		} finally {
			pending.delete(query);
			releaseSlot();
		}
	})();
	pending.set(query, work);
	return work;
}

export function lazyTidalArt(node: Element, initial: LazyTidalArtParams) {
	let current: LazyTidalArtParams = initial;
	let attempted = false;
	let aborted = false;

	function buildQuery(): string | null {
		return composeTidalArtQuery(current.query.artist, current.query.title);
	}

	async function fetchOnce() {
		if (attempted || aborted) return;
		if (!current.enabled) return;
		const q = buildQuery();
		if (!q) return;
		attempted = true;
		const url = await lookupArtwork(q);
		if (aborted || !url) return;
		current.onResolve(url);
	}

	let observer: IntersectionObserver | null = null;
	if (typeof IntersectionObserver === 'undefined') {
		void fetchOnce();
	} else {
		observer = new IntersectionObserver(
			(entries) => {
				for (const e of entries) {
					if (e.isIntersecting) {
						void fetchOnce();
						observer?.disconnect();
						observer = null;
						break;
					}
				}
			},
			{ rootMargin: initial.rootMargin ?? '200px' },
		);
		observer.observe(node);
	}

	return {
		update(next: LazyTidalArtParams) {
			current = next;
		},
		destroy() {
			aborted = true;
			observer?.disconnect();
			observer = null;
		},
	};
}
