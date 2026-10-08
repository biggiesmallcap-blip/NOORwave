import type { Genre, GenreAudioMetrics, GenreCohort, GenreEvolutionPoint, GenreHeat } from '$lib/api/client';
import { ROOT_FAMILY_COLORS, familyKeyFromSlug } from './galaxy.types';
import { varyFamilyColor } from './galaxyColor';

/** Same shape as the cached galaxy snapshot (`cachedApi.getGenreGalaxySnapshot(90)`). */
export type GenreSnapshot = {
	genres: Genre[];
	heat: GenreHeat[];
	cohorts: GenreCohort[];
	evolution: GenreEvolutionPoint[];
	metrics: GenreAudioMetrics[];
};

export interface GenreSummary {
	id: number;
	name: string;
	color: string;
	familyId: number;
	familyName: string;
	/** Root down to the parent; excludes the genre itself. */
	lineage: { id: number; name: string }[];
	children: { id: number; name: string; trackCount: number }[];
	trackCount: number;
	listenCount: number;
	totalListenedMs: number;
	avgBpm: number | null;
	avgEnergy: number | null;
	avgDanceability: number | null;
	evolutionHistory: { periodStart: string; listenCount: number }[];
	cohort: { label: string; title: string } | null;
}

// Backend cohort ids (noor-server queries.rs, time slot x day type) in plain words.
const COHORT_PHRASES: Record<string, string> = {
	night_owl: 'Mostly late nights',
	morning_commute: 'Mostly weekday mornings',
	lazy_morning: 'Mostly weekend mornings',
	afternoon_drift: 'Mostly weekday afternoons',
	weekend_afternoon: 'Mostly weekend afternoons',
	evening_wind_down: 'Mostly weekday evenings',
	weekend_evening: 'Mostly weekend evenings'
};

export function cohortPhrase(cohortId: string | null | undefined): string | null {
	return cohortId ? (COHORT_PHRASES[cohortId] ?? null) : null;
}

function findPath(nodes: Genre[], id: number): Genre[] | null {
	for (const node of nodes) {
		if (node.id === id) return [node];
		const rest = findPath(node.children ?? [], id);
		if (rest) return [node, ...rest];
	}
	return null;
}

export function buildGenreSummary(snapshot: GenreSnapshot, id: number): GenreSummary | null {
	const path = findPath(snapshot.genres, id);
	if (!path) return null;
	const genre = path[path.length - 1];
	const root = path[0];
	const familyColor = ROOT_FAMILY_COLORS[familyKeyFromSlug(root.slug)].color;
	const heat = snapshot.heat.find((entry) => entry.genre_id === id);
	const metrics = snapshot.metrics.find((entry) => entry.genre_id === id);
	const cohort = snapshot.cohorts.find((entry) => (entry.genre_ids ?? []).includes(id));
	const phrase = cohortPhrase(cohort?.id);

	return {
		id,
		name: genre.name,
		// Same shade the galaxy draws for this node (galaxyBuilder uses the same call).
		color: varyFamilyColor(familyColor, id, path.length - 1),
		familyId: root.id,
		familyName: root.name,
		lineage: path.slice(0, -1).map((node) => ({ id: node.id, name: node.name })),
		children: (genre.children ?? [])
			.filter((child) => (child.track_count ?? 0) > 0)
			.map((child) => ({ id: child.id, name: child.name, trackCount: child.track_count ?? 0 }))
			.sort((a, b) => b.trackCount - a.trackCount || a.name.localeCompare(b.name)),
		trackCount: genre.track_count ?? 0,
		listenCount: heat?.listen_count ?? 0,
		totalListenedMs: heat?.total_listened_ms ?? 0,
		avgBpm: metrics?.avg_bpm ?? null,
		avgEnergy: metrics?.avg_energy ?? null,
		avgDanceability: metrics?.avg_danceability ?? null,
		evolutionHistory: snapshot.evolution
			.filter((point) => point.genre_id === id)
			.map((point) => ({ periodStart: point.period_start, listenCount: point.listen_count }))
			.sort((a, b) => a.periodStart.localeCompare(b.periodStart)),
		cohort: phrase ? { label: phrase, title: 'When you play this genre most, over the last 90 days' } : null
	};
}
