import type { TidalMix } from '$lib/api/client';

/// In-memory cache for the home Your Mixes shelf payload. Mirrors the 6h
/// backend TTL so revisiting Home doesn't trigger a skeleton flash + network
/// round-trip — the shelf reads cached mixes synchronously on mount and only
/// fetches on miss/expiry. Cleared on hard reload (app restart).

const TTL_MS = 6 * 60 * 60 * 1000;

interface Entry {
	mixes: TidalMix[];
	insertedAt: number;
}

let entry: Entry | null = null;

export function getCachedMixes(): TidalMix[] | null {
	if (!entry) return null;
	if (Date.now() - entry.insertedAt > TTL_MS) {
		entry = null;
		return null;
	}
	return entry.mixes;
}

export function putCachedMixes(mixes: TidalMix[]): void {
	entry = { mixes, insertedAt: Date.now() };
}

export function clearCachedMixes(): void {
	entry = null;
}
