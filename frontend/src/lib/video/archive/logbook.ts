// The Logbook's numbers: the census as a few plain figures and per-scene
// "seen N of M" rows. Pure functions; no badges, no streaks.

import type { AtlasScene, VideoCensus, VideoCut } from '$lib/api/video_explore_types';
import { CUTS, CUT_LABELS } from './lenses';

export interface Figure {
	label: string;
	value: string;
	note?: string;
}

function n(value: number | null | undefined): string {
	return Math.round(value ?? 0).toLocaleString('en-US');
}

function pct(value: number | null | undefined): string {
	const v = Math.max(0, Math.min(1, value ?? 0));
	return `${Math.round(v * 100)}%`;
}

/** The catalog in six numbers. */
export function censusFigures(census: VideoCensus): Figure[] {
	return [
		{ label: 'Videos', value: n(census.videos), note: `${n(census.programmable)} play on air` },
		{ label: 'Hours', value: n(census.hours) },
		{ label: 'Songs', value: n(census.songs), note: `${n(census.multi_cut_songs)} in several cuts` },
		{ label: 'Scenes', value: n(census.scenes), note: census.channel_scenes ? `${n(census.channel_scenes)} with a channel` : undefined },
		{ label: 'Concerts', value: n(census.concerts) },
		{ label: 'Artists', value: n(census.artists), note: `${n(census.charted_artists)} on the map` },
	];
}

/** How well the catalog knows itself. */
export function coverageFigures(census: VideoCensus): Figure[] {
	return [
		{ label: 'Song year known', value: pct(census.era_coverage) },
		{ label: 'Genre known', value: pct(census.genre_coverage), note: 'of artists on the map' },
		{
			label: 'Charted',
			value: pct(census.artists > 0 ? census.charted_artists / census.artists : 0),
			note: `${n(census.uncharted_artists)} artists still uncharted`,
		},
	];
}

export interface CutShare {
	cut: VideoCut;
	label: string;
	count: number;
	share: number;
}

/** Cuts biggest first, as shares of the largest (for bar widths). */
export function cutShares(census: VideoCensus): CutShare[] {
	const rows = CUTS.map((cut) => ({ cut, label: CUT_LABELS[cut], count: census.cuts?.[cut] ?? 0 }))
		.filter((row) => row.count > 0)
		.sort((a, b) => b.count - a.count);
	const max = Math.max(1, ...rows.map((row) => row.count));
	return rows.map((row) => ({ ...row, share: row.count / max }));
}

export interface SceneCoverage {
	id: number;
	name: string;
	hue: number;
	seen: number;
	videos: number;
	ratio: number;
}

/** Scenes you have been to first (most of them seen), then the rest by size.
 *  `seen` never exceeds `videos`, so a bar never overflows. */
export function sceneCoverage(
	scenes: readonly Pick<AtlasScene, 'id' | 'name' | 'hue' | 'videos' | 'seen_videos'>[]
): SceneCoverage[] {
	return scenes
		.map((scene) => {
			const videos = Math.max(0, scene.videos);
			const seen = Math.min(videos, Math.max(0, scene.seen_videos));
			return { id: scene.id, name: scene.name, hue: scene.hue, seen, videos, ratio: videos > 0 ? seen / videos : 0 };
		})
		.sort((a, b) => {
			if ((a.seen > 0) !== (b.seen > 0)) return a.seen > 0 ? -1 : 1;
			return b.ratio - a.ratio || b.videos - a.videos || a.id - b.id;
		});
}

/** "seen 42 of 1,830". */
export function seenLabel(row: Pick<SceneCoverage, 'seen' | 'videos'>): string {
	return `seen ${n(row.seen)} of ${n(row.videos)}`;
}

const DAY_MS = 86_400_000;

/** When a More / Less press was made: "today", "yesterday", "3 days ago",
 *  "2 weeks ago". */
export function steerAge(atS: number, nowMs = Date.now()): string {
	const days = Math.max(0, Math.floor((nowMs - atS * 1000) / DAY_MS));
	if (days === 0) return 'today';
	if (days === 1) return 'yesterday';
	if (days < 14) return `${days} days ago`;
	return `${Math.floor(days / 7)} weeks ago`;
}
