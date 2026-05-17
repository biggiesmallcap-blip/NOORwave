import type { TidalHomeModule, TidalMoodCategory } from '$lib/api/client';

/// In-memory caches for /moods landing + per-mood drill-down pages.
/// Same 6h TTL pattern as tidal-home-modules-cache — TIDAL's editorial moods
/// rotate slowly, so revisiting either page within a session should render
/// instantly without re-fetching. Cleared on hard reload (app restart).

const TTL_MS = 6 * 60 * 60 * 1000;

interface MoodLandingEntry {
	categories: TidalMoodCategory[];
	insertedAt: number;
}

interface MoodPageEntry {
	modules: TidalHomeModule[];
	insertedAt: number;
}

let landing: MoodLandingEntry | null = null;
const drilldown = new Map<string, MoodPageEntry>();

export function getCachedMoodCategories(): TidalMoodCategory[] | null {
	if (!landing) return null;
	if (Date.now() - landing.insertedAt > TTL_MS) {
		landing = null;
		return null;
	}
	return landing.categories;
}

export function putCachedMoodCategories(categories: TidalMoodCategory[]): void {
	landing = { categories, insertedAt: Date.now() };
}

export function getCachedMoodPage(slug: string): TidalHomeModule[] | null {
	const e = drilldown.get(slug);
	if (!e) return null;
	if (Date.now() - e.insertedAt > TTL_MS) {
		drilldown.delete(slug);
		return null;
	}
	return e.modules;
}

export function putCachedMoodPage(slug: string, modules: TidalHomeModule[]): void {
	drilldown.set(slug, { modules, insertedAt: Date.now() });
}

export function clearCachedMoods(): void {
	landing = null;
	drilldown.clear();
}
