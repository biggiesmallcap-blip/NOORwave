import { writable } from 'svelte/store';

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

function load(key: string, fallback: string): string {
	if (typeof localStorage === 'undefined') return fallback;
	try {
		const v = localStorage.getItem(key);
		return v && v.trim() ? v : fallback;
	} catch {
		return fallback;
	}
}

function persist(key: string, value: string) {
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.setItem(key, value);
	} catch {
		// quota or denied — ignore
	}
}

function loadInitialMode(): TrendingMode {
	if (typeof localStorage === 'undefined') return 'worldwide';
	try {
		const stored = localStorage.getItem(MODE_KEY);
		if (stored && (VALID_MODES as string[]).includes(stored)) return stored as TrendingMode;
		// One-time migration from the pre-merge `noor.trending.source` key so
		// users who previously picked Tidal don't get reset to Worldwide.
		const legacy = localStorage.getItem(LEGACY_SOURCE_KEY);
		if (legacy === 'tidal') return 'tidal';
	} catch {
		// fall through
	}
	return 'worldwide';
}

export const selectedTrendingMode = writable<TrendingMode>(loadInitialMode());
export const selectedCountry = writable<string>(load(COUNTRY_KEY, 'AU'));
export const selectedGenre = writable<string>(load(GENRE_KEY, 'electronic'));

selectedTrendingMode.subscribe((v) => persist(MODE_KEY, v));
selectedCountry.subscribe((v) => persist(COUNTRY_KEY, v));
selectedGenre.subscribe((v) => persist(GENRE_KEY, v));
