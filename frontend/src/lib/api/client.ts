import type {
	Track,
	SpotifyArtistStats,
	SpotifyTrackStats,
	TidalDiscographyAlbum,
	TidalDiscographyTrack,
	TidalArtistVideo,
	TidalSimilarArtist,
	TidalArtistBio,
	ArtworkCacheSettings,
	VideoDiscoverySetting,
	VideoDiscoveryStatus,
	VideoStationSettings,
	VideoStationsResponse,
	TidalSearchVideo,
	TidalSearchPlaylist,
	TidalSearchResults,
	TidalVideoStream,
	VideoHistoryEntry,
	TidalVideoMixItem,
	VideoDiscoverResponse,
	LikedVideosResponse,
	SpotifyPlaylistSearchItem,
	SpotifyLibrarySaveResponse,
	SpotifyTidalState,
	SpotifyPlaylistTrack,
	SpotifyPlaylistMeta,
	SpotifyPlaylistDetail,
	SpotifyTrackDetail,
	SpotifyAlbumDetail,
	SpotifyArtistDetail,
	SpotifyAlbumSearchItem,
	SpotifyArtistSearchItem,
	SpotifyArtistRelated,
	SpotifyAlbumRelated,
	SpotifyTrackRelated,
	ResolveStatusEntry,
	TidalArtistProfile,
	ArtistReleaseFilter,
	ArtistReleaseFilterStatuses,
	TidalArtistReleasePage,
	TidalArtistCore,
	TidalPlayable,
	QueueExternalRequest,
	ChartEntry,
	TrendingSource,
	LastfmChartKind,
	ChartSnapshotResponse,
	ChartMatrixResponse,
	ChartMatrixRefreshResponse,
	LastfmGenre,
	LastfmCountry,
	Album,
	Artist,
	Genre,
	Playlist,
	RuleClause,
	SearchResults,
	QueueItem,
	MixedQueueItem,
	PlaybackState,
	PlaybackSnapshot,
	QueueSnapshot,
	ShuffleDebug,
	PlaybackRuntimeInfo,
	DjEnabledResponse,
	DjProfileResponse,
	DjMixIntent,
	DjMixIntentResponse,
	DjPolicyResponse,
	DjStatusResponse,
	DjProfileCorrectionRequest,
	DjFeedbackRequest,
	StreamDisplayInfo,
	MusicBrainzStatus,
	PortableMusicBrainzSnapshotStatus,
	PortableMusicBrainzSnapshotAction,
	TrackFavoriteResponse,
	AnalyticsOverview,
	ListenHistoryEntry,
	GenreHeat,
	GenreCohort,
	GenreEvolutionPoint,
	AnalyticsDashboard,
	AnalyticsSignals,
	VibeTrack,
	BasicTrack,
	DiscoveryPreset,
	DiscoveryPreview,
	DiscoveryMode,
	DiscoveryExternalResult,
	DiscoveryExternalFeed,
	DiscoveryTrainingRun,
	DiscoveryStatus,
	DiscoveryEngine,
	DiscoveryTrainingSafetyProfile,
	RadioResponse,
	RadioBlend,
	RadioQueue,
	HomeReleasesResponse,
	TidalMixesResponse,
	TidalRadioStationsResponse,
	TidalHomeModulesResponse,
	TidalMoodsResponse,
	TidalDiscoverModuleResponse,
	LastfmStatus,
	ListenBrainzStatus,
	HomeRecommendationsResponse,
	DatabaseStats,
	HomeSuggestionsResponse,
	HomeShufflePicksResponse,
	LibraryTopArtist,
	LastfmAuthStartResponse,
	LastfmAuthCompleteResponse,
	HomePicksResponse,
	HomeArticlesResponse,
	HomeNewsResponse,
	AudioDspFeatures,
	AudioSearchParams,
	AudioSearchResponse,
	AudioFeaturesStats,
	GenreAudioMetrics,
	AudioDevice,
	AudioSettings,
} from './types';
export type * from './types';

const NOOR_PORT = String(import.meta.env.NOOR_PORT || '17600');
const API_BASE = `http://localhost:${NOOR_PORT}`;
export const DEFAULT_API_TIMEOUT_MS = 20_000;
export const BULK_QUEUE_API_TIMEOUT_MS = 90_000;
export const TIDAL_CATALOG_API_TIMEOUT_MS = 40_000;
// VACUUM rewrites the entire database. On a multi-GB library that is minutes,
// not seconds, and the default timeout would abort the request client-side while
// the server carried on and finished successfully - reporting a failure for work
// that actually worked.
export const COMPACT_DATABASE_TIMEOUT_MS = 30 * 60_000;

type ApiRequestInit = RequestInit & {
	timeoutMs?: number;
};

export class ApiTimeoutError extends Error {
	constructor(
		public path: string,
		public timeoutMs: number
	) {
		super(`API request timed out after ${timeoutMs} ms: ${path}`);
		this.name = 'ApiTimeoutError';
	}
}

export function getApiBase(): string {
	if (typeof window === 'undefined') {
		return API_BASE;
	}

	// Production: noor-server serves this page, so the API and WS are at the same
	// origin it loaded from. Means the port can change at runtime with no rebuild —
	// the frontend just follows the server wherever it is.
	if (!import.meta.env.DEV) {
		return window.location.origin;
	}

	// Dev: Vite serves the UI on its own port, so target the configured backend port.
	const { protocol, hostname } = window.location;
	return `${protocol}//${hostname}:${NOOR_PORT}`;
}

// ─── Token management ────────────────────────────────────────────────────────

const TOKEN_KEY = 'noor_api_token';
let memoryToken: string | null = null;

export function getStoredToken(): string | null {
	if (typeof localStorage === 'undefined') return memoryToken;
	try {
		return localStorage.getItem(TOKEN_KEY) ?? memoryToken;
	} catch {
		return memoryToken;
	}
}

export function setStoredToken(token: string): boolean {
	memoryToken = token;
	if (typeof localStorage === 'undefined') return false;
	try {
		localStorage.setItem(TOKEN_KEY, token);
		return true;
	} catch {
		return false;
	}
}

export function setMemoryToken(token: string): void {
	memoryToken = token;
}

export function clearPersistedToken(): void {
	if (typeof localStorage === 'undefined') return;
	try { localStorage.removeItem(TOKEN_KEY); } catch { /* memory fallback remains */ }
}

export function clearStoredToken(): void {
	memoryToken = null;
	if (typeof localStorage === 'undefined') return;
	try {
		localStorage.removeItem(TOKEN_KEY);
	} catch {
		// A blocked storage API must not prevent the in-memory session clearing.
	}
}

function requestTimeout(
	path: string,
	externalSignal: AbortSignal | null | undefined,
	timeoutMs: number
): {
	signal: AbortSignal | undefined;
	cleanup: () => void;
	timedOut: () => boolean;
} {
	if (timeoutMs <= 0) {
		return { signal: externalSignal ?? undefined, cleanup: () => {}, timedOut: () => false };
	}

	const controller = new AbortController();
	let timedOut = false;
	let timeoutId: ReturnType<typeof setTimeout> | null = null;

	const abortFromExternal = () => {
		controller.abort(externalSignal?.reason);
	};

	if (externalSignal?.aborted) {
		abortFromExternal();
	} else {
		externalSignal?.addEventListener('abort', abortFromExternal, { once: true });
	}

	timeoutId = setTimeout(() => {
		timedOut = true;
		controller.abort(new ApiTimeoutError(path, timeoutMs));
	}, timeoutMs);

	return {
		signal: controller.signal,
		cleanup: () => {
			if (timeoutId !== null) clearTimeout(timeoutId);
			externalSignal?.removeEventListener('abort', abortFromExternal);
		},
		timedOut: () => timedOut,
	};
}

function timeoutForOptions(
	options: ApiRequestInit | undefined,
	fallback = DEFAULT_API_TIMEOUT_MS
): number {
	return typeof options?.timeoutMs === 'number' ? options.timeoutMs : fallback;
}

// Drop-in replacement for fetch() that attaches the Bearer token and fires
// the noor:unauthorized event on 401, matching the behaviour of fetchApiResponse.
export async function authFetch(url: string, init?: ApiRequestInit): Promise<Response> {
	const token = getStoredToken();
	const headers = new Headers(init?.headers);
	if (token) headers.set('authorization', `Bearer ${token}`);
	const { timeoutMs: _timeoutMs, signal: externalSignal, ...fetchInit } = init ?? {};
	const timeout = requestTimeout(url, externalSignal, timeoutForOptions(init));
	let resp: Response;
	try {
		resp = await fetch(url, { ...fetchInit, headers, signal: timeout.signal });
	} catch (error) {
		if (timeout.timedOut()) throw new ApiTimeoutError(url, timeoutForOptions(init));
		throw error;
	} finally {
		timeout.cleanup();
	}
	if (resp.status === 401 && typeof window !== 'undefined') {
		window.dispatchEvent(new CustomEvent('noor:unauthorized'));
	}
	return resp;
}

function asRecord(value: unknown): Record<string, unknown> | null {
	return value && typeof value === 'object' && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: null;
}

function pickString(obj: Record<string, unknown>, keys: string[]): string | null {
	for (const key of keys) {
		const value = obj[key];
		if (typeof value === 'string' && value.trim()) return value;
	}
	return null;
}

function pickNumber(obj: Record<string, unknown>, keys: string[]): number | null {
	for (const key of keys) {
		const value = obj[key];
		if (typeof value === 'number' && Number.isFinite(value)) return value;
		if (typeof value === 'string') {
			const parsed = Number(value);
			if (Number.isFinite(parsed)) return parsed;
		}
	}
	return null;
}

function pickBoolean(obj: Record<string, unknown>, keys: string[]): boolean | null {
	for (const key of keys) {
		const value = obj[key];
		if (typeof value === 'boolean') return value;
	}
	return null;
}

function pickArray(obj: Record<string, unknown>, keys: string[]): unknown[] {
	for (const key of keys) {
		const value = obj[key];
		if (Array.isArray(value)) return value;
		const nested = asRecord(value);
		if (nested && Array.isArray(nested.items)) return nested.items;
	}
	return [];
}

function playlistOwnerName(value: unknown): string | null {
	if (typeof value === 'string' && value.trim()) return value;
	const owner = asRecord(value);
	return owner ? pickString(owner, ['display_name', 'displayName', 'name']) : null;
}

function normalizeSpotifyPlaylistSearchItem(raw: unknown): SpotifyPlaylistSearchItem | null {
	const item = asRecord(raw);
	if (!item) return null;
	const spotifyId = pickString(item, ['spotifyId', 'spotify_id', 'id']);
	if (!spotifyId) return null;
	return {
		spotifyId,
		title: pickString(item, ['title', 'name']),
		description: pickString(item, ['description']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		owner: playlistOwnerName(item.owner),
		followers: pickNumber(item, ['followers', 'follower_count', 'followerCount']),
		totalTracks: pickNumber(item, ['totalTracks', 'total_tracks', 'track_count', 'trackCount']),
	};
}

function normalizeSpotifyTidalState(raw: unknown): SpotifyTidalState {
	const item = asRecord(raw) ?? {};
	const status = pickString(item, ['status']);
	const allowed = ['pending', 'resolved', 'low_confidence', 'unresolved', 'error'] as const;
	const normalizedStatus = allowed.includes(status as (typeof allowed)[number])
		? (status as SpotifyTidalState['status'])
		: 'pending';
	return {
		status: normalizedStatus,
		id: pickNumber(item, ['id', 'tidal_id', 'tidalId']),
		confidence: pickNumber(item, ['confidence']) ?? 0,
		matchReason: pickString(item, ['matchReason', 'match_reason']),
		fromCache: pickBoolean(item, ['fromCache', 'from_cache']) ?? false,
	};
}

function normalizeArtistRefs(raw: unknown): { id: string | null; name: string | null }[] {
	return Array.isArray(raw)
		? raw
				.map(asRecord)
				.filter((artist): artist is Record<string, unknown> => artist !== null)
				.map((artist) => ({
					id: pickString(artist, ['id', 'spotifyId', 'spotify_id']),
					name: pickString(artist, ['name']),
				}))
		: [];
}

function normalizeSpotifyPlaylistTrack(raw: unknown): SpotifyPlaylistTrack | null {
	const item = asRecord(raw);
	if (!item) return null;
	return {
		source: 'spotify',
		spotifyId: pickString(item, ['spotifyId', 'spotify_id', 'id']),
		type: 'track',
		title: pickString(item, ['title', 'name']),
		primaryArtist: pickString(item, ['primaryArtist', 'primary_artist', 'artist']),
		artists: normalizeArtistRefs(item.artists),
		album: pickString(item, ['album', 'album_title', 'albumTitle']),
		albumId: pickString(item, ['albumId', 'album_id']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		durationMs: pickNumber(item, ['durationMs', 'duration_ms']),
		releaseDate: pickString(item, ['releaseDate', 'release_date']),
		explicit: pickBoolean(item, ['explicit']),
		trackNumber: pickNumber(item, ['trackNumber', 'track_number']),
		discNumber: pickNumber(item, ['discNumber', 'disc_number']),
		spotifyUrl: pickString(item, ['spotifyUrl', 'spotify_url', 'url']),
		previewUrl: pickString(item, ['previewUrl', 'preview_url']),
		playcount: pickNumber(item, ['playcount', 'play_count', 'playCount']),
		popularity: pickNumber(item, ['popularity']),
		isrc: pickString(item, ['isrc']),
		tidal: normalizeSpotifyTidalState(item.tidal),
	};
}

function normalizeSpotifyPlaylistMeta(raw: unknown): SpotifyPlaylistMeta {
	const item = asRecord(raw) ?? {};
	return {
		source: 'spotify',
		spotifyId: pickString(item, ['spotifyId', 'spotify_id', 'id']),
		type: 'playlist',
		title: pickString(item, ['title', 'name']),
		description: pickString(item, ['description']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		owner: playlistOwnerName(item.owner),
		followers: pickNumber(item, ['followers', 'follower_count', 'followerCount']),
		totalTracks: pickNumber(item, ['totalTracks', 'total_tracks', 'track_count', 'trackCount']),
		snapshotId: pickString(item, ['snapshotId', 'snapshot_id']),
	};
}

function normalizeSpotifyPlaylistDetail(raw: unknown): SpotifyPlaylistDetail {
	const item = asRecord(raw) ?? {};
	return {
		...normalizeSpotifyPlaylistMeta(item),
		tracks: pickArray(item, ['tracks', 'items'])
			.map(normalizeSpotifyPlaylistTrack)
			.filter((track): track is SpotifyPlaylistTrack => track !== null),
	};
}

function normalizeSpotifyAlbumSearchItem(raw: unknown): SpotifyAlbumSearchItem | null {
	const item = asRecord(raw);
	if (!item) return null;
	const spotifyId = pickString(item, ['spotifyId', 'spotify_id', 'id']);
	if (!spotifyId) return null;
	return {
		spotifyId,
		title: pickString(item, ['title', 'name']),
		primaryArtist: pickString(item, ['primaryArtist', 'primary_artist', 'artist']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		releaseDate: pickString(item, ['releaseDate', 'release_date']),
	};
}

function normalizeSpotifyArtistSearchItem(raw: unknown): SpotifyArtistSearchItem | null {
	const item = asRecord(raw);
	if (!item) return null;
	const spotifyId = pickString(item, ['spotifyId', 'spotify_id', 'id']);
	if (!spotifyId) return null;
	return {
		spotifyId,
		name: pickString(item, ['name', 'title']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		followers: pickNumber(item, ['followers', 'follower_count', 'followerCount']),
	};
}

function normalizeSpotifyTrackDetail(raw: unknown): SpotifyTrackDetail {
	const item = asRecord(raw) ?? {};
	return {
		source: 'spotify',
		spotifyId: pickString(item, ['spotifyId', 'spotify_id', 'id']),
		type: 'track',
		title: pickString(item, ['title', 'name']),
		primaryArtist: pickString(item, ['primaryArtist', 'primary_artist', 'artist']),
		artists: normalizeArtistRefs(item.artists),
		album: pickString(item, ['album', 'album_title', 'albumTitle']),
		albumId: pickString(item, ['albumId', 'album_id']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		durationMs: pickNumber(item, ['durationMs', 'duration_ms']),
		releaseDate: pickString(item, ['releaseDate', 'release_date']),
		explicit: pickBoolean(item, ['explicit']),
		trackNumber: pickNumber(item, ['trackNumber', 'track_number']),
		discNumber: pickNumber(item, ['discNumber', 'disc_number']),
		spotifyUrl: pickString(item, ['spotifyUrl', 'spotify_url', 'url']),
		previewUrl: pickString(item, ['previewUrl', 'preview_url']),
		playcount: pickNumber(item, ['playcount', 'play_count', 'playCount']),
		popularity: pickNumber(item, ['popularity']),
		isrc: pickString(item, ['isrc']),
		tidal: normalizeSpotifyTidalState(item.tidal),
	};
}

function normalizeSpotifyAlbumDetail(raw: unknown): SpotifyAlbumDetail {
	const item = asRecord(raw) ?? {};
	return {
		source: 'spotify',
		spotifyId: pickString(item, ['spotifyId', 'spotify_id', 'id']),
		type: 'album',
		title: pickString(item, ['title', 'name']),
		primaryArtist: pickString(item, ['primaryArtist', 'primary_artist', 'artist']),
		artists: normalizeArtistRefs(item.artists),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		releaseDate: pickString(item, ['releaseDate', 'release_date']),
		totalTracks: pickNumber(item, ['totalTracks', 'total_tracks', 'track_count', 'trackCount']),
		albumType: pickString(item, ['albumType', 'album_type']),
		label: pickString(item, ['label']),
		genres: Array.isArray(item.genres)
			? item.genres.filter((g): g is string => typeof g === 'string')
			: [],
		spotifyUrl: pickString(item, ['spotifyUrl', 'spotify_url', 'url']),
		tracks: pickArray(item, ['tracks', 'items'])
			.map(normalizeSpotifyPlaylistTrack)
			.filter((t): t is SpotifyPlaylistTrack => t !== null),
	};
}

function normalizeSpotifyArtistDetail(raw: unknown): SpotifyArtistDetail {
	const item = asRecord(raw) ?? {};
	return {
		source: 'spotify',
		spotifyId: pickString(item, ['spotifyId', 'spotify_id', 'id']),
		type: 'artist',
		name: pickString(item, ['name', 'title']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		genres: Array.isArray(item.genres)
			? item.genres.filter((g): g is string => typeof g === 'string')
			: [],
		popularity: pickNumber(item, ['popularity']),
		monthlyListeners: pickNumber(item, ['monthlyListeners', 'monthly_listeners']),
		followers: pickNumber(item, ['followers', 'follower_count', 'followerCount']),
		worldRank: pickNumber(item, ['worldRank', 'world_rank']),
		biography: pickString(item, ['biography', 'bio']),
	};
}

function collectPendingIds(raw: unknown): string[] {
	const item = asRecord(raw) ?? {};
	return [
		...(Array.isArray(item.pendingSpotifyIds) ? item.pendingSpotifyIds : []),
		...(Array.isArray(item.pending_spotify_ids) ? item.pending_spotify_ids : []),
	].filter((id): id is string => typeof id === 'string' && id.length > 0);
}

async function fetchApiResponse(
	path: string,
	params?: Record<string, string>,
	options?: ApiRequestInit
): Promise<Response> {
	const url = new URL(`${getApiBase()}${path}`);
	if (params) {
		Object.entries(params).forEach(([k, v]) => url.searchParams.set(k, v));
	}

	const token = getStoredToken();
	const headers = new Headers(options?.headers);
	if (!headers.has('content-type')) headers.set('content-type', 'application/json');
	if (token) headers.set('authorization', `Bearer ${token}`);

	const { timeoutMs: _timeoutMs, signal: externalSignal, ...fetchOptions } = options ?? {};
	const timeout = requestTimeout(path, externalSignal, timeoutForOptions(options));
	let resp: Response;
	try {
		resp = await fetch(url.toString(), {
			...fetchOptions,
			headers,
			signal: timeout.signal,
		});
	} catch (error) {
		if (timeout.timedOut()) throw new ApiTimeoutError(path, timeoutForOptions(options));
		throw error;
	} finally {
		timeout.cleanup();
	}

	if (resp.status === 401) {
		// Token was rejected, so dispatch an event for the connect screen.
		if (typeof window !== 'undefined') window.dispatchEvent(new CustomEvent('noor:unauthorized'));
	}

	return resp;
}

export class ApiError extends Error {
	/**
	 * Parsed response body, if available. Carries the corrective state for
	 * 409 responses from `POST /api/playback/position` (the route-side seek
	 * ack returns `{ state: PlaybackState }`) so the caller's catch block
	 * can `applyState(body.state)` instead of routing the failure into the
	 * generic error-toast path. Best-effort: parse failures leave this null.
	 */
	public body: unknown;

	constructor(public status: number, message: string, body?: unknown) {
		super(message);
		this.name = 'ApiError';
		this.body = body ?? null;
	}
}

async function fetchApi<T>(
	path: string,
	params?: Record<string, string>,
	options?: ApiRequestInit
): Promise<T> {
	const resp = await fetchApiResponse(path, params, options);
	if (!resp.ok) {
		const errorBody = await resp.json().catch(() => null);
		const message =
			errorBody?.message ??
			errorBody?.details ??
			errorBody?.error ??
			errorBody?.status ??
			`API error: ${resp.status}`;
		throw new ApiError(resp.status, message, errorBody);
	}
	return resp.json();
}

export const api = {
    getCatalogueStatus() {
        return fetchApi<{ tracks: { id: number; availability: string; favorite_state: string; releases: number }[] }>('/api/library/catalogue/status');
    },

    getArtworkCache() {
        return fetchApi<ArtworkCacheSettings>('/api/artwork-cache');
    },
    setArtworkCacheSize(max_mb: number) {
        return fetchApi<ArtworkCacheSettings>('/api/artwork-cache', undefined, {
            method: 'PUT', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ max_mb }),
        });
    },
    getVideoDiscoverySettings() {
        return fetchApi<{ setting: VideoDiscoverySetting }>('/api/videos/discovery/settings');
    },
    setVideoDiscoverySettings(setting: VideoDiscoverySetting) {
        return fetchApi<{ setting: VideoDiscoverySetting }>('/api/videos/discovery/settings', undefined, {
            method: 'PUT', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ setting }),
        });
    },
    getVideoStationSettings() {
        return fetchApi<VideoStationSettings>('/api/videos/stations/settings');
    },
    setVideoStationSettings(settings: { enabled: boolean; hidden: string[] }) {
        return fetchApi<VideoStationSettings>('/api/videos/stations/settings', undefined, {
            method: 'PUT', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(settings),
        });
    },
    getVideoDiscoveryStatus() {
        return fetchApi<VideoDiscoveryStatus>('/api/videos/discovery/status');
    },
    getTidalContentSettings() {
        return fetchApi<{ hide_ai_generated: boolean }>('/api/tidal/content-settings');
    },
    setTidalContentSettings(hide_ai_generated: boolean) {
        return fetchApi<{ hide_ai_generated: boolean }>('/api/tidal/content-settings', undefined, {
            method: 'PUT', headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ hide_ai_generated }),
        });
    },
	// `favoriteOnly` is legacy: server-side it currently means "library tracks"
	// (liked tracks ∪ tracks from favorited albums). Use `likedOnly` for a strict
	// filter on tracks the user has actually liked. likedOnly takes precedence
	// over favoriteOnly server-side.
	// TODO: drop the favoriteOnly default once all call sites pass it explicitly.
	getTracks(sortBy = 'date_added', sortDir = 'desc', limit = 50, offset = 0, favoriteOnly = true, likedOnly = false) {
		return fetchApi<{ tracks: Track[]; total: number }>('/api/tracks', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
			favorite_only: String(favoriteOnly),
			liked_only: String(likedOnly),
		});
	},

	getHistory(limit = 50, offset = 0) {
		return fetchApi<{ tracks: Track[]; total: number }>('/api/history', {
			limit: String(limit),
			offset: String(offset),
		});
	},

	getAlbums(
		sortBy = 'title',
		sortDir = 'asc',
		limit = 50,
		offset = 0,
		favoriteOnly = true,
		decade: number | null = null,
	) {
		const params: Record<string, string> = {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
			favorite_only: String(favoriteOnly),
		};
		if (decade != null) params.decade = String(decade);
		return fetchApi<{ albums: Album[]; total: number }>('/api/albums', params);
	},

	// Distinct decades (1950, 1960, ...) present in the album library. Powers the
	// library's decade-filter chips so a decade resolves server-side against the
	// full library instead of only the albums already paged into the client.
	getAlbumDecades(favoriteOnly = true) {
		return fetchApi<{ decades: number[] }>('/api/albums/decades', {
			favorite_only: String(favoriteOnly),
		});
	},

	getAlbumTracks(id: number) {
		return fetchApi<{
			tracks: Track[];
			tidal_tracks: TidalDiscographyTrack[];
			album_tidal_id: number | null;
			album_is_favorite: boolean;
		}>(`/api/albums/${id}/tracks`);
	},

	/** Liner-note facts: label (filled once from TIDAL), release date, year. */
	getAlbumCredits(id: number) {
		return fetchApi<{ label: string | null; release_date: string | null; year: number | null }>(`/api/albums/${id}/credits`);
	},

	getAlbumSpotifyStats(id: number) {
		return fetchApi<SpotifyTrackStats>(`/api/albums/${id}/spotify-stats`);
	},

	getArtists(sortBy = 'name', sortDir = 'asc', limit = 50, offset = 0) {
		return fetchApi<{ artists: Artist[] }>('/api/artists', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
		});
	},

	/** Artist total and the first list offset of each initial (A-Z, then #),
	 *  in the same order as getArtists. */
	getArtistLetters() {
		return fetchApi<{ total: number; letters: { letter: string; offset: number }[] }>('/api/artists/letters');
	},

	getArtist(id: number) {
		return fetchApi<{
			id: number;
			tidal_id: number | null;
			name: string;
			biography: string | null;
			photo_url: string | null;
			track_count: number;
			album_count: number;
		}>(`/api/artists/${id}`);
	},

	getArtistTracks(id: number) {
		return fetchApi<{ tracks: Track[] }>(`/api/artists/${id}/tracks`);
	},

	getArtistDiscography(id: number, preview = false) {
		return fetchApi<{
			albums: TidalDiscographyAlbum[];
			top_tracks: TidalDiscographyTrack[];
			videos: TidalArtistVideo[];
			similar_artists: TidalSimilarArtist[];
			bio: TidalArtistBio | null;
			picture_url: string | null;
			available: boolean;
			reason?: string;
			sections_failed?: string[];
			release_filter_status?: ArtistReleaseFilterStatuses;
		}>(`/api/artists/${id}/discography${preview ? '?preview=true' : ''}`);
	},

	getArtistSpotifyStats(id: number) {
		return fetchApi<SpotifyArtistStats>(`/api/artists/${id}/spotify-stats`);
	},

	getTidalAlbumTracks(tidalAlbumId: number) {
		return fetchApi<{ tracks: TidalDiscographyTrack[] }>(
			`/api/tidal/albums/${tidalAlbumId}/tracks`
		);
	},

	importTidalAlbum(tidalAlbumId: number) {
		return fetchApi<{ album_id: number; tracks: { tidal_id: number; local_id: number }[] }>(
			`/api/tidal/albums/${tidalAlbumId}/import`,
			undefined,
			{ method: 'POST' }
		);
	},

	importTidalTrackForRadio(track: TidalPlayable) {
		return fetchApi<{ tidal_id: number; local_id: number; artist_id: number; album_id: number | null }>('/api/tidal/tracks/import', undefined, {
			method: 'POST',
			body: JSON.stringify({
				tidal_id: track.tidal_id,
				title: track.title || 'Unknown title',
				artist_name: track.artist_name || 'Unknown artist',
				artist_tidal_id: track.artist_tidal_id ?? null,
				album_title: track.album_title,
				album_tidal_id: track.album_tidal_id ?? null,
				artwork_url: track.artwork_url,
				duration_ms: track.duration_ms,
			}),
		});
	},

	getGenres() {
		return fetchApi<{ genres: Genre[] }>('/api/genres');
	},

	getGenreGalaxySnapshot(days = 90) {
		return fetchApi<{
			genres: Genre[];
			heat: GenreHeat[];
			cohorts: GenreCohort[];
			evolution: GenreEvolutionPoint[];
			metrics: GenreAudioMetrics[];
		}>('/api/genres/snapshot', {
			days: String(days)
		});
	},

	getGenreHeat(days = 90) {
		return fetchApi<{ heat: GenreHeat[] }>('/api/genres/heat', {
			days: String(days)
		});
	},

	getGenreCohorts(days = 90) {
		return fetchApi<{ cohorts: GenreCohort[] }>('/api/genres/cohorts', {
			days: String(days)
		});
	},

	getGenreEvolution(days = 90) {
		return fetchApi<{ evolution: GenreEvolutionPoint[] }>('/api/genres/evolution', {
			days: String(days)
		});
	},

	getGenreTracks(id: number, includeDescendants = true) {
		return fetchApi<{ tracks: Track[] }>(`/api/genres/${id}/tracks`, {
			include_descendants: String(includeDescendants),
		});
	},

	getPlaylists() {
		return fetchApi<{ playlists: Playlist[] }>('/api/playlists');
	},

	getPlaylistTracks(id: number) {
		return fetchApi<{ tracks: Track[] }>(`/api/playlists/${id}/tracks`);
	},

	evaluateSmartPlaylist(id: number) {
		return fetchApi<{ playlist: Playlist; tracks: Track[]; resolved_count: number }>(
			`/api/smart/playlists/${id}/evaluate`
		);
	},

	createSmartPlaylist(name: string, description: string | null, rules: RuleClause) {
		return fetchApi<{ playlist: Playlist }>('/api/smart/playlists', undefined, {
			method: 'POST',
			body: JSON.stringify({ name, description, rules }),
		});
	},

	updateSmartPlaylist(id: number, name: string, description: string | null, rules: RuleClause) {
		return fetchApi<{ playlist: Playlist }>(`/api/smart/playlists/${id}`, undefined, {
			method: 'PUT',
			body: JSON.stringify({ name, description, rules }),
		});
	},

	deleteSmartPlaylist(id: number) {
		return fetchApi<{ deleted: boolean }>(`/api/smart/playlists/${id}`, undefined, {
			method: 'DELETE',
		});
	},

	togglePlaylistFavorite(id: number) {
		return fetchApi<{ playlist: Playlist }>(`/api/playlists/${id}/favorite`, undefined, {
			method: 'PATCH',
		});
	},

	addTracksToPlaylist(id: number, trackIds: number[]) {
		return fetchApi<{ added: number }>(`/api/playlists/${id}/tracks`, undefined, {
			method: 'POST',
			body: JSON.stringify({ track_ids: trackIds }),
		});
	},

	/** Create a regular playlist. Local-only: no counterpart is made on TIDAL. */
	createPlaylist(name: string, description: string | null = null) {
		return fetchApi<{ playlist: Playlist }>('/api/playlists', undefined, {
			method: 'POST',
			body: JSON.stringify({ name, description }),
		});
	},

	/**
	 * Rename a playlist and/or replace its description. For a TIDAL-mirrored
	 * playlist the server writes to TIDAL first, so a 409 here means the
	 * playlist changed remotely and the caller should refresh.
	 */
	updatePlaylist(id: number, name: string, description: string | null = null) {
		return fetchApi<{ playlist: Playlist }>(`/api/playlists/${id}`, undefined, {
			method: 'PATCH',
			body: JSON.stringify({ name, description }),
		});
	},

	/** Delete any playlist. TIDAL-mirrored ones are deleted on TIDAL too. */
	deletePlaylist(id: number) {
		return fetchApi<{ deleted: boolean }>(`/api/playlists/${id}`, undefined, {
			method: 'DELETE',
		});
	},

	/**
	 * Remove tracks by zero-based position, not by track id: a playlist may hold
	 * the same track twice, so an id would not say which copy to drop.
	 */
	removePlaylistTracks(id: number, positions: number[]) {
		return fetchApi<{ removed: number }>(`/api/playlists/${id}/tracks`, undefined, {
			method: 'DELETE',
			body: JSON.stringify({ positions }),
		});
	},

	/**
	 * Move a track within a playlist. `to` is the destination index measured
	 * after the moved row is lifted out - use `reorderDropIndex` to convert a
	 * drop target into it.
	 */
	movePlaylistTrack(id: number, from: number, to: number) {
		return fetchApi<{ tracks: Track[] }>(`/api/playlists/${id}/tracks/move`, undefined, {
			method: 'POST',
			body: JSON.stringify({ from, to }),
		});
	},

	/** Re-pull one TIDAL playlist's tracks now, without waiting for a sync. */
	refreshPlaylistFromTidal(id: number) {
		return fetchApi<{ tracks: number }>(`/api/playlists/${id}/refresh`, undefined, {
			method: 'POST',
		});
	},

	getPlaylistCoverSample(id: number, signal?: AbortSignal) {
		return fetchApi<{ urls: string[] }>(
			`/api/playlists/${id}/cover-sample`,
			undefined,
			{ signal },
		);
	},

	previewSmartPlaylist(rules: RuleClause, signal?: AbortSignal) {
		return fetchApi<{ count: number }>('/api/smart/playlists/preview', undefined, {
			method: 'POST',
			body: JSON.stringify({ rules }),
			signal,
		});
	},

	searchLibraryArtistNames(q: string, signal?: AbortSignal, limit = 20) {
		return fetchApi<{ artists: { id: number; name: string }[] }>(
			`/api/artists/search?q=${encodeURIComponent(q)}&limit=${limit}`,
			undefined,
			{ signal },
		);
	},

	searchTidalPlaylists(
		q: string,
		signal?: AbortSignal,
		opts?: { limit?: number; offset?: number; timeoutMs?: number },
	) {
		const limit = opts?.limit ?? 20;
		const offset = opts?.offset ?? 0;
		return fetchApi<{ playlists: TidalSearchPlaylist[] }>(
			`/api/tidal/playlists/search?q=${encodeURIComponent(q)}&limit=${limit}&offset=${offset}`,
			undefined,
			{ signal, timeoutMs: opts?.timeoutMs },
		);
	},

	/**
	 * Fetch a Spotify-sourced playlist's card metadata without tracks.
	 */
	async getSpotifyPlaylistMeta(spotifyId: string, signal?: AbortSignal): Promise<SpotifyPlaylistMeta> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/playlist/${encodeURIComponent(spotifyId)}/meta`,
			undefined,
			{ signal },
		);
		return normalizeSpotifyPlaylistMeta(raw);
	},

	/**
	 * Fetch a Spotify-sourced playlist's full metadata + track list, with
	 * each track stamped with its current TIDAL resolution state.
	 */
	getSpotifyPlaylist(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<{ playlist: SpotifyPlaylistDetail; pendingSpotifyIds: string[] }> {
		return fetchApi<{ playlist?: unknown; pendingSpotifyIds?: unknown; pending_spotify_ids?: unknown }>(
			`/api/discovery/sportify/playlist/${encodeURIComponent(spotifyId)}`,
			undefined,
			{ signal },
		).then((res) => ({
			playlist: normalizeSpotifyPlaylistDetail(res.playlist),
			pendingSpotifyIds: [
				...(Array.isArray(res.pendingSpotifyIds) ? res.pendingSpotifyIds : []),
				...(Array.isArray(res.pending_spotify_ids) ? res.pending_spotify_ids : []),
			].filter((id): id is string => typeof id === 'string' && id.length > 0),
		}));
	},

	async getSpotifyTrack(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<SpotifyTrackDetail> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/track/${encodeURIComponent(spotifyId)}`,
			undefined,
			{ signal },
		);
		return normalizeSpotifyTrackDetail(raw);
	},

	async getSpotifyAlbum(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<{ album: SpotifyAlbumDetail; pendingSpotifyIds: string[] }> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/album/${encodeURIComponent(spotifyId)}`,
			undefined,
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return {
			album: normalizeSpotifyAlbumDetail(root.album),
			pendingSpotifyIds: collectPendingIds(root),
		};
	},

	async getSpotifyArtist(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<SpotifyArtistDetail> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/artist/${encodeURIComponent(spotifyId)}`,
			undefined,
			{ signal },
		);
		return normalizeSpotifyArtistDetail(raw);
	},

	async getSpotifyArtistTopTracks(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<{ spotifyId: string; tracks: SpotifyPlaylistTrack[]; pendingSpotifyIds: string[] }> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/artist/${encodeURIComponent(spotifyId)}/top-tracks`,
			undefined,
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return {
			spotifyId: pickString(root, ['spotifyId', 'spotify_id']) ?? spotifyId,
			tracks: pickArray(root, ['tracks'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			pendingSpotifyIds: collectPendingIds(root),
		};
	},

	async getSpotifyArtistRelated(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<SpotifyArtistRelated> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/artist/${encodeURIComponent(spotifyId)}/related`,
			undefined,
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return {
			spotifyId: pickString(root, ['spotifyId', 'spotify_id']) ?? spotifyId,
			topTracks: pickArray(root, ['topTracks', 'top_tracks'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			deepCuts: pickArray(root, ['deepCuts', 'deep_cuts'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			recentReleases: pickArray(root, ['recentReleases', 'recent_releases'])
				.map(normalizeSpotifyAlbumSearchItem)
				.filter((a): a is SpotifyAlbumSearchItem => a !== null),
			similarArtists: pickArray(root, ['similarArtists', 'similar_artists'])
				.map(normalizeSpotifyArtistSearchItem)
				.filter((a): a is SpotifyArtistSearchItem => a !== null),
			pendingSpotifyIds: collectPendingIds(root),
		};
	},

	async getSpotifyAlbumRelated(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<SpotifyAlbumRelated> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/album/${encodeURIComponent(spotifyId)}/related`,
			undefined,
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return {
			spotifyId: pickString(root, ['spotifyId', 'spotify_id']) ?? spotifyId,
			moreFromArtist: pickArray(root, ['moreFromArtist', 'more_from_artist'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			moreAlbumsByArtist: pickArray(root, ['moreAlbumsByArtist', 'more_albums_by_artist'])
				.map(normalizeSpotifyAlbumSearchItem)
				.filter((a): a is SpotifyAlbumSearchItem => a !== null),
			pendingSpotifyIds: collectPendingIds(root),
		};
	},

	async getSpotifyTrackRelated(
		spotifyId: string,
		signal?: AbortSignal,
	): Promise<SpotifyTrackRelated> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/track/${encodeURIComponent(spotifyId)}/related`,
			undefined,
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return {
			spotifyId: pickString(root, ['spotifyId', 'spotify_id']) ?? spotifyId,
			moreFromAlbum: pickArray(root, ['moreFromAlbum', 'more_from_album'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			moreFromArtist: pickArray(root, ['moreFromArtist', 'more_from_artist'])
				.map(normalizeSpotifyPlaylistTrack)
				.filter((t): t is SpotifyPlaylistTrack => t !== null),
			pendingSpotifyIds: collectPendingIds(root),
		};
	},

	/**
	 * Cache-only resolution status poll. Used by the ephemeral playlist view
	 * to fill in lazy-tail tracks as the background resolver finishes them.
	 */
	getResolveTidalStatus(
		spotifyIds: string[],
		signal?: AbortSignal,
	): Promise<{ entries: ResolveStatusEntry[] }> {
		return fetchApi<{ entries: ResolveStatusEntry[] }>(
			`/api/resolve/tidal/status`,
			{ spotify_ids: spotifyIds.join(',') },
			{ signal },
		);
	},

	/**
	 * Save the ephemeral Spotify playlist into the user's library. Imports
	 * each resolved TIDAL track and creates a noor playlist; unresolved
	 * rows are skipped (counts come back in the response).
	 */
	saveSpotifyPlaylist(spotifyId: string, name?: string) {
		return fetchApi<{
			playlist: Playlist;
			added: number;
			totalTracks: number;
			resolvedCount: number;
			unresolvedCount: number;
			importFailures: number;
		}>(`/api/spotify-playlist/save`, undefined, {
			method: 'POST',
			body: JSON.stringify({ spotify_id: spotifyId, name }),
		});
	},

	saveSpotifyTrack(spotifyId: string) {
		return fetchApi<SpotifyLibrarySaveResponse>(`/api/spotify-track/save`, undefined, {
			method: 'POST',
			body: JSON.stringify({ spotify_id: spotifyId }),
		});
	},

	saveSpotifyAlbum(spotifyId: string) {
		return fetchApi<SpotifyLibrarySaveResponse>(`/api/spotify-album/save`, undefined, {
			method: 'POST',
			body: JSON.stringify({ spotify_id: spotifyId }),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
	},

	async searchSpotifyPlaylists(
		q: string,
		limit = 12,
		signal?: AbortSignal,
		offset = 0,
		timeoutMs?: number,
	): Promise<SpotifyPlaylistSearchItem[]> {
		type Resp = { playlists?: unknown[]; spotify_playlists?: unknown[] };
		const fromResponse = (res: Resp) =>
			[...(res.playlists ?? []), ...(res.spotify_playlists ?? [])]
				.map(normalizeSpotifyPlaylistSearchItem)
				.filter((item): item is SpotifyPlaylistSearchItem => item !== null);

		try {
			const res = await fetchApi<Resp>(
				`/api/discovery/sportify/search`,
				{ q, type: 'playlist', limit: String(limit), offset: String(offset) },
				{ signal, timeoutMs },
			);
			return fromResponse(res);
		} catch (error) {
			if (signal?.aborted) throw error;
			return [];
		}
	},

	getTidalPlaylistTracks(tidalUuid: string) {
		return fetchApi<{ tracks: TidalPlayable[] }>(
			`/api/tidal/playlists/${tidalUuid}/tracks`,
			undefined,
			{ timeoutMs: BULK_QUEUE_API_TIMEOUT_MS },
		);
	},

	getAnalyticsOverview() {
		return fetchApi<{ overview: AnalyticsOverview }>('/api/analytics/overview');
	},

	getAnalyticsDashboard(recentLimit = 12, topLimit = 8, days = 14) {
		return fetchApi<{ dashboard: AnalyticsDashboard }>('/api/analytics/dashboard', {
			recent_limit: String(recentLimit),
			top_limit: String(topLimit),
			days: String(days),
		});
	},

	async getAnalyticsSignals(days = 30): Promise<AnalyticsSignals> {
		const response = await fetchApi<{ signals: AnalyticsSignals }>(
			'/api/analytics/signals',
			{ days: String(days) },
		);
		const { signals } = response;

		// Length assertions - fail fast and loud if the server ever returns sparse rows.
		// Layout has not started rendering at this point, so a thrown error surfaces in
		// the page-level catch rather than producing mis-aligned ridges.
		const axis = signals.tempo.bucket_axis;
		const expectedBuckets = (axis.max - axis.min) / axis.step;
		for (const row of signals.tempo.rows) {
			if (row.buckets.length !== expectedBuckets) {
				throw new Error(
					`tempo row "${row.label}" has ${row.buckets.length} buckets, expected ${expectedBuckets}`,
				);
			}
		}
		for (const row of signals.ridgeline) {
			if (row.hourly.length !== 24) {
				throw new Error(
					`ridgeline row "${row.date}" has ${row.hourly.length} hours, expected 24`,
				);
			}
		}

		return signals;
	},

	getTrending(opts: {
		source?: TrendingSource;
		kind?: LastfmChartKind;
		limit?: number;
		country?: string; // ISO alpha-2 (e.g. "AU") OR a Last.fm full name; backend canonicalises.
		tag?: string;     // canonical curated genre key (e.g. "hip-hop"); mutually exclusive with country.
	} = {}) {
		const params: Record<string, string> = {};
		if (opts.source) params.source = opts.source;
		if (opts.kind) params.kind = opts.kind;
		if (opts.limit != null) params.limit = String(opts.limit);
		if (opts.country) params.country = opts.country;
		if (opts.tag) params.tag = opts.tag;
		return fetchApi<{
			source: string;
			kind?: LastfmChartKind;
			limit: number;
			country: string | null;
			tag: string | null;
			items?: ChartEntry[];
			tracks: ChartEntry[];
		}>('/api/charts', params);
	},

	getChartSnapshot(opts: {
		source?: string;
		period?: string;
		region?: string;
		limit?: number;
	} = {}) {
		const params: Record<string, string> = {};
		if (opts.source) params.source = opts.source;
		if (opts.period) params.period = opts.period;
		if (opts.region) params.region = opts.region;
		if (opts.limit != null) params.limit = String(opts.limit);
		return fetchApi<ChartSnapshotResponse>('/api/charts/snapshots', params);
	},

	getChartMatrix(opts: { regionGroup?: string } = {}) {
		const params: Record<string, string> = {};
		if (opts.regionGroup) params.region_group = opts.regionGroup;
		return fetchApi<ChartMatrixResponse>('/api/charts/matrix', params);
	},

	refreshChartMatrix() {
		return fetchApi<ChartMatrixRefreshResponse>('/api/charts/matrix/refresh', undefined, {
			method: 'POST',
			body: JSON.stringify({}),
		});
	},

	getLastfmGenres() {
		return fetchApi<{ genres: LastfmGenre[]; default_genre: string }>(
			'/api/charts/lastfm/genres',
		);
	},

	getLastfmCountries() {
		return fetchApi<{ countries: LastfmCountry[]; default_country: string }>(
			'/api/charts/lastfm/countries',
		);
	},

	getRecentListens(limit = 25) {
		return fetchApi<{ listens: ListenHistoryEntry[] }>('/api/analytics/listens/recent', {
			limit: String(limit),
		});
	},

	previewDiscovery(
		prompt: string,
		mode: DiscoveryMode,
		services: string[],
		limit = 8
	) {
		return fetchApi<{ preview: DiscoveryPreview }>('/api/discovery/preview', undefined, {
			method: 'POST',
			body: JSON.stringify({ prompt, mode, services, limit }),
		});
	},

	getDiscoveryPresets() {
		return fetchApi<{ presets: DiscoveryPreset[] }>('/api/discovery/presets');
	},

	getDiscoveryStatus() {
		return fetchApi<{ status: DiscoveryStatus }>('/api/discovery/status');
	},

	getDiscoveryTrainingStatus() {
		return fetchApi<{ run: DiscoveryTrainingRun | null }>('/api/discovery/train/status');
	},

	startDiscoveryTraining(mode: 'full' | 'incremental', rebuild_audio = false) {
		return fetchApi<{ status: string; mode: string; engine?: DiscoveryEngine; message?: string }>('/api/discovery/train', undefined, {
			method: 'POST',
			body: JSON.stringify({ mode, rebuild_audio }),
		});
	},

	stopDiscoveryTraining() {
		return fetchApi<{ status: string }>('/api/discovery/train/stop', undefined, {
			method: 'POST',
		});
	},

	getDiscoveryIntensity() {
		return fetchApi<{
			intensity: 'max' | 'medium' | 'low';
			dimension: number;
			top_k: number;
			window_size: number;
			include_audio_proxy: boolean;
			available: Array<'max' | 'medium' | 'low'>;
		}>('/api/discovery/train/intensity');
	},

	getDiscoveryEngine() {
		return fetchApi<{
			engine: DiscoveryEngine;
			label: string;
			family: string;
			trainable: boolean;
			available: DiscoveryEngine[];
		}>('/api/discovery/train/engine');
	},

	setDiscoveryEngine(engine: DiscoveryEngine) {
		return fetchApi<{
			engine: DiscoveryEngine;
			label: string;
			family: string;
			trainable: boolean;
		}>('/api/discovery/train/engine', undefined, {
			method: 'POST',
			body: JSON.stringify({ engine }),
		});
	},

	setDiscoveryIntensity(intensity: 'max' | 'medium' | 'low') {
		return fetchApi<{ intensity: string }>('/api/discovery/train/intensity', undefined, {
			method: 'POST',
			body: JSON.stringify({ intensity }),
		});
	},

	getDiscoverySafety() {
		return fetchApi<{
			track_count: number;
			intensity: 'max' | 'medium' | 'low';
			estimated_seconds: number;
			estimated_minutes: number;
			estimated_ram_mb: number;
			last_run_seconds: number | null;
			recommendation: 'safe' | 'moderate' | 'high_cost';
			safety_profile: DiscoveryTrainingSafetyProfile;
			safety_timeout_seconds: number;
			worker_threads: number;
			params: {
				dimension: number;
				top_k: number;
				window_size: number;
				include_audio_proxy: boolean;
			};
		}>('/api/discovery/train/safety');
	},

	getDiscoverySafetyProfile() {
		return fetchApi<{
			profile: DiscoveryTrainingSafetyProfile;
			label: string;
			worker_threads: number;
			available: DiscoveryTrainingSafetyProfile[];
		}>('/api/discovery/train/safety-profile');
	},

	setDiscoverySafetyProfile(profile: DiscoveryTrainingSafetyProfile) {
		return fetchApi<{
			profile: DiscoveryTrainingSafetyProfile;
			label: string;
			worker_threads: number;
		}>('/api/discovery/train/safety-profile', undefined, {
			method: 'POST',
			body: JSON.stringify({ profile }),
		});
	},

	/** Whether the one-time recommendations upgrade retrain is due or running. */
	getDiscoveryUpgrade() {
		return fetchApi<{ pending: boolean; running: boolean; trainer_version: number }>(
			'/api/discovery/upgrade'
		);
	},

	/** Mark (or unmark) a track or artist "Not for me" for every recommendation. */
	setNotForMe(kind: 'track' | 'artist', id: number, notForMe: boolean) {
		return fetchApi<{ kind: string; id: number; not_for_me: boolean }>(
			'/api/recommendations/not-for-me',
			undefined,
			{
				method: notForMe ? 'POST' : 'DELETE',
				body: JSON.stringify({ kind, id }),
			}
		);
	},

	recordDiscoveryFeedback(
		seed_track_id: number,
		candidate_track_id: number,
		action: string,
		surface: string,
		context?: Record<string, unknown>
	) {
		return fetchApi<{ recorded: boolean }>('/api/discovery/feedback', undefined, {
			method: 'POST',
			body: JSON.stringify({ seed_track_id, candidate_track_id, action, surface, context }),
		});
	},

	createDiscoveryPreset(
		name: string,
		prompt: string,
		mode: DiscoveryMode,
		services: string[]
	) {
		return fetchApi<{ preset: DiscoveryPreset }>('/api/discovery/presets', undefined, {
			method: 'POST',
			body: JSON.stringify({ name, prompt, mode, services }),
		});
	},

	discoverNewMusic(
		prompt: string,
		mode: DiscoveryMode,
		services: string[],
		limit = 10
	) {
		return fetchApi<{ feed: DiscoveryExternalFeed }>('/api/discovery/new', undefined, {
			method: 'POST',
			body: JSON.stringify({ prompt, mode, services, limit }),
		});
	},

	saveDiscoveryTrack(result: DiscoveryExternalResult) {
		return fetchApi<{ saved: boolean; provider: string; provider_track_id: string; message: string }>(
			'/api/discovery/save',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify(result),
			}
		);
	},

	playDiscoveryTrack(result: DiscoveryExternalResult) {
		return fetchApi<PlaybackSnapshot>('/api/discovery/play', undefined, {
			method: 'POST',
			body: JSON.stringify(result),
		});
	},

	findDiscoveryConnections(
		prompt: string,
		mode: DiscoveryMode,
		services: string[],
		seed: DiscoveryExternalResult,
		limit = 8
	) {
		return fetchApi<{ feed: DiscoveryExternalFeed }>('/api/discovery/connections', undefined, {
			method: 'POST',
			body: JSON.stringify({ prompt, mode, services, seed, limit }),
		});
	},

	// Similar Radio
	getRadioTracks(params: {
		seed_track_id?: number;
		seed_tidal_id?: number;
		creativity?: number;
		context_window?: number;
		limit?: number;
		exclude_ids?: number[];
	}) {
		return fetchApi<RadioResponse>('/api/discovery/radio', undefined, {
			method: 'POST',
			body: JSON.stringify(params),
		});
	},

	startRadioSong(params: {
		seed_track_id: number;
		blend?: RadioBlend;
		limit?: number;
		exclude_track_ids?: number[];
	}): Promise<RadioQueue> {
		return fetchApi<RadioQueue>('/api/radio/song', undefined, {
			method: 'POST',
			body: JSON.stringify(params),
		});
	},

	/** POST /api/radio/start - atomically builds queue and returns first playable item. */
	startRadioStart(params: {
		seed_track_id: number;
		blend?: RadioBlend;
		limit?: number;
	}): Promise<{
		state: PlaybackState;
		queue: QueueItem[];
		queue_revision?: number;
		first_playable: {
			type: 'library' | 'pending';
			queue_item_id: number;
			track_id: number | null;
		};
		pending_count?: number;
	}> {
		return fetchApi('/api/radio/start', undefined, {
			method: 'POST',
			body: JSON.stringify(params),
		});
	},

	startRadioAlbum(params: {
		seed_album_id: number;
		blend?: RadioBlend;
		limit?: number;
		exclude_track_ids?: number[];
	}): Promise<RadioQueue> {
		return fetchApi<RadioQueue>('/api/radio/album', undefined, {
			method: 'POST',
			body: JSON.stringify(params),
		});
	},

	startRadioArtist(params: {
		seed_artist_id: number;
		blend?: RadioBlend;
		limit?: number;
		exclude_track_ids?: number[];
	}): Promise<RadioQueue> {
		return fetchApi<RadioQueue>('/api/radio/artist', undefined, {
			method: 'POST',
			body: JSON.stringify(params),
		});
	},

	computeRadioSimilarity() {
		return fetchApi<{ status: string; message: string }>('/api/discovery/radio/compute', undefined, {
			method: 'POST',
		});
	},

	getRadioSimilarityStatus() {
		return fetchApi<{ row_count: number; built_at: string | null }>(
			'/api/discovery/radio/status',
		);
	},

	search(query: string, limit = 20, signal?: AbortSignal) {
		return fetchApi<SearchResults>('/api/search', { q: query, limit: String(limit) }, { signal });
	},

	searchAudio(params: AudioSearchParams, signal?: AbortSignal) {
		// Strip null/undefined fields before sending
		const body: Record<string, unknown> = {};
		for (const [k, v] of Object.entries(params)) {
			if (v !== null && v !== undefined) body[k] = v;
		}
		return fetchApi<AudioSearchResponse>('/api/search/audio', undefined, {
			signal,
			method: 'POST',
			body: JSON.stringify(body),
		});
	},

	getStatus() {
		return fetchApi<{ name: string; version: string; status: string }>('/api/status');
	},

	getPlaybackState() {
		return fetchApi<PlaybackSnapshot>('/api/playback/state');
	},

	getPlaybackRuntime() {
		return fetchApi<{ available: boolean; runtime: PlaybackRuntimeInfo | null; stream: StreamDisplayInfo | null }>(
			'/api/playback/runtime'
		);
	},

	getDjEnabled(): Promise<DjEnabledResponse> {
		return fetchApi<DjEnabledResponse>('/api/dj/enabled');
	},

	setDjEnabled(enabled: boolean): Promise<DjEnabledResponse> {
		return fetchApi<DjEnabledResponse>('/api/dj/enabled', undefined, {
			method: 'PUT',
			body: JSON.stringify({ enabled }),
		});
	},

	getDjStatus(): Promise<DjStatusResponse> {
		return fetchApi<DjStatusResponse>('/api/dj/status');
	},

	getDjProfile(trackId: number): Promise<DjProfileResponse> {
		return fetchApi<DjProfileResponse>(`/api/dj/profile/${trackId}`);
	},

	getDjMixIntent(): Promise<DjMixIntentResponse> {
		return fetchApi<DjMixIntentResponse>('/api/dj/mix-intent');
	},

	setDjMixIntent(intent: DjMixIntent): Promise<DjMixIntentResponse> {
		return fetchApi<DjMixIntentResponse>('/api/dj/mix-intent', undefined, {
			method: 'PUT',
			body: JSON.stringify({ intent }),
		});
	},

	getDjPolicy(): Promise<DjPolicyResponse> {
		return fetchApi<DjPolicyResponse>('/api/dj/policy');
	},

	setDjPolicy(policy: Partial<DjPolicyResponse>): Promise<DjPolicyResponse> {
		return fetchApi<DjPolicyResponse>('/api/dj/policy', undefined, {
			method: 'PUT',
			body: JSON.stringify(policy),
		});
	},

	async setDjProfileCorrection(correction: DjProfileCorrectionRequest): Promise<void> {
		await fetchApi<unknown>('/api/dj/profile-correction', undefined, {
			method: 'POST',
			body: JSON.stringify(correction),
		});
	},

	rebuildDjProfile(
		request: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>,
	): Promise<{ accepted: boolean; status: string }> {
		return fetchApi<{ accepted: boolean; status: string }>('/api/dj/profile-rebuild', undefined, {
			method: 'POST',
			body: JSON.stringify(request),
		});
	},

	async recordDjFeedback(feedback: DjFeedbackRequest): Promise<void> {
		await fetchApi<unknown>('/api/dj/feedback', undefined, {
			method: 'POST',
			body: JSON.stringify(feedback),
		});
	},
	getMusicBrainzStatus() {
		return fetchApi<MusicBrainzStatus>('/api/library/enrich/musicbrainz/status');
	},

	getPortableMusicBrainzSnapshot() {
		return fetchApi<PortableMusicBrainzSnapshotStatus>('/api/library/enrich/musicbrainz/portable');
	},

	exportPortableMusicBrainzSnapshot() {
		return fetchApi<PortableMusicBrainzSnapshotAction>(
			'/api/library/enrich/musicbrainz/portable/export',
			undefined,
			{ method: 'POST' }
		);
	},

	importPortableMusicBrainzSnapshot() {
		return fetchApi<PortableMusicBrainzSnapshotAction>(
			'/api/library/enrich/musicbrainz/portable/import',
			undefined,
			{ method: 'POST' }
		);
	},

	playTrack(trackId: number) {
		return fetchApi<PlaybackSnapshot>('/api/playback/play', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_id: trackId }),
		});
	},

	pausePlayback() {
		return fetchApi<{ state: PlaybackState }>('/api/playback/pause', undefined, {
			method: 'POST',
		});
	},

	resumePlayback() {
		return fetchApi<{ state: PlaybackState }>('/api/playback/resume', undefined, {
			method: 'POST',
		});
	},

	/** Ask the server to drop the WASAPI exclusive device so the WebView can
	 *  play a video's audio in shared mode. No-op when exclusive mode is off.
	 *  Swallows failures: video startup must never block on it. */
	releaseExclusivePlayback() {
		return fetchApi<{ ok: boolean }>('/api/playback/exclusive/release', undefined, {
			method: 'POST',
		}).catch(() => ({ ok: false }));
	},

	previousTrack() {
		return fetchApi<PlaybackSnapshot>('/api/playback/previous', undefined, {
			method: 'POST',
		});
	},

	nextTrack() {
		return fetchApi<PlaybackSnapshot>('/api/playback/next', undefined, {
			method: 'POST',
		});
	},

	setPlaybackVolume(volume: number) {
		return fetchApi<{ state: PlaybackState }>('/api/playback/volume', undefined, {
			method: 'POST',
			body: JSON.stringify({ volume }),
		});
	},

	setPlaybackPosition(positionMs: number, allowSegmentSeek = false) {
		return fetchApi<{ state: PlaybackState }>('/api/playback/position', undefined, {
			method: 'POST',
			body: JSON.stringify({
				position_ms: positionMs,
				allow_segment_seek: allowSegmentSeek,
			}),
		});
	},

	setPlaybackShuffle(mode: PlaybackState['shuffle_mode']) {
		return fetchApi<PlaybackSnapshot>('/api/playback/shuffle', undefined, {
			method: 'POST',
			body: JSON.stringify({ mode }),
		});
	},

	setPlaybackRepeat(mode: PlaybackState['repeat_mode']) {
		return fetchApi<{ state: PlaybackState }>('/api/playback/repeat', undefined, {
			method: 'POST',
			body: JSON.stringify({ mode }),
		});
	},

	setPlaybackAutomix(
		enabled: boolean,
		crossfade_ms?: number,
		discover_new?: boolean,
		use_learning?: boolean,
		allow_external?: boolean
	) {
		return fetchApi<PlaybackSnapshot>('/api/playback/automix', undefined, {
			method: 'POST',
			body: JSON.stringify({ enabled, crossfade_ms, discover_new, use_learning, allow_external }),
		});
	},

	addQueueTrack(trackId: number) {
		return fetchApi<QueueSnapshot>('/api/playback/queue/add', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_id: trackId }),
		});
	},

	queuePlayNext(req: QueueExternalRequest) {
		return fetchApi<QueueSnapshot>('/api/queue/play_next', undefined, {
			method: 'POST',
			body: JSON.stringify(req),
		});
	},

	queuePlayNextMany(items: QueueExternalRequest[]) {
		return fetchApi<QueueSnapshot>('/api/queue/play_next_many', undefined, {
			method: 'POST',
			body: JSON.stringify({ items }),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
	},

	queueAppend(req: QueueExternalRequest) {
		return fetchApi<QueueSnapshot>('/api/queue/append', undefined, {
			method: 'POST',
			body: JSON.stringify(req),
		});
	},

	queueAppendMany(items: QueueExternalRequest[]) {
		return fetchApi<QueueSnapshot>('/api/queue/append_many', undefined, {
			method: 'POST',
			body: JSON.stringify({ items }),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
	},

	replacePlaybackQueue(
		items: MixedQueueItem[],
		options?: { shuffleMode?: PlaybackState['shuffle_mode']; startPlayback?: boolean }
	) {
		const body: Record<string, unknown> = { items };
		if (options?.shuffleMode && options.shuffleMode !== 'off') {
			body.shuffle_mode = options.shuffleMode;
		}
		if (options?.startPlayback) body.start_playback = true;
		return fetchApi<{
			queued_count: number;
			pending_count: number;
			shuffle_debug?: ShuffleDebug | null;
			state: PlaybackState;
			queue: QueueItem[];
			queue_revision?: number;
		}>(
			'/api/playback/queue',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify(body),
				timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
			}
		);
	},

	/**
	 * Jump playback to a specific queue row (library or pending) and start it.
	 * Anchoring by queue-item id keeps duplicate tracks unambiguous.
	 */
	playQueueItem(queueItemId: number) {
		return fetchApi<PlaybackSnapshot>(
			'/api/playback/queue/play-item',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify({ queue_item_id: queueItemId }),
			}
		);
	},

	removeQueueTrack(queueItemId: number) {
		return fetchApi<QueueSnapshot & { playback_state?: PlaybackState }>(
			'/api/playback/queue/remove',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify({ queue_item_id: queueItemId }),
			}
		);
	},

	moveQueueTrack(itemId: number, newPos: number) {
		return fetchApi<QueueSnapshot & { playback_state?: PlaybackState }>(
			'/api/playback/queue/move',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify({ item_id: itemId, new_pos: newPos }),
			}
		);
	},

	clearQueue() {
		return fetchApi<QueueSnapshot & { playback_state?: PlaybackState }>('/api/playback/queue/clear', undefined, {
			method: 'POST',
			body: JSON.stringify({}),
		});
	},

	createPlaylistFromQueue(name: string, includeTidalOnly: boolean = true) {
		return fetchApi<{ playlist: { id: number; name: string }; added: number }>(
			'/api/playlists/from-queue',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify({ name, include_tidal_only: includeTidalOnly }),
			}
		);
	},

	setTrackFavorite(trackId: number, favorite: boolean) {
		return fetchApi<TrackFavoriteResponse>('/api/library/tracks/favorite', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_id: trackId, favorite }),
		});
	},

	setAlbumFavorite(albumId: number, favorite: boolean) {
		return fetchApi<{
			album_id: number;
			tidal_id: number | null;
			favorite: boolean;
			updated: boolean;
		}>('/api/library/albums/favorite', undefined, {
			method: 'POST',
			body: JSON.stringify({ album_id: albumId, favorite }),
		});
	},

	batchAddToPlaylist(playlistId: number, trackIds: number[]) {
		return fetchApi<{
			playlist_id: number;
			requested_tracks: number;
			resolved_tracks: number;
			added: number;
		}>('/api/library/batch/add-to-playlist', undefined, {
			method: 'POST',
			body: JSON.stringify({ playlist_id: playlistId, track_ids: trackIds }),
		});
	},

	batchDelete(trackIds: number[], albumIds: number[] = []) {
		return fetchApi<{
			requested_tracks: number;
			requested_albums: number;
			removed_tracks: number;
			removed_albums: number;
			resolved_tracks: number;
			resolved_albums: number;
		}>('/api/library/batch/delete', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_ids: trackIds, album_ids: albumIds }),
		});
	},

	batchSetGenre(genreId: number, trackIds: number[]) {
		return fetchApi<{
			genre_id: number;
			requested_tracks: number;
			affected: number;
		}>('/api/library/batch/set-genre', undefined, {
			method: 'POST',
			body: JSON.stringify({ genre_id: genreId, track_ids: trackIds }),
		});
	},

	// ─── Home Page Discovery ───────────────────────────────────────────────

	getHomeReleases() {
		return fetchApi<HomeReleasesResponse>('/api/home/releases');
	},

	getHomePicks() {
		return fetchApi<HomePicksResponse>('/api/home/picks');
	},

	getHomeArticles() {
		return fetchApi<HomeArticlesResponse>('/api/home/articles');
	},

	getHomeNews() {
		return fetchApi<HomeNewsResponse>('/api/home/news');
	},

	getHomeRecommendations() {
		return fetchApi<HomeRecommendationsResponse>('/api/home/recommendations');
	},

	// Hidden-gem suggestions for the Library home murals. Seeds are advisory:
	// pass the user's recent listens to prime the "recent" slice, or omit them
	// and let the server derive everything from listen history.
	getHomeSuggestions(seedTrackIds: number[] = [], limit?: number) {
		return fetchApi<HomeSuggestionsResponse>('/api/home/suggestions', undefined, {
			method: 'POST',
			body: JSON.stringify({ seed_track_ids: seedTrackIds, limit }),
		});
	},

	// Library shuffle picks for the Random tracks / Random albums murals. One
	// request for both panels: the client used to derive random offsets from the
	// library totals and issue a single-row request per pick, which could not
	// start until the library store had paged in.
	getHomeShufflePicks(limit = 12) {
		return fetchApi<HomeShufflePicksResponse>('/api/home/shuffle-picks', { limit: String(limit) });
	},

	// Library hero: artists ranked by plays summed per artist server-side, so
	// plays spread over many tracks count in full.
	getLibraryTopArtists(limit = 20) {
		return fetchApi<{ artists: LibraryTopArtist[] }>('/api/library/top-artists', { limit: String(limit) });
	},

	// ─── TIDAL: Your Mixes ────────────────────────────────────────────────
	// 503 here means TIDAL isn't connected - the YourMixesShelf surfaces a
	// connect prompt rather than an error toast.
	getTidalMixes() {
		return fetchApi<TidalMixesResponse>('/api/tidal/mixes', undefined, {
			timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS,
		});
	},

	// Personal Radio Stations - same 503/connect-prompt contract as getTidalMixes.
	getTidalRadioStations() {
		return fetchApi<TidalRadioStationsResponse>('/api/tidal/radio-stations', undefined, {
			timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS,
		});
	},

	// Editorial home modules from TIDAL pages/home - drives the search-page
	// discover surface. 503 when TIDAL is disconnected.
	getTidalHomeModules() {
		return fetchApi<TidalHomeModulesResponse>('/api/tidal/home-modules', undefined, {
			timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS,
		});
	},

	// Generic editorial page fetch - drives /charts, /moods, and (eventually)
	// /genres / /new-releases. Backend whitelists the section + optional id.
	// Path is split on / so each segment is encoded individually (needed for
	// `mood/{id}` style two-segment paths).
	getTidalPage(path: string) {
		return fetchApi<TidalHomeModulesResponse>(
			`/api/tidal/page/${path.split('/').map(encodeURIComponent).join('/')}`,
			undefined,
			{ timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS },
		);
	},

	// TIDAL moods landing: returns the PAGE_LINKS category list (Party,
	// Workout, Focus, etc). Each entry has a slug that can be fed to
	// getTidalMoodPage for the drill-down content.
	getTidalMoods() {
		return fetchApi<TidalMoodsResponse>('/api/tidal/moods', undefined, {
			timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS,
		});
	},

	// Drill-down for one mood category. Backend proxies to pages/{slug} which
	// returns the standard editorial modules shape.
	getTidalMoodPage(slug: string, signal?: AbortSignal) {
		return fetchApi<TidalHomeModulesResponse>(
			`/api/tidal/mood-page/${encodeURIComponent(slug)}`,
			undefined,
			{ signal },
		);
	},

	// Full item set for one home discover module (used by the "View all"
	// detail route). Backend follows the module's `dataApiPath` server-side.
	getTidalDiscoverModule(moduleId: string, limit = 50) {
		return fetchApi<TidalDiscoverModuleResponse>(
			`/api/tidal/discover-modules/${encodeURIComponent(moduleId)}/items?limit=${limit}`
		);
	},

	// Mix track list - used to queue + play a mix when a card is clicked.
	getTidalMixTracks(mixId: string) {
		return fetchApi<{ tracks: TidalDiscographyTrack[] }>(
			`/api/tidal/mixes/${encodeURIComponent(mixId)}/tracks`,
			undefined,
			{ timeoutMs: TIDAL_CATALOG_API_TIMEOUT_MS },
		);
	},


	// ─── Last.fm scrobble auth (server-side web-auth flow) ────────────────
	getLastfmStatus() {
		return fetchApi<LastfmStatus>('/api/lastfm/status');
	},

	saveLastfmConfig(api_key: string, api_secret: string) {
		return fetchApi<{ status: string; message?: string }>('/api/lastfm/config', undefined, {
			method: 'POST',
			body: JSON.stringify({ api_key, api_secret }),
		});
	},

	clearLastfmConfig() {
		return fetchApi<{ status: string }>('/api/lastfm/config', undefined, {
			method: 'DELETE',
		});
	},

	getListenBrainzStatus() {
		return fetchApi<ListenBrainzStatus>('/api/listenbrainz/status');
	},

	saveListenBrainzConfig(token: string) {
		return fetchApi<{ status: string; user?: string; message?: string }>('/api/listenbrainz/config', undefined, {
			method: 'POST',
			body: JSON.stringify({ token }),
		});
	},

	clearListenBrainzConfig() {
		return fetchApi<{ status: string }>('/api/listenbrainz/config', undefined, {
			method: 'DELETE',
		});
	},

	backfillScrobbles() {
		return fetchApi<{ status: string; days: number; eligible?: number; providers?: number; queued: number }>('/api/scrobbling/backfill', undefined, {
			method: 'POST',
		});
	},

	// 501 here means LASTFM_API_SECRET isn't configured on the server.
	lastfmAuthStart() {
		return fetchApi<LastfmAuthStartResponse>('/api/lastfm/auth/start', undefined, {
			method: 'POST',
		});
	},

	lastfmAuthComplete() {
		return fetchApi<LastfmAuthCompleteResponse>('/api/lastfm/auth/complete', undefined, {
			method: 'POST',
		});
	},

	lastfmAuthDisconnect() {
		return fetchApi<{ status: string }>('/api/lastfm/auth/disconnect', undefined, {
			method: 'POST',
		});
	},

	// ─── Audio Analysis ───────────────────────────────────────────────

	startAudioAnalysis(mode: 'preview' | 'local', localPath?: string) {
		return fetchApi<{ status: string; mode: string }>('/api/library/analyze/audio-features', undefined, {
			method: 'POST',
			body: JSON.stringify({ mode, local_path: localPath }),
		});
	},

	getAudioAnalysisStatus() {
		return fetchApi<{ running: boolean; analyzed: number }>('/api/library/analyze/status');
	},

	getPassiveDsp() {
		return fetchApi<{ enabled: boolean }>('/api/library/analyze/passive');
	},

	setPassiveDsp(enabled: boolean) {
		return fetchApi<{ enabled: boolean }>('/api/library/analyze/passive', undefined, {
			method: 'PUT',
			body: JSON.stringify({ enabled }),
		});
	},

	stopAudioAnalysis() {
		return fetchApi<{ status: string }>('/api/library/analyze/stop', undefined, { method: 'POST' });
	},

	getTrackAudioFeatures(trackId: number) {
		return fetchApi<{ features: AudioDspFeatures | null }>(`/api/tracks/${trackId}/audio-features`);
	},

	setBpmMultiplier(trackId: number, factor: number) {
		return fetchApi<{
			ok: boolean;
			track_id: number;
			old_bpm: number;
			new_bpm: number;
			manual_override: boolean;
		}>(`/api/tracks/${trackId}/bpm-multiplier`, undefined, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ factor })
		});
	},

	getAudioFeaturesStats() {
		return fetchApi<{ stats: AudioFeaturesStats }>('/api/library/audio-features/stats');
	},

	resetAudioAnalysis() {
		return fetchApi<{ status: string }>('/api/library/analyze/reset', undefined, {
			method: 'DELETE',
		});
	},

	getGenreAudioMetrics() {
		return fetchApi<{ metrics: GenreAudioMetrics[] }>('/api/genres/audio-metrics');
	},

	searchTidal(
		q: string,
		limit = 20,
		signal?: AbortSignal,
		offset = 0,
		timeoutMs?: number,
	): Promise<TidalSearchResults> {
		return fetchApi<TidalSearchResults>(
			'/api/tidal/search',
			{ q, limit: String(limit), offset: String(offset) },
			{ signal, timeoutMs },
		);
	},

	searchTidalVideos(
		q: string,
		limit = 20,
		offset = 0,
		signal?: AbortSignal
	): Promise<{ videos: TidalSearchVideo[] }> {
		return fetchApi<{ videos: TidalSearchVideo[] }>(
			'/api/tidal/videos/search',
			{ q, limit: String(limit), offset: String(offset) },
			{ signal },
		);
	},

	getTidalVideoStream(videoId: number, quality = 'HIGH'): Promise<TidalVideoStream> {
		return fetchApi<TidalVideoStream>(
			`/api/tidal/videos/${videoId}/playback`,
			{ quality },
		);
	},

	/** Record that a video started playing, so the editorial builder can hold it
	 *  out of the next few rotations. Fire-and-forget; failures are ignorable. */
	recordVideoHistory(body: {
		tidal_video_id: number;
		title?: string | null;
		artist_tidal_id?: number | null;
		artist_name?: string | null;
		/** The card itself, kept so the Recently watched shelf can draw it. */
		video?: TidalSearchVideo | null;
	}): Promise<{ ok: boolean; id?: number }> {
		return fetchApi<{ ok: boolean; id?: number }>('/api/videos/history', undefined, {
			method: 'POST',
			body: JSON.stringify(body),
		});
	},

	/** Recent watches, newest first, one per video (Recently watched, History). */
	getVideoHistory(limit = 40): Promise<{ items: VideoHistoryEntry[] }> {
		return fetchApi<{ items: VideoHistoryEntry[] }>(`/api/videos/history?limit=${limit}`);
	},

	clearVideoHistory(): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>('/api/videos/history', undefined, { method: 'DELETE' });
	},

	removeVideoFromHistory(videoId: number): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>(`/api/videos/history/videos/${videoId}`, undefined, { method: 'DELETE' });
	},

	finishVideoHistory(
		id: number,
		body: { watched_ms: number; video_duration_ms: number | null; completed: boolean },
	): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>(`/api/videos/history/${id}/finish`, undefined, {
			method: 'POST',
			body: JSON.stringify(body),
		});
	},

	getTidalVideoMixItems(mixId: string | number): Promise<{ items: TidalVideoMixItem[] }> {
		return fetchApi<{ items: TidalVideoMixItem[] }>(
			`/api/tidal/video-mixes/${encodeURIComponent(String(mixId))}/items`
		);
	},

	getTidalVideoPlaylistItems(uuid: string): Promise<{ items: TidalSearchVideo[] }> {
		return fetchApi<{ items: TidalSearchVideo[] }>(
			`/api/tidal/video-playlists/${encodeURIComponent(uuid)}/items`
		);
	},

	/** Editorial video sets for the /videos browse state. Stale-while-
	 *  revalidate server-side: an empty `sets` with `building: true` means
	 *  today's snapshot is being assembled and a re-fetch will pick it up. */
	getVideosDiscover(): Promise<VideoDiscoverResponse> {
		return fetchApi<VideoDiscoverResponse>('/api/videos/discover');
	},

	/** Fetch the next small video radio batch. Server uses its local catalog first. */
	getVideoRadioNext(body: {
		seed_artist_id: number | null;
		seed_artist_name: string | null;
		seed_video_id?: number | null;
		exclude_video_ids: number[];
		recent_video_ids: number[];
		recent_songs: { artist_id: number | null; artist_name: string | null; title: string }[];
		recent_artist_ids: number[];
	}): Promise<{ items: TidalSearchVideo[]; unfamiliar_video_ids: number[]; building?: boolean }> {
		return fetchApi<{ items: TidalSearchVideo[]; unfamiliar_video_ids: number[]; building?: boolean }>('/api/videos/radio/next', undefined, {
			method: 'POST',
			body: JSON.stringify(body),
		});
	},

	getVideoStations(): Promise<VideoStationsResponse> {
		return fetchApi<VideoStationsResponse>('/api/videos/stations');
	},

	getVideoStationNext(id: string, body: {
		exclude_video_ids: number[];
		recent_video_ids: number[];
		session_nonce: string;
	}): Promise<{ items: TidalSearchVideo[]; exhausted: boolean }> {
		return fetchApi<{ items: TidalSearchVideo[]; exhausted: boolean }>(
			`/api/videos/stations/${encodeURIComponent(id)}/next`, undefined, {
				method: 'POST',
				body: JSON.stringify(body),
			});
	},

	getRelatedVideos(body: {
		seed_artist_id: number | null;
		seed_artist_name: string | null;
		exclude_video_ids: number[];
	}, signal?: AbortSignal): Promise<{ items: (TidalSearchVideo & { why?: string })[]; building: boolean }> {
		return fetchApi<{ items: (TidalSearchVideo & { why?: string })[]; building: boolean }>('/api/videos/related', undefined, {
			method: 'POST', body: JSON.stringify(body), signal,
		});
	},

	getSavedVideos(): Promise<{ items: TidalSearchVideo[] }> {
		return fetchApi<{ items: TidalSearchVideo[] }>('/api/videos/saved');
	},

	setVideoSaved(video: TidalSearchVideo, saved: boolean): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>('/api/videos/saved', undefined, {
			method: 'POST', body: JSON.stringify({ video, saved }),
		});
	},

	/** The liked-videos wall. Pure reads over what the background resolve has
	 *  found so far, so this never waits on TIDAL. */
	getLikedVideos(): Promise<LikedVideosResponse> {
		return fetchApi<LikedVideosResponse>('/api/videos/liked');
	},

	/** Manual kick for the background resolve. A no-op when a pass is already
	 *  running or nothing is due, so it is safe to press repeatedly. */
	refreshLikedVideos(): Promise<{ running: boolean }> {
		return fetchApi<{ running: boolean }>('/api/videos/liked/refresh', undefined, {
			method: 'POST',
		});
	},

	/** "Wrong match / hide this" - matching is loose on purpose, so a card
	 *  occasionally lands on the wrong song. Also stops the periodic re-check
	 *  from bringing it back.
	 *
	 *  Takes the card's whole set of liked rows: a song favorited twice draws one
	 *  card, and suppressing half of it would just redraw from the other half. */
	hideLikedVideo(trackIds: number[], tidalVideoId: number): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>('/api/videos/liked/hide', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_ids: trackIds, tidal_video_id: tidalVideoId }),
		});
	},


	getTidalArtistProfile(tidalArtistId: number, preview = false): Promise<TidalArtistProfile> {
		return fetchApi<TidalArtistProfile>(`/api/tidal/artists/${tidalArtistId}${preview ? '?preview=true' : ''}`);
	},

	getTidalArtistReleasePage(tidalArtistId: number, filter: ArtistReleaseFilter, offset: number): Promise<TidalArtistReleasePage> {
		return fetchApi<TidalArtistReleasePage>(`/api/tidal/artists/${tidalArtistId}/releases?filter=${filter}&offset=${offset}`);
	},

	getTidalArtistCore(tidalArtistId: number): Promise<TidalArtistCore> {
		return fetchApi<TidalArtistCore>(`/api/tidal/artists/${tidalArtistId}/core`);
	},

	startSongRadioFromTidal(tidalId: number): Promise<RadioResponse> {
		return fetchApi<RadioResponse>('/api/discovery/radio', undefined, {
			method: 'POST',
			body: JSON.stringify({ seed_tidal_id: tidalId }),
		});
	},

	listAudioDevices(): Promise<{ devices: AudioDevice[] }> {
		return fetchApi<{ devices: AudioDevice[] }>('/api/audio/devices');
	},

	getAudioSettings(): Promise<AudioSettings> {
		return fetchApi<AudioSettings>('/api/audio/settings');
	},

	updateAudioSettings(settings: AudioSettings): Promise<AudioSettings> {
		return fetchApi<AudioSettings>('/api/audio/settings', undefined, {
			method: 'PUT',
			body: JSON.stringify(settings),
		});
	},

	retryAudioExclusive(): Promise<{ ok: boolean }> {
		return fetchApi<{ ok: boolean }>('/api/audio/exclusive/retry', undefined, {
			method: 'POST',
		});
	},

	ping() {
		return fetch(`${getApiBase()}/api/ping`).then((r) => r.ok).catch(() => false);
	},

	getServerToken() {
		return fetchApi<{ token: string }>('/api/server/token');
	},

	getDatabaseStats() {
		return fetchApi<DatabaseStats>('/api/server/database/stats');
	},

	// Rewrites the whole database file. Slow by nature (minutes on a large
	// library) and needs roughly the file's size in free space, so it is only
	// ever called from an explicit user action.
	compactDatabase() {
		return fetchApi<{
			status: string;
			before_bytes: number;
			after_bytes: number;
			reclaimed_bytes: number;
		}>('/api/server/database/compact', undefined, {
			method: 'POST',
			timeoutMs: COMPACT_DATABASE_TIMEOUT_MS,
		});
	},

	regenerateServerToken() {
		return fetchApi<{ token: string }>('/api/server/token/regenerate', undefined, {
			method: 'POST',
		});
	},

	getVibeTracksForTrack(trackId: number) {
		return fetchApi<{ tracks: VibeTrack[] }>(`/api/search/vibe?track_id=${trackId}`);
	},

	getUnderratedTracksForArtist(artistId: number) {
		return fetchApi<{ tracks: BasicTrack[] }>(`/api/search/underrated?artist_id=${artistId}`);
	},
};
