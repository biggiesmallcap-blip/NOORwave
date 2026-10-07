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

/** Distinct artist names in a station's preview, in order, at most `limit`. */
export function previewArtists(card: VideoStationCard, limit = 3): string[] {
	const seen = new Set<string>();
	const names: string[] = [];
	for (const video of card.preview) {
		const name = video.artist_name?.trim();
		if (!name || seen.has(name.toLowerCase())) continue;
		seen.add(name.toLowerCase());
		names.push(name);
		if (names.length >= limit) break;
	}
	return names;
}

/** The line under a station's title. A subtitle that other cards in the same
 *  row repeat word for word ("Artists you like and ones you...") says nothing,
 *  so those cards name the artists inside instead. */
export function stationMeta(card: VideoStationCard, row: VideoStationCard[]): string {
	const subtitle = card.subtitle.trim();
	const repeated = row.filter((other) => other.subtitle.trim() === subtitle).length > 1;
	if (subtitle && !repeated) return subtitle;
	const artists = previewArtists(card);
	if (artists.length > 0) return artists.join(', ');
	return `${card.unwatched_count} videos you have not seen`;
}
