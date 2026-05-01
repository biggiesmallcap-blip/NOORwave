import type { ChartEntry } from '$lib/api/client';

/// In-memory cache for trending chart payloads, keyed by scope token.
/// Mirrors the 6h backend TTL so navigating between Home and Search (which
/// each remount their TrendingShelf) doesn't kick off a new request — the
/// shelf reads from this cache first and only fetches on miss/expiry.
///
/// Module-level so all TrendingShelf instances on the page share the same
/// cache. Cleared on hard reload (which is the explicit "refresh" gesture).

const TTL_MS = 6 * 60 * 60 * 1000; // 6 hours
const MAX_ENTRIES = 32;

interface CacheEntry {
	tracks: ChartEntry[];
	insertedAt: number;
}

const cache = new Map<string, CacheEntry>();

export function getCached(token: string): ChartEntry[] | null {
	const entry = cache.get(token);
	if (!entry) return null;
	if (Date.now() - entry.insertedAt > TTL_MS) {
		cache.delete(token);
		return null;
	}
	return entry.tracks;
}

export function putCached(token: string, tracks: ChartEntry[]): void {
	if (cache.size >= MAX_ENTRIES) {
		// Drop the oldest entry. Map iteration order = insertion order.
		const oldestKey = cache.keys().next().value;
		if (oldestKey !== undefined) cache.delete(oldestKey);
	}
	cache.set(token, { tracks, insertedAt: Date.now() });
}
