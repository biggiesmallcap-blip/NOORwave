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
	// The primary action now plays the entity (album tracklist, artist top
	// tracks) rather than navigating, so the label is play-oriented even when the
	// id still has to be resolved through a TIDAL search first.
	if (entity === 'artist') return 'Play artist';
	if (entity === 'album') return 'Play album';
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
	if (artists.length === 0) return null;
	const wanted = normalizeCatalogName(item.title);
	const exact = artists.find((artist) => normalizeCatalogName(artist.name) === wanted);
	if (exact) return exact;
	// Tolerate the punctuation/suffix drift between Last.fm and TIDAL spellings
	// (e.g. "MF DOOM" vs "MF DOOM (Daniel Dumile)").
	const partial = artists.find((artist) => namesOverlap(normalizeCatalogName(artist.name), wanted));
	if (partial) return partial;
	// Previously this fell through to `artists[0]`, on the reasoning that the
	// search was keyed on the artist name so the top hit is probably right.
	// When it is wrong it is silently wrong: the user clicks one artist and
	// lands on another, with nothing to indicate a guess was made. A sole
	// result is a safe bet; anything else goes to the search page, where the
	// user picks. findAlbumMatch has always refused to guess for the same
	// reason - a wrong album is worse than no album.
	return artists.length === 1 ? artists[0] : null;
}

export function findAlbumMatch(
	item: ProviderRecommendationItem,
	albums: TidalSearchAlbum[],
): TidalSearchAlbum | null {
	if (albums.length === 0) return null;
	const wantedTitle = normalizeCatalogName(item.title);
	const wantedArtist = normalizeCatalogName(item.artist_name);
	const artistOk = (album: TidalSearchAlbum) =>
		!wantedArtist || normalizeCatalogName(album.artist_name) === wantedArtist;

	const exact = albums.find((album) => artistOk(album) && normalizeCatalogName(album.title) === wantedTitle);
	if (exact) return exact;

	// A wrong album is worse than no album, so only accept a fuzzy hit by the same
	// artist (handles deluxe/remaster/edition suffixes). If the artist is known and
	// only one of their albums came back, take it; otherwise require title overlap.
	const sameArtist = albums.filter(artistOk);
	if (sameArtist.length === 0) return null;
	const overlap = sameArtist.find((album) => namesOverlap(normalizeCatalogName(album.title), wantedTitle));
	if (overlap) return overlap;
	if (wantedArtist && sameArtist.length === 1) return sameArtist[0];
	return null;
}

/**
 * True when one normalised name extends the other at a word boundary - the
 * cheap stand-in for fuzzy matching that catches edition suffixes and
 * parenthetical qualifiers ("untrue" vs "untrue deluxe edition", "mf doom" vs
 * "mf doom daniel dumile").
 *
 * Compares whole tokens rather than raw substrings. Plain containment made
 * every short name match a longer one that merely spelled it: "nova" hit
 * "casanova" and "novastar", and since the caller took the first overlap it
 * found, that silently opened the wrong artist.
 */
function namesOverlap(a: string, b: string): boolean {
	if (!a || !b) return false;
	if (a === b) return true;
	const aTokens = a.split(' ');
	const bTokens = b.split(' ');
	const [shorter, longer] =
		aTokens.length <= bTokens.length ? [aTokens, bTokens] : [bTokens, aTokens];
	return shorter.every((token, index) => longer[index] === token);
}

/**
 * Fold a catalogue name to its comparable form.
 *
 * Ported to Rust as `db::catalog_name::normalize_catalog_name`. The two must
 * agree: this copy picks a search result at click time, the Rust one matches a
 * local row at resolve time, and a divergence means a name resolves on one side
 * but not the other. The shared case table lives in
 * `catalog_name_parity.test.ts` and in that module's `NORMALIZE_PARITY_CASES`.
 */
export function normalizeCatalogName(value: string | null | undefined): string {
	return (value ?? '')
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.toLowerCase()
		.replace(/&/g, 'and')
		.replace(/[^a-z0-9]+/g, ' ')
		.trim()
		.replace(/\s+/g, ' ');
}
