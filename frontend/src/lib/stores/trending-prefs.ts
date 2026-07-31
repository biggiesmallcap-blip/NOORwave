import { createPersistedStore, oneOf, readPersisted } from './persisted';

/// Trending shelf scope. One unified shelf with these four modes:
/// - `worldwide`: Last.fm `chart.gettoptracks`
/// - `country`:   Last.fm `geo.gettoptracks` for `$selectedCountry`
/// - `genre`:     Last.fm `tag.gettoptracks` for `$selectedGenre`
/// - `tidal`:     Tidal editorial chart
export type TrendingMode = 'worldwide' | 'country' | 'genre' | 'tidal';
const VALID_MODES: TrendingMode[] = ['worldwide', 'country', 'genre', 'tidal'];

const MODE_KEY = 'noor.trending.mode';
const COUNTRY_KEY = 'noor.trending.country';
const GENRE_KEY = 'noor.trending.genre';
const LEGACY_SOURCE_KEY = 'noor.trending.source'; // pre-merge: 'lastfm' | 'tidal'

/** Reject a stored empty string so a blank key falls back to the default. */
const nonEmpty = (raw: string): string | undefined => (raw.trim() ? raw : undefined);

// One-time migration from the pre-merge `noor.trending.source` key so users who
// previously picked Tidal don't get reset to Worldwide. This is the *fallback*
// the store uses when MODE_KEY itself holds nothing valid, so the current key
// still wins whenever it is set.
const legacyMode: TrendingMode =
	readPersisted<string>(LEGACY_SOURCE_KEY, '') === 'tidal' ? 'tidal' : 'worldwide';

export const selectedTrendingMode = createPersistedStore<TrendingMode>(MODE_KEY, legacyMode, {
	parse: oneOf(VALID_MODES),
});
export const selectedCountry = createPersistedStore(COUNTRY_KEY, 'AU', { parse: nonEmpty });
export const selectedGenre = createPersistedStore(GENRE_KEY, 'electronic', { parse: nonEmpty });
