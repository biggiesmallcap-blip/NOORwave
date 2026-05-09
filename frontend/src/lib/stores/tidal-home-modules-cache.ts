import type { TidalHomeModule } from '$lib/api/client';

/// In-memory cache for the search-page DiscoverShelves payload. Mirrors the
/// 6h TTL used for mixes/radio so revisiting Search doesn't trigger a
/// skeleton flash + network round-trip — the shelves read cached modules
/// synchronously on mount and only fetch on miss/expiry. Cleared on hard
/// reload (app restart).
///
/// Tidal's `pages/home` editorial content rotates on roughly a daily cadence,
/// so 6h gives a fresh-enough surface without re-querying on every nav.

const TTL_MS = 6 * 60 * 60 * 1000;

interface Entry {
	modules: TidalHomeModule[];
	insertedAt: number;
}

let entry: Entry | null = null;

export function getCachedHomeModules(): TidalHomeModule[] | null {
	if (!entry) return null;
	if (Date.now() - entry.insertedAt > TTL_MS) {
		entry = null;
		return null;
	}
	return entry.modules;
}

export function putCachedHomeModules(modules: TidalHomeModule[]): void {
	entry = { modules, insertedAt: Date.now() };
}

export function clearCachedHomeModules(): void {
	entry = null;
}
