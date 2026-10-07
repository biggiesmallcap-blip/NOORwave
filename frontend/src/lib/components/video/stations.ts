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

/** A station's filmstrip: its preview videos that have a frame to show. */
export function stationFrames(card: VideoStationCard): VideoStationCard['preview'] {
	return card.preview.filter((video) => Boolean(video.artwork_url));
}

/** Channel numbers down the guide: the spotlight is 01, then every row in
 *  display order. */
export function numberStations(
	spotlight: VideoStationCard | null,
	rows: { stations: VideoStationCard[] }[],
): Map<string, string> {
	const order = [...(spotlight ? [spotlight] : []), ...rows.flatMap((row) => row.stations)];
	return new Map(order.map((station, index) => [station.id, String(index + 1).padStart(2, '0')]));
}

const BIO_MAX = 240;
const BIO_MIN = 40;
const NAMED_ENTITIES: Record<string, string> = { amp: '&', quot: '"', apos: "'", lt: '<', gt: '>', nbsp: ' ' };

function decodeEntity(match: string, code: string): string {
	const lower = code.toLowerCase();
	if (lower in NAMED_ENTITIES) return NAMED_ENTITIES[lower];
	const point = lower.startsWith('#x') ? parseInt(lower.slice(2), 16) : lower.startsWith('#') ? parseInt(lower.slice(1), 10) : NaN;
	return Number.isInteger(point) && point > 0 && point <= 0x10ffff ? String.fromCodePoint(point) : match;
}

/** Cut at the last sentence end that fits, else at a word with "...". */
function excerpt(text: string): string {
	const head = text.slice(0, BIO_MAX);
	let end = -1;
	for (const match of head.matchAll(/[.!?](?=\s|$)/g)) {
		if (match.index >= BIO_MIN) end = match.index;
	}
	if (end >= 0) return head.slice(0, end + 1);
	const space = head.lastIndexOf(' ');
	return `${(space > BIO_MIN ? head.slice(0, space) : head).replace(/[\s,;:]+$/, '')}...`;
}

/** TIDAL bios arrive with [wimpLink ...]Name[/wimpLink] markup, stray HTML
 *  and entities, and are sometimes broken outright. Returns a short plain
 *  excerpt, or null when nothing readable is left. */
export function cleanBio(raw: string | null | undefined): string | null {
	if (!raw) return null;
	let text = raw;
	for (let previous = ''; previous !== text; ) {
		previous = text;
		text = text.replace(/\[(\w+)[^\]]*\]([\s\S]*?)\[\/\1\]/g, '$2');
	}
	text = text.replace(/<\s*\/?\s*(br|p)\b[^>]*>/gi, ' ').replace(/<\/?[a-z][^>]*>/gi, '');
	if (/[[\]<>]/.test(text)) return null;
	text = text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, decodeEntity).replace(/\s+/g, ' ').trim();
	if (text.length > BIO_MAX) text = excerpt(text);
	return text.length >= BIO_MIN ? text : null;
}
