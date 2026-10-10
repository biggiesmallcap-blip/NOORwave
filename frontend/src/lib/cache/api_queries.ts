import {
	dataCache,
	type CachedQuery,
	type CacheKeyInput,
	type QueryOptions,
} from '$lib/cache/query';
import {
	api,
	getApiBase,
	getStoredToken,
	type Album,
	type AnalyticsSignals,
	type Artist,
	type AudioSettings,
	type DiscoveryEngine,
	type DiscoveryStatus,
	type Genre,
	type GenreAudioMetrics,
	type GenreCohort,
	type GenreEvolutionPoint,
	type GenreHeat,
	type HomeArticlesResponse,
	type HomeNewsResponse,
	type HomeRecommendationsResponse,
	type HomeShufflePicksResponse,
	type HomeSuggestionsResponse,
	type LastfmStatus,
	type ListenBrainzStatus,
	type MusicBrainzStatus,
	type PlaybackSnapshot,
	type Playlist,
	type PortableMusicBrainzSnapshotStatus,
	type SearchResults,
	type TidalHomeModulesResponse,
	type TidalMixesResponse,
	type TidalMoodsResponse,
	type TidalRadioStationsResponse,
	type TidalArtistProfile,
	type Track,
	type AudioDevice,
	type ChartMatrixResponse,
	type ChartSnapshotResponse,
	type LastfmCountry,
	type LastfmGenre,
} from '$lib/api/client';

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function hashScope(value: string): string {
	let hash = 2166136261;
	for (let index = 0; index < value.length; index += 1) {
		hash ^= value.charCodeAt(index);
		hash = Math.imul(hash, 16777619);
	}
	return (hash >>> 0).toString(36);
}

function cacheNamespace(): string {
	const apiBase = getApiBase();
	const token = getStoredToken() ?? 'no-token';
	return `${hashScope(apiBase)}.${hashScope(token)}`;
}

let activeCacheNamespace: string | null = null;

export function ensureCacheScope(): void {
	const next = cacheNamespace();
	if (activeCacheNamespace === next) return;
	const changed = activeCacheNamespace !== null;
	activeCacheNamespace = next;
	if (changed) dataCache.clear();
	// Drop persisted entries from any other namespace so a token/api-base change
	// can't orphan them in localStorage forever. Only sweep once a real token is
	// present, so a transient pre-auth 'no-token' scope can't nuke the entries
	// written under the real token.
	if (getStoredToken()) dataCache.sweepForeignPersisted(next);
}

function scopedPersist(maxAgeMs: number): NonNullable<QueryOptions['persist']> {
	return { maxAgeMs, namespace: cacheNamespace };
}

const volatileOptions: QueryOptions = { staleMs: 5 * SECOND };
const shortOptions: QueryOptions = { staleMs: 30 * SECOND };
const mediumOptions: QueryOptions = { staleMs: 5 * MINUTE, persist: scopedPersist(DAY) };
export function artistPreviewStaleMs(data: unknown): number {
	const profile = (data ?? {}) as Partial<TidalArtistProfile>;
	if (!profile.available) return 5 * SECOND;
	if (profile.sections_failed?.length || Object.values(profile.release_filter_status ?? {}).some((status) => status.failed)) {
		return 30 * SECOND;
	}
	return 5 * MINUTE;
}
const artistPreviewOptions: QueryOptions = {
	...mediumOptions,
	staleMsForData: artistPreviewStaleMs,
};
const longOptions: QueryOptions = { staleMs: 30 * MINUTE, persist: scopedPersist(7 * DAY) };
const moodsOptions: QueryOptions = { ...longOptions, returnStale: true };
// Read-mostly surfaces that should paint last-known content instantly on open and
// revalidate in the background. NOT for lists that are mutated then re-read (library,
// playlists, artist/album detail) - returnStale there would flash pre-mutation rows.
const staticOptions: QueryOptions = { ...longOptions, returnStale: true };

// Chart payloads: paint the last-known chart instantly on open, revalidate in
// the background. An empty list is usually a transient upstream miss, so it
// goes stale fast instead of pinning an empty shelf for the full window.
function chartStaleMs(data: unknown): number {
	const payload = (data ?? {}) as { tracks?: unknown[] | null; entries?: unknown[] | null; rows?: unknown[] | null };
	const rows = payload.tracks ?? payload.entries ?? payload.rows ?? [];
	return rows.length > 0 ? 15 * MINUTE : 30 * SECOND;
}
// Library mural payloads: an empty sample is usually the server still warming
// up at boot, so it must not pin empty murals for the whole window.
function homeMuralStaleMs(fullMs: number): (data: unknown) => number {
	return (data) => {
		const payload = (data ?? {}) as { tracks?: unknown[] | null; albums?: unknown[] | null };
		return (payload.tracks?.length ?? 0) + (payload.albums?.length ?? 0) > 0 ? fullMs : 15 * SECOND;
	};
}
const chartOptions: QueryOptions = {
	staleMs: 15 * MINUTE,
	staleMsForData: chartStaleMs,
	persist: scopedPersist(DAY),
	returnStale: true,
};
// The curated country and genre lists are server constants.
const chartCatalogOptions: QueryOptions = { staleMs: DAY, persist: scopedPersist(7 * DAY), returnStale: true };

export type TrendingQuery = { source: 'lastfm' | 'tidal'; limit: number; country?: string; tag?: string };
export type ChartSnapshotQuery = { source: string; period: string; region: string; limit: number };
export type TrendingResponse = Awaited<ReturnType<typeof api.getTrending>>;

export const cacheKeys = {
	playbackState: () => ['api', 'getPlaybackState'] as const,
	playbackRuntime: () => ['api', 'getPlaybackRuntime'] as const,
	tracks: (
		sortBy = 'date_added',
		sortDir = 'desc',
		limit = 50,
		offset = 0,
		favoriteOnly = true,
		likedOnly = false,
	) => ['api', 'getTracks', { favoriteOnly, likedOnly, limit, offset, sortBy, sortDir }] as const,
	history: (limit = 50, offset = 0) => ['api', 'getHistory', { limit, offset }] as const,
	albums: (
		sortBy = 'title',
		sortDir = 'asc',
		limit = 50,
		offset = 0,
		favoriteOnly = true,
		decade: number | null = null,
	) => ['api', 'getAlbums', { decade, favoriteOnly, limit, offset, sortBy, sortDir }] as const,
	albumDecades: (favoriteOnly = true) => ['api', 'getAlbumDecades', { favoriteOnly }] as const,
	artists: (sortBy = 'name', sortDir = 'asc', limit = 50, offset = 0) =>
		['api', 'getArtists', { limit, offset, sortBy, sortDir }] as const,
	artist: (id: number) => ['api', 'getArtist', { id }] as const,
	artistTracks: (id: number) => ['api', 'getArtistTracks', { id }] as const,
	artistDiscography: (id: number) => ['api', 'getArtistDiscography', { id }] as const,
	artistDiscographyPreview: (id: number) => ['api', 'getArtistDiscographyPreview', { id }] as const,
	tidalArtistCore: (id: number) => ['api', 'getTidalArtistCore', { id }] as const,
	tidalArtistProfile: (id: number) => ['api', 'getTidalArtistProfile', { id }] as const,
	tidalArtistPreview: (id: number) => ['api', 'getTidalArtistPreview', { id }] as const,
	artistSpotifyStats: (id: number) => ['api', 'getArtistSpotifyStats', { id }] as const,
	albumTracks: (id: number) => ['api', 'getAlbumTracks', { id }] as const,
	albumSpotifyStats: (id: number) => ['api', 'getAlbumSpotifyStats', { id }] as const,
	genres: () => ['api', 'getGenres'] as const,
	genreGalaxySnapshot: (days = 90) => ['api', 'getGenreGalaxySnapshot', { days }] as const,
	analyticsSignals: (days = 30) => ['api', 'getAnalyticsSignals', { days }] as const,
	genreHeat: (days = 90) => ['api', 'getGenreHeat', { days }] as const,
	genreCohorts: (days = 90) => ['api', 'getGenreCohorts', { days }] as const,
	genreEvolution: (days = 90) => ['api', 'getGenreEvolution', { days }] as const,
	genreAudioMetrics: () => ['api', 'getGenreAudioMetrics'] as const,
	genreTracks: (id: number, includeDescendants = true) =>
		['api', 'getGenreTracks', { id, includeDescendants }] as const,
	playlists: () => ['api', 'getPlaylists'] as const,
	playlistTracks: (id: number) => ['api', 'getPlaylistTracks', { id }] as const,
	evaluateSmartPlaylist: (id: number) => ['api', 'evaluateSmartPlaylist', { id }] as const,
	playlistCoverSample: (id: number) => ['api', 'getPlaylistCoverSample', { id }] as const,
	recentListens: (limit = 20) => ['api', 'getRecentListens', { limit }] as const,
	search: (query: string, limit = 20) => ['api', 'search', { limit, query }] as const,
	homeArticles: () => ['api', 'getHomeArticles'] as const,
	homeNews: () => ['api', 'getHomeNews'] as const,
	homeRecommendations: () => ['api', 'getHomeRecommendations'] as const,
	homeSuggestions: (seedKey: string) => ['api', 'getHomeSuggestions', { seedKey }] as const,
	homeShufflePicks: (limit: number) => ['api', 'getHomeShufflePicks', { limit }] as const,
	tidalMixes: () => ['api', 'getTidalMixes'] as const,
	tidalRadioStations: () => ['api', 'getTidalRadioStations'] as const,
	tidalHomeModules: () => ['api', 'getTidalHomeModules'] as const,
	tidalPage: (path: string) => ['api', 'getTidalPage', { path }] as const,
	tidalMoods: () => ['api', 'getTidalMoods'] as const,
	// Under one 'charts' prefix so the TIDAL-content invalidation (which matches
	// /charts/) and invalidateChartCaches both reach every chart payload.
	charts: {
		trending: (opts: TrendingQuery) => ['api', 'charts', 'trending', opts] as const,
		lastfmCountries: () => ['api', 'charts', 'lastfmCountries'] as const,
		lastfmGenres: () => ['api', 'charts', 'lastfmGenres'] as const,
		matrix: () => ['api', 'charts', 'matrix'] as const,
		snapshot: (opts: ChartSnapshotQuery) => ['api', 'charts', 'snapshot', opts] as const,
	},
	settings: {
		musicBrainzStatus: () => ['api', 'getMusicBrainzStatus'] as const,
		portableMusicBrainzSnapshot: () => ['api', 'getPortableMusicBrainzSnapshot'] as const,
		discoveryStatus: () => ['api', 'getDiscoveryStatus'] as const,
		discoveryEngine: () => ['api', 'getDiscoveryEngine'] as const,
		discoveryIntensity: () => ['api', 'getDiscoveryIntensity'] as const,
		discoverySafety: () => ['api', 'getDiscoverySafety'] as const,
		discoverySafetyProfile: () => ['api', 'getDiscoverySafetyProfile'] as const,
		radioSimilarityStatus: () => ['api', 'getRadioSimilarityStatus'] as const,
		audioSettings: () => ['api', 'getAudioSettings'] as const,
		audioDevices: () => ['api', 'listAudioDevices'] as const,
		lastfmStatus: () => ['api', 'getLastfmStatus'] as const,
		listenBrainzStatus: () => ['api', 'getListenBrainzStatus'] as const,
		audioFeaturesStats: () => ['api', 'getAudioFeaturesStats'] as const,
		audioAnalysisStatus: () => ['api', 'getAudioAnalysisStatus'] as const,
		passiveDsp: () => ['api', 'getPassiveDsp'] as const,
	},
};

function query<T>(
	key: CacheKeyInput,
	fetcher: () => Promise<T>,
	options: QueryOptions,
): CachedQuery<T> {
	ensureCacheScope();
	return dataCache.query(key, fetcher, options);
}

function fetchCached<T>(
	key: CacheKeyInput,
	fetcher: () => Promise<T>,
	options: QueryOptions,
): Promise<T> {
	ensureCacheScope();
	return dataCache.fetchQuery(key, fetcher, options);
}

export const cachedApi = {
	getPlaybackState() {
		return fetchCached<PlaybackSnapshot>(cacheKeys.playbackState(), () => api.getPlaybackState(), volatileOptions);
	},
	getPlaybackRuntime() {
		return fetchCached<Awaited<ReturnType<typeof api.getPlaybackRuntime>>>(
			cacheKeys.playbackRuntime(),
			() => api.getPlaybackRuntime(),
			shortOptions,
		);
	},
	getTracks(
		sortBy = 'date_added',
		sortDir = 'desc',
		limit = 50,
		offset = 0,
		favoriteOnly = true,
		likedOnly = false,
	) {
		const options = offset === 0 ? mediumOptions : shortOptions;
		return fetchCached<{ tracks: Track[]; total: number }>(
			cacheKeys.tracks(sortBy, sortDir, limit, offset, favoriteOnly, likedOnly),
			() => api.getTracks(sortBy, sortDir, limit, offset, favoriteOnly, likedOnly),
			options,
		);
	},
	getHistory(limit = 50, offset = 0) {
		const options = offset === 0 ? volatileOptions : shortOptions;
		return fetchCached<{ tracks: Track[]; total: number }>(
			cacheKeys.history(limit, offset),
			() => api.getHistory(limit, offset),
			options,
		);
	},
	getAlbums(
		sortBy = 'title',
		sortDir = 'asc',
		limit = 50,
		offset = 0,
		favoriteOnly = true,
		decade: number | null = null,
	) {
		const options = offset === 0 ? mediumOptions : shortOptions;
		return fetchCached<{ albums: Album[]; total: number }>(
			cacheKeys.albums(sortBy, sortDir, limit, offset, favoriteOnly, decade),
			() => api.getAlbums(sortBy, sortDir, limit, offset, favoriteOnly, decade),
			options,
		);
	},
	getAlbumDecades(favoriteOnly = true) {
		return fetchCached<{ decades: number[] }>(
			cacheKeys.albumDecades(favoriteOnly),
			() => api.getAlbumDecades(favoriteOnly),
			longOptions,
		);
	},
	getArtists(sortBy = 'name', sortDir = 'asc', limit = 50, offset = 0) {
		return fetchCached<{ artists: Artist[] }>(
			cacheKeys.artists(sortBy, sortDir, limit, offset),
			() => api.getArtists(sortBy, sortDir, limit, offset),
			offset === 0 ? mediumOptions : shortOptions,
		);
	},
	getArtist(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getArtist>>>(
			cacheKeys.artist(id),
			() => api.getArtist(id),
			longOptions,
		);
	},
	getArtistTracks(id: number) {
		return fetchCached<{ tracks: Track[] }>(
			cacheKeys.artistTracks(id),
			() => api.getArtistTracks(id),
			longOptions,
		);
	},
	getArtistDiscography(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getArtistDiscography>>>(
			cacheKeys.artistDiscography(id),
			() => api.getArtistDiscography(id),
			longOptions,
		);
	},
	getArtistDiscographyPreview(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getArtistDiscography>>>(
			cacheKeys.artistDiscographyPreview(id),
			() => api.getArtistDiscography(id, true),
			artistPreviewOptions,
		);
	},
	// TIDAL artist profile (non-library artists). Cached like the library
	// discography so re-visits render instantly and concurrent loads of the
	// same artist share one request; medium staleness because in_library /
	// local_id flags live inside the payload and imports should surface
	// within minutes even without a library_synced invalidation.
	getTidalArtistProfile(tidalArtistId: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getTidalArtistProfile>>>(
			cacheKeys.tidalArtistProfile(tidalArtistId),
			() => api.getTidalArtistProfile(tidalArtistId),
			mediumOptions,
		);
	},
	getTidalArtistPreview(tidalArtistId: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getTidalArtistProfile>>>(
			cacheKeys.tidalArtistPreview(tidalArtistId),
			() => api.getTidalArtistProfile(tidalArtistId, true),
			artistPreviewOptions,
		);
	},
	getTidalArtistCore(tidalArtistId: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getTidalArtistCore>>>(
			cacheKeys.tidalArtistCore(tidalArtistId),
			() => api.getTidalArtistCore(tidalArtistId),
			{ ...mediumOptions, returnStale: true },
		);
	},
	getArtistSpotifyStats(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getArtistSpotifyStats>>>(
			cacheKeys.artistSpotifyStats(id),
			() => api.getArtistSpotifyStats(id),
			mediumOptions,
		);
	},
	getAlbumTracks(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getAlbumTracks>>>(
			cacheKeys.albumTracks(id),
			() => api.getAlbumTracks(id),
			longOptions,
		);
	},
	getAlbumSpotifyStats(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.getAlbumSpotifyStats>>>(
			cacheKeys.albumSpotifyStats(id),
			() => api.getAlbumSpotifyStats(id),
			mediumOptions,
		);
	},
	getGenres() {
		return fetchCached<{ genres: Genre[] }>(cacheKeys.genres(), () => api.getGenres(), longOptions);
	},
	getGenreGalaxySnapshot(days = 90) {
		return fetchCached<{
			genres: Genre[];
			heat: GenreHeat[];
			cohorts: GenreCohort[];
			evolution: GenreEvolutionPoint[];
			metrics: GenreAudioMetrics[];
		}>(
			cacheKeys.genreGalaxySnapshot(days),
			() => api.getGenreGalaxySnapshot(days),
			mediumOptions,
		);
	},
	// Reactive variant for instant-paint seeding (getSnapshot hydrates the persisted
	// copy). Kept on mediumOptions - the snapshot is large (~100KB), so we don't
	// broaden its persistence window.
	genreGalaxySnapshotQuery(days = 90) {
		return query<{
			genres: Genre[];
			heat: GenreHeat[];
			cohorts: GenreCohort[];
			evolution: GenreEvolutionPoint[];
			metrics: GenreAudioMetrics[];
		}>(cacheKeys.genreGalaxySnapshot(days), () => api.getGenreGalaxySnapshot(days), mediumOptions);
	},
	getAnalyticsSignals(days = 30) {
		return fetchCached<AnalyticsSignals>(
			cacheKeys.analyticsSignals(days),
			() => api.getAnalyticsSignals(days),
			staticOptions,
		);
	},
	analyticsSignalsQuery(days = 30) {
		return query<AnalyticsSignals>(
			cacheKeys.analyticsSignals(days),
			() => api.getAnalyticsSignals(days),
			staticOptions,
		);
	},
	getGenreHeat(days = 90) {
		return fetchCached<{ heat: GenreHeat[] }>(
			cacheKeys.genreHeat(days),
			() => api.getGenreHeat(days),
			shortOptions,
		);
	},
	getGenreCohorts(days = 90) {
		return fetchCached<{ cohorts: GenreCohort[] }>(
			cacheKeys.genreCohorts(days),
			() => api.getGenreCohorts(days),
			mediumOptions,
		);
	},
	getGenreEvolution(days = 90) {
		return fetchCached<{ evolution: GenreEvolutionPoint[] }>(
			cacheKeys.genreEvolution(days),
			() => api.getGenreEvolution(days),
			mediumOptions,
		);
	},
	getGenreAudioMetrics() {
		return fetchCached<{ metrics: GenreAudioMetrics[] }>(
			cacheKeys.genreAudioMetrics(),
			() => api.getGenreAudioMetrics(),
			mediumOptions,
		);
	},
	getGenreTracks(id: number, includeDescendants = true) {
		return fetchCached<{ tracks: Track[] }>(
			cacheKeys.genreTracks(id, includeDescendants),
			() => api.getGenreTracks(id, includeDescendants),
			mediumOptions,
		);
	},
	getPlaylists() {
		return fetchCached<{ playlists: Playlist[] }>(cacheKeys.playlists(), () => api.getPlaylists(), mediumOptions);
	},
	getPlaylistTracks(id: number) {
		return fetchCached<{ tracks: Track[] }>(
			cacheKeys.playlistTracks(id),
			() => api.getPlaylistTracks(id),
			mediumOptions,
		);
	},
	evaluateSmartPlaylist(id: number) {
		return fetchCached<Awaited<ReturnType<typeof api.evaluateSmartPlaylist>>>(
			cacheKeys.evaluateSmartPlaylist(id),
			() => api.evaluateSmartPlaylist(id),
			shortOptions,
		);
	},
	getPlaylistCoverSample(id: number, signal?: AbortSignal) {
		if (signal) return api.getPlaylistCoverSample(id, signal);
		return fetchCached<{ urls: string[] }>(
			cacheKeys.playlistCoverSample(id),
			() => api.getPlaylistCoverSample(id),
			mediumOptions,
		);
	},
	getRecentListens(limit = 20) {
		return fetchCached<Awaited<ReturnType<typeof api.getRecentListens>>>(
			cacheKeys.recentListens(limit),
			() => api.getRecentListens(limit),
			mediumOptions,
		);
	},
	search(query: string, limit = 20, signal?: AbortSignal) {
		if (signal) return api.search(query, limit, signal);
		return fetchCached<SearchResults>(
			cacheKeys.search(query, limit),
			() => api.search(query, limit),
			mediumOptions,
		);
	},
	getHomeArticles() {
		return fetchCached<HomeArticlesResponse>(cacheKeys.homeArticles(), () => api.getHomeArticles(), mediumOptions);
	},
	getHomeNews() {
		return fetchCached<HomeNewsResponse>(cacheKeys.homeNews(), () => api.getHomeNews(), mediumOptions);
	},
	getHomeRecommendations() {
		return fetchCached<HomeRecommendationsResponse>(
			cacheKeys.homeRecommendations(),
			() => api.getHomeRecommendations(),
			staticOptions,
		);
	},
	// Reactive queries for the Library murals. Mounted components subscribe, so
	// the stale-first paint is replaced when the background refresh lands; the
	// old one-shot fetchQuery(returnStale) handed back the stale payload and
	// dropped the fresh one, leaving a boot-time empty sample on screen until a
	// hard reload. Empty payloads go stale fast for the same reason.
	// In-memory only (no persist): suggestion payloads carry full Track rows, so
	// persisting every rotation would bloat the localStorage query cache (the
	// boot-crash quota risk).
	homeSuggestionsQuery(limit?: number) {
		return query<HomeSuggestionsResponse>(
			cacheKeys.homeSuggestions(''),
			() => api.getHomeSuggestions([], limit),
			{ staleMs: 30 * MINUTE, staleMsForData: homeMuralStaleMs(30 * MINUTE) },
		);
	},
	// The server holds each sample for five minutes, so refetching inside that
	// window repaints the same picks rather than reshuffling.
	homeShufflePicksQuery(limit = 12) {
		return query<HomeShufflePicksResponse>(
			cacheKeys.homeShufflePicks(limit),
			() => api.getHomeShufflePicks(limit),
			{ staleMs: 5 * MINUTE, staleMsForData: homeMuralStaleMs(5 * MINUTE) },
		);
	},
	getTidalMixes() {
		return fetchCached<TidalMixesResponse>(cacheKeys.tidalMixes(), () => api.getTidalMixes(), staticOptions);
	},
	getTidalRadioStations() {
		return fetchCached<TidalRadioStationsResponse>(
			cacheKeys.tidalRadioStations(),
			() => api.getTidalRadioStations(),
			staticOptions,
		);
	},
	getTidalHomeModules() {
		return fetchCached<TidalHomeModulesResponse>(
			cacheKeys.tidalHomeModules(),
			() => api.getTidalHomeModules(),
			longOptions,
		);
	},
	getTidalPage(path: string) {
		return fetchCached<TidalHomeModulesResponse>(
			cacheKeys.tidalPage(path),
			() => api.getTidalPage(path),
			longOptions,
		);
	},
	getTidalMoods() {
		return fetchCached<TidalMoodsResponse>(
			cacheKeys.tidalMoods(),
			() => api.getTidalMoods(),
			moodsOptions,
		);
	},
	getMusicBrainzStatus() {
		return fetchCached<MusicBrainzStatus>(
			cacheKeys.settings.musicBrainzStatus(),
			() => api.getMusicBrainzStatus(),
			shortOptions,
		);
	},
	getPortableMusicBrainzSnapshot() {
		return fetchCached<PortableMusicBrainzSnapshotStatus>(
			cacheKeys.settings.portableMusicBrainzSnapshot(),
			() => api.getPortableMusicBrainzSnapshot(),
			mediumOptions,
		);
	},
	getDiscoveryStatus() {
		return fetchCached<{ status: DiscoveryStatus }>(
			cacheKeys.settings.discoveryStatus(),
			() => api.getDiscoveryStatus(),
			shortOptions,
		);
	},
	getDiscoveryEngine() {
		return fetchCached<{
			engine: DiscoveryEngine;
			label: string;
			family: string;
			trainable: boolean;
			available: DiscoveryEngine[];
		}>(cacheKeys.settings.discoveryEngine(), () => api.getDiscoveryEngine(), mediumOptions);
	},
	getDiscoveryIntensity() {
		return fetchCached<Awaited<ReturnType<typeof api.getDiscoveryIntensity>>>(
			cacheKeys.settings.discoveryIntensity(),
			() => api.getDiscoveryIntensity(),
			mediumOptions,
		);
	},
	getDiscoverySafety() {
		return fetchCached<Awaited<ReturnType<typeof api.getDiscoverySafety>>>(
			cacheKeys.settings.discoverySafety(),
			() => api.getDiscoverySafety(),
			mediumOptions,
		);
	},
	getDiscoverySafetyProfile() {
		return fetchCached<Awaited<ReturnType<typeof api.getDiscoverySafetyProfile>>>(
			cacheKeys.settings.discoverySafetyProfile(),
			() => api.getDiscoverySafetyProfile(),
			mediumOptions,
		);
	},
	getRadioSimilarityStatus() {
		return fetchCached<{ row_count: number; built_at: string | null }>(
			cacheKeys.settings.radioSimilarityStatus(),
			() => api.getRadioSimilarityStatus(),
			shortOptions,
		);
	},
	getAudioSettings() {
		return fetchCached<AudioSettings>(
			cacheKeys.settings.audioSettings(),
			() => api.getAudioSettings(),
			mediumOptions,
		);
	},
	listAudioDevices() {
		return fetchCached<{ devices: AudioDevice[] }>(
			cacheKeys.settings.audioDevices(),
			() => api.listAudioDevices(),
			mediumOptions,
		);
	},
	getAudioFeaturesStats() {
		return fetchCached<Awaited<ReturnType<typeof api.getAudioFeaturesStats>>>(
			cacheKeys.settings.audioFeaturesStats(),
			() => api.getAudioFeaturesStats(),
			mediumOptions,
		);
	},
	getAudioAnalysisStatus() {
		return fetchCached<Awaited<ReturnType<typeof api.getAudioAnalysisStatus>>>(
			cacheKeys.settings.audioAnalysisStatus(),
			() => api.getAudioAnalysisStatus(),
			shortOptions,
		);
	},
	getPassiveDsp() {
		return fetchCached<Awaited<ReturnType<typeof api.getPassiveDsp>>>(
			cacheKeys.settings.passiveDsp(),
			() => api.getPassiveDsp(),
			mediumOptions,
		);
	},
	getLastfmStatus() {
		return fetchCached<LastfmStatus>(cacheKeys.settings.lastfmStatus(), () => api.getLastfmStatus(), staticOptions);
	},
	getListenBrainzStatus() {
		return fetchCached<ListenBrainzStatus>(
			cacheKeys.settings.listenBrainzStatus(),
			() => api.getListenBrainzStatus(),
			staticOptions,
		);
	},
	homeArticlesQuery() {
		return query<HomeArticlesResponse>(cacheKeys.homeArticles(), () => api.getHomeArticles(), mediumOptions);
	},
	homeNewsQuery() {
		return query<HomeNewsResponse>(cacheKeys.homeNews(), () => api.getHomeNews(), mediumOptions);
	},
	// Reactive, persisted, instant-paint queries for the home shelves. Creating one
	// hydrates the persisted snapshot (getState/peek do NOT) and schedules a
	// background revalidate; subscribers paint last-known content with no skeleton.
	tidalMixesQuery() {
		return query<TidalMixesResponse>(cacheKeys.tidalMixes(), () => api.getTidalMixes(), staticOptions);
	},
	tidalRadioStationsQuery() {
		return query<TidalRadioStationsResponse>(
			cacheKeys.tidalRadioStations(),
			() => api.getTidalRadioStations(),
			staticOptions,
		);
	},
	homeRecommendationsQuery() {
		return query<HomeRecommendationsResponse>(
			cacheKeys.homeRecommendations(),
			() => api.getHomeRecommendations(),
			staticOptions,
		);
	},
	trendingQuery(opts: TrendingQuery) {
		return query<TrendingResponse>(cacheKeys.charts.trending(opts), () => api.getTrending(opts), chartOptions);
	},
	lastfmCountriesQuery() {
		return query<{ countries: LastfmCountry[]; default_country: string }>(
			cacheKeys.charts.lastfmCountries(),
			() => api.getLastfmCountries(),
			chartCatalogOptions,
		);
	},
	lastfmGenresQuery() {
		return query<{ genres: LastfmGenre[]; default_genre: string }>(
			cacheKeys.charts.lastfmGenres(),
			() => api.getLastfmGenres(),
			chartCatalogOptions,
		);
	},
	chartMatrixQuery() {
		return query<ChartMatrixResponse>(cacheKeys.charts.matrix(), () => api.getChartMatrix(), chartOptions);
	},
	chartSnapshotQuery(opts: ChartSnapshotQuery) {
		return query<ChartSnapshotResponse>(
			cacheKeys.charts.snapshot(opts),
			// The raw provider payloads are never rendered; dropping them keeps
			// each persisted region x provider snapshot small in localStorage.
			() => api.getChartSnapshot(opts).then((snapshot) => ({
				...snapshot,
				entries: snapshot.entries.map((entry) => ({ ...entry, raw_json: null, provider_positions_json: null })),
			})),
			chartOptions,
		);
	},
	lastfmStatusQuery() {
		return query<LastfmStatus>(cacheKeys.settings.lastfmStatus(), () => api.getLastfmStatus(), staticOptions);
	},
	listenBrainzStatusQuery() {
		return query<ListenBrainzStatus>(
			cacheKeys.settings.listenBrainzStatus(),
			() => api.getListenBrainzStatus(),
			staticOptions,
		);
	},
};

export function seedCachedValue<T>(key: CacheKeyInput, data: T, options: QueryOptions = mediumOptions): void {
	ensureCacheScope();
	dataCache.prime(key, data, options);
}

export function invalidateLibraryCaches(options: { refetch?: boolean } = {}): void {
	ensureCacheScope();
	dataCache.invalidatePrefix(['api', 'getTracks'], options);
	dataCache.invalidatePrefix(['api', 'getHistory'], options);
	dataCache.invalidatePrefix(['api', 'getAlbums'], options);
	dataCache.invalidatePrefix(['api', 'getArtists'], options);
	dataCache.invalidatePrefix(['api', 'getArtistTracks'], options);
	dataCache.invalidatePrefix(['api', 'getArtistDiscography'], options);
	dataCache.invalidatePrefix(['api', 'getArtistDiscographyPreview'], options);
	dataCache.invalidatePrefix(['api', 'getTidalArtistProfile'], options);
	dataCache.invalidatePrefix(['api', 'getTidalArtistPreview'], options);
	dataCache.invalidatePrefix(['api', 'getAlbumTracks'], options);
	dataCache.invalidatePrefix(['api', 'search'], options);
}

export function invalidateHomeCaches(options: { refetch?: boolean } = {}): void {
	ensureCacheScope();
	dataCache.invalidatePrefix(['api', 'getHomeArticles'], options);
	dataCache.invalidatePrefix(['api', 'getHomeNews'], options);
	dataCache.invalidatePrefix(['api', 'getHomeRecommendations'], options);
	dataCache.invalidatePrefix(['api', 'getTidalMixes'], options);
	dataCache.invalidatePrefix(['api', 'getTidalRadioStations'], options);
	dataCache.invalidatePrefix(['api', 'getTidalHomeModules'], options);
	dataCache.invalidatePrefix(['api', 'getTidalMoods'], options);
}

/**
 * After a provider-matrix refresh: refetch the matrix now and mark every
 * stored snapshot stale, so whichever region and provider is opened next
 * reads the new day instead of hydrating yesterday's list.
 */
export function invalidateChartSnapshotCaches(): void {
	ensureCacheScope();
	dataCache.invalidatePrefix(cacheKeys.charts.matrix(), { refetch: true });
	dataCache.invalidatePrefix(['api', 'charts', 'snapshot']);
}

export function patchDiscoveryProgress(progress: {
	stage?: string;
	progress?: number;
	tracks_done?: number;
	tracks_total?: number;
}): void {
	ensureCacheScope();
	dataCache.patch<{ status: DiscoveryStatus }>(cacheKeys.settings.discoveryStatus(), (current) => {
		const latest = current?.status.latest_run;
		if (!current || !latest) return current;
		return {
			status: {
				...current.status,
				latest_run: {
					...latest,
					progress: typeof progress.progress === 'number' ? progress.progress : latest.progress,
					stage: typeof progress.stage === 'string' ? progress.stage : latest.stage,
					items_done: typeof progress.tracks_done === 'number' ? progress.tracks_done : latest.items_done,
					items_total: typeof progress.tracks_total === 'number' ? progress.tracks_total : latest.items_total,
				},
			},
		};
	});
}
