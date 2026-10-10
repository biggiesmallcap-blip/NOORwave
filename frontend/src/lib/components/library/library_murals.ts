import type { Album, SuggestedAlbum, Track } from '$lib/api/client';

// Pure model for the Library landing's suggestion murals. Data loading and
// rendering live in LibraryMurals.svelte; everything here is deterministic so
// it can be unit-tested without a DOM or the API.

export const HOME_MURAL_ITEM_LIMIT = 12;

// Max tracks one artist may contribute to a suggestion panel, so a single
// prolific neighbour can't clone-fill it. Mirrors the server cap in
// noor-server/src/server/routes/home_suggestions.rs.
export const SUGGESTION_ARTIST_CAP = 2;

export interface HomeAlbumCard {
	id: number;
	title: string;
	artist_id: number | null;
	artist_name: string | null;
	artwork_url: string | null;
}

export type HomeMuralItemKind = 'track' | 'album';

export interface HomeMuralItem {
	id: number;
	kind: HomeMuralItemKind;
	title: string;
	subtitle: string;
	artwork_url: string | null;
	track?: Track;
	album?: HomeAlbumCard;
}

export type HomeMuralPanelId = 'suggested-tracks' | 'suggested-albums' | 'random-tracks' | 'random-albums';

export interface HomeMuralPanel {
	id: HomeMuralPanelId;
	label: string;
	caption: string;
	kind: HomeMuralItemKind;
	items: HomeMuralItem[];
}

export interface HomeMuralSources {
	suggestionTracks: Track[];
	suggestionAlbums: HomeAlbumCard[];
	randomTracks: Track[];
	randomAlbums: HomeAlbumCard[];
}

function suggestionArtistKey(track: Track): number | string {
	return track.artist_id ?? track.artist_name ?? '';
}

// Greedy per-artist cap, then a top-up pass from what the cap skipped. An empty
// key (missing artist) is never capped so those tracks don't all collapse into
// one synthetic bucket. The top-up matters because the cap shapes the head of
// the mural, not its length: dropping capped tracks outright left the panel
// showing 5 of 12 whenever the server's list leaned on a few artists.
export function capPerArtist(tracks: Track[], max: number, limit: number): Track[] {
	const perArtist = new Map<number | string, number>();
	const out: Track[] = [];
	const skipped: Track[] = [];
	for (const track of tracks) {
		if (out.length >= limit) break;
		const key = suggestionArtistKey(track);
		if (max > 0 && key !== '') {
			const count = perArtist.get(key) ?? 0;
			if (count >= max) {
				skipped.push(track);
				continue;
			}
			perArtist.set(key, count + 1);
		}
		out.push(track);
	}
	for (const track of skipped) {
		if (out.length >= limit) break;
		out.push(track);
	}
	return out;
}

export function uniqueById<T extends { id: number }>(items: T[]): T[] {
	const seen = new Set<number>();
	const result: T[] = [];
	for (const item of items) {
		if (seen.has(item.id)) continue;
		seen.add(item.id);
		result.push(item);
	}
	return result;
}

export function toAlbumCard(album: Album | SuggestedAlbum): HomeAlbumCard {
	return {
		id: album.id,
		title: album.title,
		artist_id: album.artist_id ?? null,
		artist_name: album.artist_name ?? null,
		artwork_url: album.artwork_url ?? null,
	};
}

export function trackToMuralItem(track: Track): HomeMuralItem {
	return {
		id: track.id,
		kind: 'track',
		title: track.title,
		subtitle: track.artist_name ?? track.album_title ?? 'Unknown artist',
		artwork_url: track.artwork_url,
		track,
	};
}

export function albumToMuralItem(album: HomeAlbumCard): HomeMuralItem {
	return {
		id: album.id,
		kind: 'album',
		title: album.title,
		subtitle: album.artist_name ?? 'Unknown artist',
		artwork_url: album.artwork_url,
		album,
	};
}

/** The four panels in display order, with empty ones dropped. */
export function buildMuralPanels(sources: HomeMuralSources): HomeMuralPanel[] {
	const panels: HomeMuralPanel[] = [
		{
			id: 'suggested-tracks',
			label: 'Suggested tracks',
			caption: 'from your listening history',
			kind: 'track',
			items: capPerArtist(sources.suggestionTracks, SUGGESTION_ARTIST_CAP, HOME_MURAL_ITEM_LIMIT).map(trackToMuralItem),
		},
		{
			id: 'suggested-albums',
			label: 'Suggested albums',
			caption: 'from your listening history',
			kind: 'album',
			// Rendered as the server ranked them. The old same-artist expansion
			// tail-filled this with the album that was just played.
			items: sources.suggestionAlbums.slice(0, HOME_MURAL_ITEM_LIMIT).map(albumToMuralItem),
		},
		{
			id: 'random-tracks',
			label: 'Random tracks',
			caption: 'shuffled from your library',
			kind: 'track',
			items: sources.randomTracks.slice(0, HOME_MURAL_ITEM_LIMIT).map(trackToMuralItem),
		},
		{
			id: 'random-albums',
			label: 'Random albums',
			caption: 'shuffled from your library',
			kind: 'album',
			items: sources.randomAlbums.slice(0, HOME_MURAL_ITEM_LIMIT).map(albumToMuralItem),
		},
	];
	return panels.filter((panel) => panel.items.length > 0);
}

/**
 * Queue order for playing a track from a panel: the panel's tracks in order,
 * deduplicated, with the clicked track guaranteed to be present.
 */
export function panelQueueTrackIds(panel: HomeMuralPanel, clickedTrackId: number): number[] {
	const seen = new Set<number>();
	const ids: number[] = [];
	for (const item of panel.items) {
		if (item.kind !== 'track' || !item.track) continue;
		if (seen.has(item.track.id)) continue;
		seen.add(item.track.id);
		ids.push(item.track.id);
	}
	if (!seen.has(clickedTrackId)) ids.unshift(clickedTrackId);
	return ids;
}

// Domain-prefixed so a track and an album with the same numeric id never
// collide in the lazy-art map.
export function muralItemKey(item: HomeMuralItem): string {
	return `${item.kind}-${item.id}`;
}

// Search terms for the lazy TIDAL-art lookup, shared by the tile's lazy action
// and the synchronous cache peek so both hit the same key.
export function muralItemLazyQuery(item: HomeMuralItem): { artist: string | null; title: string } {
	if (item.kind === 'album') {
		return { artist: item.album?.artist_name ?? null, title: item.album?.title ?? item.title };
	}
	return { artist: item.track?.artist_name ?? null, title: item.title };
}

export function fallbackLetters(label: string): string {
	return label.split(/\s+/).map((part) => part[0]?.toUpperCase() ?? '').join('').slice(0, 2) || '?';
}
