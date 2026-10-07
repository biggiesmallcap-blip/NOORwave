import type { VideoStationCard } from '$lib/api/client';

export const STATION_ROWS = [
	{ id: 'for_you', title: 'For you' },
	{ id: 'genres', title: 'Your genres' },
	{ id: 'vibes', title: 'Vibes' },
	{ id: 'themes', title: 'Themes' },
	{ id: 'charts', title: 'Charts' },
] as const;

/** Below this the catalog is too thin for stations to feel endless. */
export const SMALL_CATALOG = 500;

export function groupStations(stations: VideoStationCard[]) {
	const spotlight = stations.find((station) => station.group === 'spotlight') ?? null;
	const rows = STATION_ROWS
		.map((row) => ({ ...row, stations: stations.filter((station) => station.group === row.id) }))
		.filter((row) => row.stations.length > 0);
	return { spotlight, rows };
}

export function spotlightArtistId(card: VideoStationCard): number | null {
	const match = /^spotlight:(\d+)$/.exec(card.id);
	return match ? Number(match[1]) : null;
}
