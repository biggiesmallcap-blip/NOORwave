import type {
	ProviderRecommendationItem,
	TidalSearchAlbum,
	TidalSearchArtist,
	TidalSearchResults,
} from '$lib/api/client';

export function recommendationEntity(item: ProviderRecommendationItem): string {
	return item.entity_type ?? 'track';
}

export function recommendationKnownHref(item: ProviderRecommendationItem): string | null {
	const entity = recommendationEntity(item);
	if (entity === 'artist') {
		if (item.local_artist_id) return `/artists/${item.local_artist_id}`;
		if (item.tidal_artist_id) return `/tidal/artists/${item.tidal_artist_id}`;
		return null;
	}
	if (entity === 'album') {
		if (item.local_album_id) return `/albums/${item.local_album_id}`;
		if (item.tidal_album_id) return `/tidal/albums/${item.tidal_album_id}`;
		return null;
	}
	return null;
}

export function recommendationSearchQuery(item: ProviderRecommendationItem): string {
	if (recommendationEntity(item) === 'album') {
		return [item.artist_name, item.title].filter(Boolean).join(' ');
	}
	return item.title;
}

export function recommendationSearchHref(item: ProviderRecommendationItem): string {
	return `/search?q=${encodeURIComponent(recommendationSearchQuery(item))}`;
}

export function recommendationActionLabel(item: ProviderRecommendationItem): string {
	const entity = recommendationEntity(item);
	if (entity === 'artist') return recommendationKnownHref(item) ? 'Open artist' : 'Resolve artist';
	if (entity === 'album') return recommendationKnownHref(item) ? 'Open album' : 'Resolve album';
	if (item.local_track_id) return 'Play';
	if ((item.tidal_id ?? 0) > 0) return 'Play from TIDAL';
	return 'Resolve on TIDAL';
}

export function recommendationHrefFromSearch(
	item: ProviderRecommendationItem,
	results: TidalSearchResults,
): string | null {
	const entity = recommendationEntity(item);
	if (entity === 'artist') {
		const match = findArtistMatch(item, results.artists);
		if (!match) return null;
		return match.local_id ? `/artists/${match.local_id}` : `/tidal/artists/${match.tidal_id}`;
	}
	if (entity === 'album') {
		const match = findAlbumMatch(item, results.albums);
		if (!match) return null;
		return match.local_id ? `/albums/${match.local_id}` : `/tidal/albums/${match.tidal_id}`;
	}
	return null;
}

export function findArtistMatch(
	item: ProviderRecommendationItem,
	artists: TidalSearchArtist[],
): TidalSearchArtist | null {
	const wanted = normalizeCatalogName(item.title);
	return artists.find((artist) => normalizeCatalogName(artist.name) === wanted) ?? null;
}

export function findAlbumMatch(
	item: ProviderRecommendationItem,
	albums: TidalSearchAlbum[],
): TidalSearchAlbum | null {
	const wantedTitle = normalizeCatalogName(item.title);
	const wantedArtist = normalizeCatalogName(item.artist_name);
	return albums.find((album) => {
		if (normalizeCatalogName(album.title) !== wantedTitle) return false;
		if (!wantedArtist) return true;
		return normalizeCatalogName(album.artist_name) === wantedArtist;
	}) ?? null;
}

function normalizeCatalogName(value: string | null | undefined): string {
	return (value ?? '')
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.toLowerCase()
		.replace(/&/g, 'and')
		.replace(/[^a-z0-9]+/g, ' ')
		.trim()
		.replace(/\s+/g, ' ');
}
