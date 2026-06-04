const API_BASE = 'http://localhost:3334';
export const DEFAULT_API_TIMEOUT_MS = 20_000;
export const BULK_QUEUE_API_TIMEOUT_MS = 90_000;

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

	const { protocol, hostname } = window.location;
	return `${protocol}//${hostname}:3334`;
}

// ─── Token management ────────────────────────────────────────────────────────

const TOKEN_KEY = 'noor_api_token';

export function getStoredToken(): string | null {
	if (typeof localStorage === 'undefined') return null;
	return localStorage.getItem(TOKEN_KEY);
}

export function setStoredToken(token: string): void {
	localStorage.setItem(TOKEN_KEY, token);
}

export function clearStoredToken(): void {
	localStorage.removeItem(TOKEN_KEY);
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

export interface Track {
	id: number;
	title: string;
	artist_id: number;
	artist_name: string | null;
	artist_tidal_id?: number | null;
	album_id: number | null;
	album_title: string | null;
	album_tidal_id?: number | null;
	disc_number: number | null;
	track_number: number | null;
	duration_ms: number | null;
	isrc: string | null;
	tidal_id: number | null;
	best_quality: string | null;
	best_source: string | null;
	fidelity_score: number;
	is_favorite: boolean;
	play_count: number;
	last_played_at: string | null;
	date_added: string | null;
	source: string;
	artwork_url: string | null;
	bpm?: number | null;
	key_signature?: string | null;
	camelot_key?: string | null;
	energy?: number | null;
	danceability?: number | null;
	is_instrumental?: boolean | null;
	samples_analyzed?: number | null;
}

export interface SpotifyTopCity {
	city: string;
	country: string;
	listeners: number;
}

export interface SpotifyArtistStats {
	monthly_listeners: number | null;
	followers?: number | null;
	world_rank?: number | null;
	top_cities?: SpotifyTopCity[];
	tracks: { isrc: string; title: string; playcount: number | null }[];
}

export type SpotifyTrackStats = SpotifyArtistStats;

export interface TidalDiscographyAlbum {
	tidal_id: number;
	local_id: number | null;
	title: string;
	artwork_url: string | null;
	release_date: string | null;
	release_type: string | null;
	// TIDAL's editorial filter that surfaced this album. More reliable than
	// release_type for bucketing - release_type on the body field disagrees
	// with the filter often enough to leave whole sections empty.
	source_filter: 'ALBUMS' | 'EPSANDSINGLES' | 'COMPILATIONS' | 'LIVE' | null;
	number_of_tracks: number | null;
	artist_name: string;
	in_library: boolean;
}

export interface TidalDiscographyTrack {
	tidal_id: number;
	title: string;
	duration_ms: number;
	artwork_url: string | null;
	album_title: string | null;
	album_tidal_id?: number | null;
	track_number?: number | null;
	disc_number?: number | null;
	artist_name?: string | null;
	artist_tidal_id?: number | null;
	track_id?: number;
	is_in_library?: boolean;
	is_favorite?: boolean;
}

export interface TidalArtistVideo {
	tidal_id: number;
	title: string;
	duration_ms: number;
	artwork_url: string | null;
	artist_name: string | null;
	album_tidal_id?: number | null;
}

export interface TidalSimilarArtist {
	tidal_id: number;
	local_id: number | null;
	name: string;
	artwork_url: string | null;
	in_library: boolean;
}

export interface TidalArtistBio {
	summary: string | null;
	text: string | null;
	source: string | null;
}

export interface TidalSearchTrack {
	tidal_id: number;
	title: string;
	duration_ms: number;
	artist_id: number | null;
	artist_name: string | null;
	album_title: string | null;
	album_tidal_id: number | null;
	artwork_url: string | null;
	audio_quality: string | null;
	stream_ready: boolean | null;
	local_id?: number | null;
	in_library: boolean;
}

export interface TidalSearchAlbum {
	tidal_id: number;
	title: string;
	artist_name: string | null;
	artwork_url: string | null;
	local_id: number | null;
	in_library: boolean;
}

export interface TidalSearchArtist {
	tidal_id: number;
	name: string;
	artwork_url: string | null;
	local_id: number | null;
	in_library: boolean;
}

export interface TidalSearchVideo {
	tidal_id: number;
	title: string;
	duration_ms: number | null;
	artist_id: number | null;
	artist_name: string | null;
	album_tidal_id: number | null;
	artwork_url: string | null;
	quality: string | null;
	explicit: boolean | null;
	type: string;
}

export interface TidalSearchPlaylist {
	uuid: string;
	title: string;
	description: string | null;
	number_of_tracks: number | null;
	artwork_url: string | null;
}

export interface TidalSearchResults {
	tracks: TidalSearchTrack[];
	albums: TidalSearchAlbum[];
	artists: TidalSearchArtist[];
	videos?: TidalSearchVideo[];
}

export interface TidalVideoStream {
	hls_url: string;
	expires_at: string | null;
	quality: string;
}

export interface TidalVideoMix {
	id: string | number;
	title: string;
	artwork_url?: string | null;
	description?: string | null;
	type: 'mix';
}

export type TidalVideoMixItem = TidalSearchVideo & {
	mix_id?: string | number | null;
};

/**
 * Compact Spotify-playlist search result. Powers the Spotify section of
 * /search and Ctrl+K. Click navigates to the ephemeral /spotify-playlist/{id}
 * view, which fetches the full track listing and bulk-resolves to TIDAL.
 */
export interface SpotifyPlaylistSearchItem {
	spotifyId: string;
	title: string | null;
	description: string | null;
	thumbnail: string | null;
	owner: string | null;
	followers: number | null;
	totalTracks: number | null;
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

function normalizeSpotifyPlaylistDetail(raw: unknown): SpotifyPlaylistDetail {
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
		tracks: pickArray(item, ['tracks', 'items'])
			.map(normalizeSpotifyPlaylistTrack)
			.filter((track): track is SpotifyPlaylistTrack => track !== null),
	};
}

function normalizeSpotifyTrackSearchItem(raw: unknown): SpotifyTrackSearchItem | null {
	const item = asRecord(raw);
	if (!item) return null;
	const spotifyId = pickString(item, ['spotifyId', 'spotify_id', 'id']);
	if (!spotifyId) return null;
	return {
		spotifyId,
		title: pickString(item, ['title', 'name']),
		primaryArtist: pickString(item, ['primaryArtist', 'primary_artist', 'artist']),
		album: pickString(item, ['album', 'album_title', 'albumTitle']),
		thumbnail: pickString(item, ['thumbnail', 'image_url', 'imageUrl', 'artwork_url', 'artworkUrl', 'cover']),
		durationMs: pickNumber(item, ['durationMs', 'duration_ms']),
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

export interface SpotifyTidalState {
	status: 'pending' | 'resolved' | 'low_confidence' | 'unresolved' | 'error';
	id: number | null;
	confidence: number;
	matchReason: string | null;
	fromCache: boolean;
}

export interface SpotifyPlaylistTrack {
	source: 'spotify';
	spotifyId: string | null;
	type: 'track';
	title: string | null;
	primaryArtist: string | null;
	artists: { id: string | null; name: string | null }[];
	album: string | null;
	albumId: string | null;
	thumbnail: string | null;
	durationMs: number | null;
	releaseDate: string | null;
	explicit: boolean | null;
	trackNumber: number | null;
	discNumber: number | null;
	spotifyUrl: string | null;
	previewUrl: string | null;
	playcount: number | null;
	popularity: number | null;
	isrc: string | null;
	tidal: SpotifyTidalState;
}

export interface SpotifyPlaylistDetail {
	source: 'spotify';
	spotifyId: string | null;
	type: 'playlist';
	title: string | null;
	description: string | null;
	thumbnail: string | null;
	owner: string | null;
	followers: number | null;
	totalTracks: number | null;
	snapshotId: string | null;
	tracks: SpotifyPlaylistTrack[];
}

export interface SpotifyTrackDetail {
	source: 'spotify';
	spotifyId: string | null;
	type: 'track';
	title: string | null;
	primaryArtist: string | null;
	artists: { id: string | null; name: string | null }[];
	album: string | null;
	albumId: string | null;
	thumbnail: string | null;
	durationMs: number | null;
	releaseDate: string | null;
	explicit: boolean | null;
	trackNumber: number | null;
	discNumber: number | null;
	spotifyUrl: string | null;
	previewUrl: string | null;
	playcount: number | null;
	popularity: number | null;
	isrc: string | null;
	tidal: SpotifyTidalState;
}

export interface SpotifyAlbumDetail {
	source: 'spotify';
	spotifyId: string | null;
	type: 'album';
	title: string | null;
	primaryArtist: string | null;
	artists: { id: string | null; name: string | null }[];
	thumbnail: string | null;
	releaseDate: string | null;
	totalTracks: number | null;
	albumType: string | null;
	label: string | null;
	genres: string[];
	spotifyUrl: string | null;
	tracks: SpotifyPlaylistTrack[];
}

export interface SpotifyArtistDetail {
	source: 'spotify';
	spotifyId: string | null;
	type: 'artist';
	name: string | null;
	thumbnail: string | null;
	genres: string[];
	popularity: number | null;
	monthlyListeners: number | null;
	followers: number | null;
	worldRank: number | null;
	biography: string | null;
}

export interface SpotifyTrackSearchItem {
	spotifyId: string;
	title: string | null;
	primaryArtist: string | null;
	album: string | null;
	thumbnail: string | null;
	durationMs: number | null;
}

export interface SpotifyAlbumSearchItem {
	spotifyId: string;
	title: string | null;
	primaryArtist: string | null;
	thumbnail: string | null;
	releaseDate: string | null;
}

export interface SpotifyArtistSearchItem {
	spotifyId: string;
	name: string | null;
	thumbnail: string | null;
	followers: number | null;
}

export interface SpotifyArtistRelated {
	spotifyId: string;
	topTracks: SpotifyPlaylistTrack[];
	deepCuts: SpotifyPlaylistTrack[];
	recentReleases: SpotifyAlbumSearchItem[];
	similarArtists: SpotifyArtistSearchItem[];
	pendingSpotifyIds: string[];
}

export interface SpotifyAlbumRelated {
	spotifyId: string;
	moreFromArtist: SpotifyPlaylistTrack[];
	moreAlbumsByArtist: SpotifyAlbumSearchItem[];
	pendingSpotifyIds: string[];
}

export interface SpotifyTrackRelated {
	spotifyId: string;
	moreFromAlbum: SpotifyPlaylistTrack[];
	moreFromArtist: SpotifyPlaylistTrack[];
	pendingSpotifyIds: string[];
}

export interface ResolveStatusEntry {
	spotifyId: string;
	tidal: SpotifyTidalState;
}

export interface TidalArtistProfile {
	artist_name: string | null;
	picture_url: string | null;
	top_tracks: TidalDiscographyTrack[];
	albums: TidalDiscographyAlbum[];
}

/** Minimal shape accepted by all ephemeral Tidal play functions */
export interface TidalPlayable {
	tidal_id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	artist_tidal_id?: number | null;
	album_tidal_id?: number | null;
	track_id?: number;
	local_id?: number | null;
	is_in_library?: boolean;
	is_favorite?: boolean;
}

/** Phase 5 - entry returned by `GET /api/charts`.
 *
 * Either `local_track` (when the chart entry resolved to a library track) or
 * `tidal_playable` (when it didn't) is set; the frontend picks the row
 * component accordingly. `image_url` is a fallback artwork preview from the
 * source API and is only useful when neither resolution gave us artwork.
 */
export interface QueueExternalRequest {
	kind: 'library' | 'tidal' | 'external';
	track_id?: number;
	tidal_id?: number;
	artist: string;
	title: string;
	album_title?: string | null;
	artist_tidal_id?: number | null;
	album_tidal_id?: number | null;
	duration_ms?: number | null;
}

export interface ChartEntry {
	local_track: Track | null;
	tidal_playable: TidalPlayable | null;
	image_url: string | null;
	source: 'lastfm' | 'tidal';
	genre: string | null;
	entity_type?: 'track' | 'artist' | 'tag' | string;
	display_title?: string | null;
	display_subtitle?: string | null;
	metric_label?: string | null;
}

export type TrendingSource = 'lastfm' | 'tidal';
export type LastfmChartKind = 'tracks' | 'artists' | 'tags';

export interface ChartSnapshotSummary {
	id: number;
	source_key: string;
	region: string;
	period: string;
	chart_date: string;
	fetched_at: number;
	status: string;
}

export interface ChartSnapshotEntry {
	id: number;
	rank: number;
	rank_delta: number | null;
	artist: string;
	title: string;
	entity_type: 'track' | 'album' | 'artist' | 'video' | string;
	album: string | null;
	artwork_url: string | null;
	external_track_id: string | null;
	external_artist_id: string | null;
	external_video_id: string | null;
	external_url: string | null;
	streams: number | null;
	stream_delta: number | null;
	views: number | null;
	likes: number | null;
	audience: number | null;
	audience_delta: number | null;
	points: number | null;
	points_delta: number | null;
	seven_day_streams: number | null;
	total_streams: number | null;
	days_on_chart: number | null;
	peak_rank: number | null;
	provider_positions_json: unknown | null;
	raw_json: unknown | null;
	external_candidate_id: number | null;
	local_track_id: number | null;
	tidal_id: number | null;
	resolution_status: 'local' | 'tidal' | 'pending' | 'unresolved' | 'not_playable' | string;
	resolution_score: number | null;
}

export interface ChartSnapshotResponse {
	source: string;
	period: string;
	region: string;
	limit: number;
	snapshot: ChartSnapshotSummary | null;
	entries: ChartSnapshotEntry[];
}

export interface ChartMatrixProvider {
	source_key: string;
	label: string;
}

export interface ChartMatrixCell {
	snapshot_id: number;
	entry_id: number;
	source_key: string;
	region: string;
	chart_date: string;
	rank: number;
	rank_delta: number | null;
	artist: string;
	title: string;
	entity_type: string;
	artwork_url: string | null;
	streams: number | null;
	views: number | null;
	points: number | null;
	external_url: string | null;
	tidal_id: number | null;
	resolution_status: string;
}

export interface ChartMatrixRow {
	region: string;
	cells: Record<string, ChartMatrixCell | null>;
}

export interface ChartMatrixResponse {
	region_group: string;
	period: string;
	providers: ChartMatrixProvider[];
	rows: ChartMatrixRow[];
}

export interface ChartMatrixRefreshResponse {
	source: string;
	chart_date: string;
	fetched_at: number;
	report: {
		rows_seen: number;
		entries_written: number;
		snapshots_written: number;
	};
}

export interface LastfmGenre {
	key: string;
	label: string;
}

export interface LastfmCountry {
	code: string;
	label: string;
}

export interface Album {
	id: number;
	tidal_id: number | null;
	title: string;
	artist_id: number;
	artist_name: string | null;
	year: number | null;
	artwork_url: string | null;
	release_type: string | null;
	track_count: number | null;
	source: string;
}

export interface Artist {
	id: number;
	tidal_id: number | null;
	name: string;
	biography: string | null;
	photo_url: string | null;
}

export interface Genre {
	id: number;
	name: string;
	slug: string;
	parent_id: number | null;
	children: Genre[];
	track_count: number | null;
}

export interface Playlist {
	id: number;
	tidal_uuid: string | null;
	name: string;
	description: string | null;
	is_smart: boolean;
	track_count: number;
	smart_rules?: string | null;
	is_favorite: boolean;
	created_at: string;
	updated_at: string;
}

// ─── Smart Playlist Rule Types ───────────────────────────────────────────────

export type LogicOp = 'AND' | 'OR';
export type NumberOp = 'eq' | 'gte' | 'lte' | 'gt' | 'lt' | 'between_inclusive';
export type QualityTier = 'lossy' | 'lossless' | 'hi_res';
export type DateField = 'date_added' | 'last_played_at';

export type SampleDataSource = 'acrcloud' | 'fingerprint';

export type RuleClause =
	| { type: 'group'; op: LogicOp; clauses: RuleClause[] }
	| { type: 'genre'; names: string[]; match_descendants: boolean }
	| { type: 'artist'; names: string[] }
	| { type: 'date_range'; field: DateField; range: { start: string | null; end: string | null } }
	| { type: 'play_count'; op: NumberOp; value: number; value_max?: number | null }
	| { type: 'quality'; minimum: QualityTier }
	| { type: 'not_in_playlist'; playlist_ids: number[] }
	| { type: 'bpm_range'; min: number | null; max: number | null }
	| { type: 'key_signature'; key: string }
	| { type: 'camelot_key'; key: string }
	| { type: 'energy_range'; min: number | null; max: number | null }
	| { type: 'danceability_range'; min: number | null; max: number | null }
	| { type: 'instrumental_only'; is_instrumental: boolean }
	| { type: 'has_sample_data'; source: SampleDataSource | null };

export interface SmartPlaylistDefinition {
	name: string;
	description?: string | null;
	root: RuleClause;
}

export interface SearchResults {
	tracks: Track[];
	albums: Album[];
	artists: Artist[];
}

export interface QueueItem {
	id: number;
	position: number;
	source: string;
	track: Track;
	/**
	 * Per-row provenance string. Radio writes a structured "why is this
	 * here" reason on insert; automix and manual paths leave it null
	 * until those producers migrate. Format: an optional human prefix
	 * followed by " | " and a JSON suffix the frontend tooltip parses
	 * (see `parseReason` in $lib/utils/reason).
	 */
	reason?: string | null;
	/** Phase 2c-ii-a: true while track_id is not yet resolved to a Tidal match. */
	is_pending?: boolean;
}

/** A last.fm-sourced radio candidate that has no library track yet. */
export interface PendingCandidateInfo {
	artist: string;
	title: string;
	duration_ms?: number | null;
	lastfm_match_score: number;
	reason?: string | null;
}

export interface PlaybackState {
	current_track: Track | null;
	/** Set even when current_track is null (pending row playing). */
	current_queue_item_id?: number | null;
	position_ms: number;
	is_playing: boolean;
	volume: number;
	shuffle_mode: 'off' | 'true' | 'weighted' | 'genre';
	repeat_mode: 'off' | 'all' | 'one';
	automix_enabled: boolean;
	crossfade_ms: number;
	automix_discover_new: boolean;
	automix_use_learning: boolean;
	automix_allow_external: boolean;
	/**
	 * How many ms of the currently-playing track are decoded into the
	 * playback buffer. Optional because the backend serializes it with
	 * `#[serde(default)]` and older JSON payloads may omit it. Read sites
	 * should `?? 0` and treat missing as "unknown / no buffer info yet".
	 */
	buffered_ms?: number;
	/**
	 * Track-time offset (ms) where the audibly-current engine's decoded
	 * audio begins. 0 for a fresh-from-start engine; non-zero only after a
	 * true DASH segment-seek restart (option C). Optional / `#[serde(default)]`
	 * on the backend; read sites should `?? 0`.
	 */
	buffered_start_ms?: number;
}

export interface PlaybackSnapshot {
	state: PlaybackState;
	queue: QueueItem[];
	shuffle_debug?: ShuffleDebug | null;
}

export interface ShuffleDebug {
	mode: PlaybackState['shuffle_mode'];
	seed: number;
	scope: string;
	locked_count: number;
	candidate_count: number;
}

export interface TidalMixPlaybackResponse {
	ok: boolean;
	first_tidal_id?: number;
	shuffle_debug?: ShuffleDebug | null;
}

export interface PlaybackRuntimeInfo {
	device_name: string;
	sample_rate: number;
	channels: number;
	active_track_id: number | null;
	last_error: string | null;
	exclusive_engaged: boolean;
	exclusive_transport_format: string | null;
	dj_engine_enabled: boolean;
}

export type DjEnabledResponse = { enabled: boolean };

export type DjTransitionSpeedBias = 'slower' | 'neutral' | 'faster';

export type DjProfileResponse = {
	track_id: number;
	profile_version: string;
	beat_count: number;
	downbeat_count: number;
	phrase_count: number;
};

export type DjMixIntent = 'safe' | 'balanced' | 'bold';

export type DjMixIntentResponse = {
	intent: DjMixIntent;
};

export type DjPolicyResponse = {
	mix_intent: DjMixIntent;
	transition_speed_bias: DjTransitionSpeedBias;
};

export type DjDeckStatus = {
	media_ref_kind: string;
	media_ref_id: string;
	title: string;
	artist?: string;
	profile_ready: boolean;
	profile_status: 'ready' | 'missing' | 'analyzing' | 'retrying' | 'decode_failed' | string;
	profile_error?: string;
	profile_retry_after_ms?: number;
	profile_retry_reason?: string;
	profile_confidence?: number;
	beat_count?: number;
	downbeat_count?: number;
	phrase_count?: number;
	waveform_status: 'ready' | 'missing' | 'analyzing' | string;
	waveform_peaks: number[];
	beat_markers_ms: number[];
	downbeat_markers_ms: number[];
	phrase_markers_ms: number[];
	drop_markers_ms: number[];
	manual_drop_markers_ms: number[];
	mix_in_markers_ms: number[];
	mix_out_markers_ms: number[];
	passive_analysis_status?: 'ready' | 'missing' | 'retrying' | 'skipped' | string;
	passive_analysis_reason?: string;
	safe_crossfade_only: boolean;
};

export type DjDropPreviewStatus = {
	status: 'armed' | 'fired' | 'skipped' | string;
	planned_fire_ms?: number;
	actual_fire_ms?: number;
	incoming_drop_ms?: number;
	source?: 'manual' | 'profile' | string;
	reason?: string;
};

export type DjOverlayDetails = {
	overlay_status: string;
	overlay_start_ms?: number;
	overlay_end_ms?: number;
	tempo_ratio?: number;
	deck_b_start_frame: number;
	drop_marker_ms?: number;
	drop_source: 'program_json' | string;
};

export type DjRuntimeRendererStatus =
	| 'rendered_handoff'
	| 'rendered_overlay'
	| 'legacy_overlap'
	| 'boundary_fallback'
	| string;

export type DjRuntimeRendererReason =
	| 'none'
	| 'prepared_mixer_missing'
	| 'lookahead_pair_mismatch'
	| 'program_not_mixer_renderable'
	| 'active_deck_not_decoded'
	| 'next_deck_not_decoded'
	| 'mixer_rejected'
	| 'active_track_changed'
	| 'next_track_changed'
	| 'render_buffer_failed'
	| 'buffer_lock_failed'
	| 'dj_disabled'
	| 'next_decode_late_at_fire'
	| 'next_deck_missing_at_fire'
	| 'transition_plan_missing_at_fire'
	| 'sync_window_not_signaled'
	| 'manual_seek_suppressed'
	| string;

export type DjStatusResponse = {
	enabled: boolean;
	current?: DjDeckStatus;
	next?: DjDeckStatus;
	planning_status:
		| 'disabled'
		| 'pair_missing'
		| 'waiting_for_profiles'
		| 'profile_failed'
		| 'waiting_for_window'
		| 'ready_to_plan'
		| 'armed'
		| 'missed'
		| string;
	selected_program?: string;
	planned_template?: string;
	renderer_template?: string;
	renderer_mode?: 'legacy_overlap' | 'dj_gain_program' | 'dj_full_program' | 'dj_overlay_program';
	downgrade_reason?: string;
	planning_reason?: string;
	sync_target?: string;
	planned_start_ms?: number;
	actual_start_ms?: number;
	timing_delta_ms?: number;
	timing_source?: string;
	timing_status?: string;
	timing_quality: 'tight' | 'usable' | 'loose' | 'bad' | 'unknown';
	timing_direction: 'on_time' | 'early' | 'late' | 'missed' | 'pending' | 'unknown';
	runtime_rendered_dj_mixer?: boolean;
	runtime_renderer_status?: DjRuntimeRendererStatus;
	runtime_renderer_reason?: DjRuntimeRendererReason;
	overlay_details?: DjOverlayDetails;
	fallback_reason?: string;
	rejected_alternatives: DjRejectedAlternative[];
	profile_confidence_floor: number;
	last_transition_event_id?: number;
	recent_timing_events: DjTimingHistoryEvent[];
	timing_history_summary: DjTimingHistorySummary;
	safe_crossfade_suggestion?: {
		media_ref_kind: string;
		media_ref_id: string;
		bad_feedback_count: number;
	};
	drop_preview: DjDropPreviewStatus;
};

export type DjTimingHistoryEvent = {
	event_id: number;
	from_title?: string;
	from_artist?: string;
	to_title?: string;
	to_artist?: string;
	planned_template: string;
	renderer_template?: string;
	planning_reason?: string;
	planned_start_ms?: number;
	actual_start_ms?: number;
	timing_delta_ms?: number;
	timing_source?: string;
	timing_status?: 'fired' | 'late' | 'missed';
	timing_quality: 'tight' | 'usable' | 'loose' | 'bad';
	timing_direction: 'on_time' | 'early' | 'late' | 'missed' | 'unknown';
	runtime_rendered_dj_mixer?: boolean;
	runtime_renderer_status?: DjRuntimeRendererStatus;
	runtime_renderer_reason?: DjRuntimeRendererReason;
	rejected_alternatives: DjRejectedAlternative[];
	started_at: string;
};

export type DjRejectedAlternative = {
	template: string;
	score: number;
	reason: string;
};

export type DjTimingHistorySummary = {
	event_count: number;
	average_delta_ms?: number;
	average_abs_delta_ms?: number;
	tight_count: number;
	usable_count: number;
	loose_count: number;
	bad_count: number;
	late_count: number;
	missed_count: number;
};

export type DjProfileCorrectionRequest = {
	media_ref_kind: string;
	media_ref_id: string;
	bpm_multiplier?: number;
	downbeat_offset_beats?: number;
	phrase_offset_bars?: number;
	safe_crossfade_only?: boolean;
	transition_speed_bias?: DjTransitionSpeedBias;
	manual_drop_markers_ms?: number[];
	notes?: string;
};

export type DjFeedbackRequest = {
	transition_event_id?: number;
	rating: 'good' | 'bad' | 'too_safe' | 'too_bold';
	reason?: string;
};
export interface StreamDisplayInfo {
	audio_quality: string;
	sample_rate: number | null;
	bit_depth: number | null;
}

export interface MusicBrainzStatus {
	total_tracks: number;
	checked_tracks: number;
	enriched_tracks: number;
	remaining: number;
	complete: boolean;
}

export interface PortableMusicBrainzSnapshotStatus {
	exists: boolean;
	path: string;
	generated_at: string | null;
	checked_rows: number;
	genre_rows: number;
	lastfm_checked_rows: number;
	context_tag_rows: number;
}

export interface PortableMusicBrainzSnapshotAction {
	status: 'exported' | 'imported';
	snapshot: PortableMusicBrainzSnapshotStatus;
	checked_inserted?: number;
	checked_skipped?: number;
	lastfm_checked_inserted?: number;
	lastfm_checked_skipped?: number;
	genre_inserted?: number;
	track_skipped?: number;
	genre_skipped?: number;
	context_tag_inserted?: number;
	context_tag_skipped?: number;
}

export interface TrackFavoriteResponse {
	track_id: number;
	tidal_id: number;
	favorite: boolean;
	updated: boolean;
}

export interface AnalyticsOverview {
	tracks: number;
	albums: number;
	artists: number;
	playlists: number;
	smart_playlists: number;
	tagged_tracks: number;
	total_listens: number;
	favorite_tracks: number;
}

export interface ListenHistoryEntry {
	id: number;
	track_id: number;
	track_title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	started_at: string;
	duration_listened_ms: number;
	completed: boolean;
}

export interface AnalyticsTopTrack {
	track_id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	listens: number;
	completed_listens: number;
	total_listened_ms: number;
	completion_rate?: number | null;
	share_of_window_listened_ms?: number | null;
	previous_rank?: number | null;
	rank_delta?: number | null;
}

export interface AnalyticsTopArtist {
	artist_id: number;
	artist_name: string;
	listens: number;
	completed_listens: number;
	unique_tracks: number;
	total_listened_ms: number;
	completion_rate?: number | null;
	share_of_window_listened_ms?: number | null;
	previous_rank?: number | null;
	rank_delta?: number | null;
}

export interface AnalyticsGenreShare {
	genre_name: string;
	listens: number;
	share_of_window_listens?: number | null;
}

export interface GenreHeat {
	genre_id: number;
	genre_name: string;
	listen_count: number;
	total_listened_ms: number;
}

export interface GenreCohort {
	id: string;
	label: string;
	icon: string;
	genre_ids: number[];
	listen_count: number;
	total_listened_ms: number;
}

export interface GenreEvolutionPoint {
	genre_id: number;
	genre_name: string;
	period_start: string;
	listen_count: number;
	total_listened_ms: number;
}

export interface AnalyticsActivityPoint {
	day: string;
	listens: number;
	completed_listens: number;
	listened_ms: number;
}

export interface AnalyticsBehavior {
	total_listened_ms: number;
	total_listens: number;
	completed_listens: number;
	skipped_listens: number;
	completion_rate: number;
	average_listen_ms: number;
	unique_tracks: number;
	repeat_track_count: number;
	active_days: number;
}

export interface AnalyticsDashboard {
	overview: AnalyticsOverview;
	recent_listens: ListenHistoryEntry[];
	top_tracks: AnalyticsTopTrack[];
	top_artists: AnalyticsTopArtist[];
	top_genres: AnalyticsGenreShare[];
	activity: AnalyticsActivityPoint[];
	behavior: AnalyticsBehavior;
}

// ─────────────────────────────────────────────────────────────────────────
// Analytics signals - GET /api/analytics/signals
// Contract: noor-server/tests/fixtures/signals-schema.json
// JSON schema: noor-server/tests/fixtures/signals-schema.json
// ─────────────────────────────────────────────────────────────────────────

export type SignalsGranularity = 'day' | 'week' | 'month';

export interface AnalyticsDisplayCaps {
	ridgeline_days?: number | null;
	tempo_rows?: number | null;
}

export interface SignalsWindow {
	days: number;
	started_at: string;
	previous_started_at: string;
	generated_at: string;
	granularity: SignalsGranularity;
	display_caps: AnalyticsDisplayCaps;
}

export interface AnalyticsTotals {
	listens: number;
	listened_ms: number;
	distinct_tracks: number;
	tagged_listens: number;
}

export interface KpiPairInt {
	current: number;
	previous: number;
}

export interface KpiPairFloat {
	current: number | null;
	previous: number | null;
}

export interface DailyKpi {
	day: string;
	listens: number;
	listened_ms: number;
	completed: number;
	sessions?: number;
}

export interface HeroStats {
	peak_hour: number | null;
	rhythm: number | null;
	night_share: number | null;
	morning_share: number | null;
	longest_session_ms?: number | null;
	distinct_tracks?: number | null;
}

export interface SessionsCoverage {
	tracked: number;
	untracked: number;
}

export interface SignalsKpis {
	listened_ms: KpiPairInt;
	sessions: KpiPairInt;
	completion: KpiPairFloat;
	skip_rate: KpiPairFloat;
	daily: DailyKpi[];
	hero_stats: HeroStats;
	sessions_coverage: SessionsCoverage;
}

export interface BucketAxis {
	min: number;
	max: number;
	step: number;
}

export interface BpmBucket {
	bucket: number;
	listens: number;
}

export interface TempoRow {
	label: string;
	granularity: SignalsGranularity;
	buckets: BpmBucket[];
}

export interface TempoStats {
	median: number | null;
	mode: number | null;
	sigma: number | null;
}

export interface Coverage {
	analyzed: number;
	total_listened: number;
}

export interface TempoView {
	bucket_axis: BucketAxis;
	rows: TempoRow[];
	stats: TempoStats;
	coverage: Coverage;
	ridge_amp_max: number;
}

export interface SonicTrack {
	track_id: number;
	title: string;
	artist_name: string | null;
	album: string | null;
	artwork_path: string | null;
	file_path: string | null;
	e: number;
	d: number;
	bpm: number;
	listens: number;
}

export interface SonicView {
	tracks: SonicTrack[];
	total: number;
	coverage: Coverage;
}

export interface RidgeRow {
	date: string;
	hourly: number[]; // length 24, zero-filled
}

export interface CohortRow {
	key: 'new_this_month' | 'established' | 'deep_cuts';
	label: string;
	tracks: number;
	listened_ms: number;
	sessions: number;
	completion: number | null;
	skip_rate: number | null;
	new_artists: number;
	repeat_rate: number | null;
}

export interface AudioProfile {
	dynamic_range_dr: number | null;
	loudness_lufs: number | null;
	bass_tilt: number | null;
	treble_tilt: number | null;
	coverage: Coverage;
	track_coverage?: Coverage;
	listen_coverage?: Coverage;
}

export interface AnalyticsSignals {
	window: SignalsWindow;
	totals: AnalyticsTotals;
	kpis: SignalsKpis;
	tempo: TempoView;
	sonic_field: SonicView;
	ridgeline: RidgeRow[];
	top_tracks: AnalyticsTopTrack[];
	top_artists: AnalyticsTopArtist[];
	top_genres: AnalyticsGenreShare[];
	cohorts: CohortRow[];
	audio_profile: AudioProfile;
}

export interface VibeTrack {
	id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	bpm: number | null;
	camelot_key: string | null;
}

export interface BasicTrack {
	id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
}

export interface DiscoveryPreset {
	id: number;
	name: string;
	prompt: string;
	mode: DiscoveryMode;
	services: string[];
	created_at: string;
}

export interface DiscoveryProfilePreview {
	prompt: string;
	mode: string;
	services: string[];
	prompt_terms: string[];
	prompt_genres: string[];
	top_artists: string[];
	top_genres: string[];
	recent_tracks: string[];
	favorite_ratio: number;
	completion_rate: number;
	summary: string;
}

export interface DiscoveryReason {
	label: string;
	detail: string;
	weight: number;
}

export interface DiscoveryPreviewResult {
	track_id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	service: string;
	service_track_id: string;
	score: number;
	tags: string[];
}

export interface DiscoveryPreview {
	profile: DiscoveryProfilePreview;
	reasons: DiscoveryReason[];
	results: DiscoveryPreviewResult[];
}

export type DiscoveryMode = 'mood' | 'reference' | 'dj' | 'word-cloud';
export type DiscoveryService = 'tidal' | 'ytmusic' | 'soundcloud' | 'bandcamp';

export interface DiscoveryProviderCapability {
	provider: string;
	can_save: boolean;
	can_play_inline: boolean;
	can_fetch_connections: boolean;
	can_map_genres: boolean;
}

export interface DiscoveryExternalResult {
	provider: string;
	provider_track_id: string;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	audio_quality: string | null;
	normalized_genres: string[];
	lastfm_tags: string[];
	lastfm_similarity_score: number | null;
	discogs_genres: string[];
	discogs_styles: string[];
	discogs_label: string | null;
	discogs_year: number | null;
	discogs_confidence: number | null;
	in_library: boolean;
	is_saved: boolean;
	is_playable: boolean;
	embedding_score: number | null;
	score: number;
	tags: string[];
}

export interface DiscoveryConnectionTrailItem {
	provider: string;
	provider_track_id: string;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	normalized_genres: string[];
	connection_reason: string;
}

export interface DiscoveryExternalFeed {
	profile: DiscoveryProfilePreview;
	reasons: DiscoveryReason[];
	results: DiscoveryExternalResult[];
	capabilities: DiscoveryProviderCapability[];
	trail_item: DiscoveryConnectionTrailItem | null;
}

export interface DiscoveryNeighborReason {
	key: string;
	label: string;
	weight: number;
}

export interface EmbeddingModel {
	id: number;
	model_key: string;
	family: string;
	dimension: number;
	status: string;
	is_active: boolean;
	trained_at: string | null;
	config_json: string | null;
	metrics_json: string | null;
	created_at: string;
}

export interface DiscoveryTrainingRun {
	id: number;
	model_id: number | null;
	stage: string;
	status: string;
	progress: number;
	items_total: number | null;
	items_done: number;
	started_at: string;
	finished_at: string | null;
	error_text: string | null;
}

export interface DiscoveryStatus {
	fallback_active: boolean;
	active_model: EmbeddingModel | null;
	selected_engine: DiscoveryEngine;
	selected_engine_family: string;
	selected_engine_trainable: boolean;
	latest_run: DiscoveryTrainingRun | null;
	coverage_ratio: number;
	playable_tracks: number;
	embedded_tracks: number;
	neighbor_tracks: number;
	clip_cache_tracks: number;
}

export type DiscoveryEngine = 'v2' | 'v1';
export type DiscoveryTrainingSafetyProfile = 'laptop_safe' | 'balanced' | 'performance';

export interface DiscoveryRadioResult {
	track_id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	best_quality: string | null;
	similarity_score: number;
	adjusted_score: number;
	co_listen_score: number;
	co_album_score: number;
	co_artist_score: number;
	genre_proximity: number;
	reason_tags: string[];
	model_key: string | null;
	source_mode: string;
}

export interface RadioResponse {
	tracks: DiscoveryRadioResult[];
	seed_track_id: number;
	creativity: number;
	context_window: number;
	computed_at: string | null;
	model_family: string | null;
	model_key: string | null;
	reasons: string[];
}

export type RadioBlend = 'familiar' | 'mixed' | 'adventurous';
export type RadioSource = 'library' | 'lastfm' | 'engine';

export interface RadioCandidate {
	track_id: number;
	tidal_track_id: number | null;
	title: string;
	artist_name: string;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	isrc: string | null;
	is_in_library: boolean;
	source: RadioSource;
	reason: string;
	similarity_score: number;
}

export interface RadioQueue {
	session_id: string;
	blend_used: RadioBlend;
	seed: {
		kind: 'track' | 'album' | 'artist';
		track_id: number | null;
		album_id: number | null;
		artist_id: number | null;
		title: string;
		artist_name: string | null;
	};
	tracks: RadioCandidate[];
	state?: PlaybackState;
	queue?: QueueItem[];
	first_playable?: {
		type: 'library' | 'pending';
		queue_item_id: number;
		track_id: number | null;
	};
	pending_count?: number;
}

// ─── Home Page Discovery Types ───────────────────────────────────────────────

export interface RSSFeedItem {
	title: string;
	link: string;
	description: string;
	author: string | null;
	published_at: string | null;
	image_url: string | null;
	source: string;
	category: string;
}

/// Last.fm-API-sourced new release. Different shape than `RSSFeedItem`:
/// no description/category (Last.fm doesn't supply them).
export interface ReleaseItem {
	title: string;
	link: string;
	author: string;
	image_url: string | null;
	source: string;
	published_at: string | null;
}

export interface HomeReleasesResponse {
	releases: ReleaseItem[];
	source: string;
}

export interface TidalMix {
	id: string;
	title: string;
	sub_title?: string | null;
	image_url?: string | null;
	mix_type?: string | null;
	is_video_mix: boolean;
}

export interface TidalMixesResponse {
	mixes: TidalMix[];
	source: string;
}

export interface TidalRadioStationsResponse {
	stations: TidalMix[];
	source: string;
}

/** One item inside a TIDAL home discover module. Per-kind fields are optional -
 *  the frontend dispatches on `kind` to pick the right shelf renderer. */
export interface TidalHomeItem {
	kind: 'track' | 'album' | 'playlist';
	id: string;
	title: string;
	artist_name?: string | null;
	artwork_url?: string | null;
	duration?: number | null;        // tracks only (seconds)
	artist_id?: number | null;
	album_id?: number | null;
	album_title?: string | null;
	creator_name?: string | null;    // playlists only
}

export interface TidalHomeModule {
	id: string;
	title: string;
	kind: string;                     // TRACK_LIST | ALBUM_LIST | PLAYLIST_LIST | MIXED_TYPES_LIST | …
	more_path?: string | null;        // upstream `pagedList.dataApiPath` - used by per-module detail route
	items: TidalHomeItem[];
}

export interface TidalHomeModulesResponse {
	modules: TidalHomeModule[];
	source: string;
}

export interface TidalMoodCategory {
	slug: string;
	title: string;
	icon: string | null;
	imageId: string | null;
	thumbnail: string | null;
}

export interface TidalDiscoverModuleResponse {
	module: TidalHomeModule;          // module returned without `more_path` (already resolved); `items` is the full set
	source: string;
}

export interface LastfmStatus {
	configured: boolean;
	enrichment: boolean;
	api_key_configured?: boolean;
	api_secret_configured?: boolean;
	scrobbling: boolean;
	scrobble_available: boolean;
	recommendations?: boolean;
	pending_submissions?: number;
	failed_submissions?: number;
	user: string | null;
}

export interface ListenBrainzStatus {
	configured: boolean;
	scrobbling: boolean;
	recommendations: boolean;
	pending_submissions: number;
	failed_submissions: number;
	user: string | null;
}

export interface ProviderRecommendationItem {
	provider: 'lastfm' | 'listenbrainz' | string;
	entity_type?: 'track' | 'artist' | 'album' | string;
	local_track_id: number | null;
	tidal_id: number | null;
	local_artist_id?: number | null;
	tidal_artist_id?: number | null;
	local_album_id?: number | null;
	tidal_album_id?: number | null;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	mbid?: string | null;
	score?: number | null;
	reason: string;
	playable: boolean;
}

export interface ProviderRecommendationShelf {
	provider: 'lastfm' | 'listenbrainz' | string;
	entity_type?: 'track' | 'artist' | 'album' | string;
	title: string;
	status: 'ok' | 'empty' | 'error' | string;
	message?: string;
	items: ProviderRecommendationItem[];
}

export interface HomeRecommendationsResponse {
	shelves: ProviderRecommendationShelf[];
}

export interface LastfmAuthStartResponse {
	status: 'awaiting' | 'error';
	auth_url?: string;
	message?: string;
}

export interface LastfmAuthCompleteResponse {
	status: 'connected' | 'not_yet_authorized' | 'error';
	user?: string;
	message?: string;
}

export interface HomePickTrack {
	id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	play_count: number;
	reason: string;
	genre?: string;
}

export interface HomePicksResponse {
	top_picks: HomePickTrack[];
	genre_variety: HomePickTrack[];
	source: string;
}

export interface HomeArticlesResponse {
	articles: RSSFeedItem[];
	source: string;
}

export interface HomeNewsResponse {
	news: RSSFeedItem[];
	sources: string[];
	source: string;
}

// ─── Audio Analysis Types ───────────────────────────────────────────────

export interface AudioDspFeatures {
	track_id: number;
	bpm: number | null;
	key_signature: string | null;
	camelot_key: string | null;
	loudness_lufs: number | null;
	energy: number | null;
	danceability: number | null;
	beat_strength: number | null;
	spectral_centroid: number | null;
	stereo_width: number | null;
	is_instrumental: boolean | null;
	analysis_source: string;
	analysis_offset_ms: number;
	samples_analyzed: number;
	analyzed_at: string;
	analysis_version: string;
}

export interface AudioSearchResult {
	id: number;
	title: string;
	artist_name: string | null;
	album_title: string | null;
	artwork_url: string | null;
	duration_ms: number | null;
	bpm: number | null;
	energy: number | null;
	danceability: number | null;
	key_signature: string | null;
	camelot_key: string | null;
	play_count: number;
	is_favorite: boolean;
	tidal_id: number | null;
	source: string;
}

export interface AudioSearchParams {
	free_text?: string;
	bpm_min?: number | null;
	bpm_max?: number | null;
	energy_min?: number | null;
	energy_max?: number | null;
	danceability_min?: number | null;
	danceability_max?: number | null;
	key_signature?: string | null;
	camelot_key?: string | null;
	year_min?: number | null;
	year_max?: number | null;
	genre_ids?: number[];
	is_instrumental?: boolean | null;
}

export interface AudioFeaturesStats {
	total_analyzed: number;
	avg_bpm: number | null;
	top_key: string | null;
	avg_energy: number | null;
	key_distribution: Record<string, number>;
}

export interface GenreAudioMetrics {
	genre_id: number;
	genre_name: string;
	avg_bpm: number | null;
	avg_energy: number | null;
	avg_danceability: number | null;
	analyzed_count: number;
}

export interface AcrCloudStatus {
	connected: boolean;
	scanned_today: number;
	daily_limit: number;
}

export interface AcrCloudScanStatus {
	running: boolean;
	scanned: number;
	total: number;
	matches_found: number;
}

export type AudioQuality = 'LOW' | 'HIGH' | 'LOSSLESS' | 'HI_RES_LOSSLESS';
export type VideoQualityMode = 'MAX' | 'AUTO';
export type ExclusiveLatencyMode = 'STABLE' | 'LOW_LATENCY' | 'ULTRA_LOW_LATENCY';

export interface AudioDevice {
	id: string;
	name: string;
	is_default: boolean;
	max_channels: number;
	supported_sample_rates: number[];
}

export interface AudioSettings {
	quality: AudioQuality;
	output_device: string | null;
	exclusive_mode: boolean;
	sample_rate_follow: boolean;
	video_quality_mode: VideoQualityMode;
	exclusive_latency_mode: ExclusiveLatencyMode;
	/** Seconds of paused state before WASAPI exclusive releases the device. Server clamps 5..=120. */
	exclusive_release_grace_secs: number;
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

	getAlbums(sortBy = 'title', sortDir = 'asc', limit = 50, offset = 0, favoriteOnly = true) {
		return fetchApi<{ albums: Album[]; total: number }>('/api/albums', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
			favorite_only: String(favoriteOnly),
		});
	},

	getAlbumTracks(id: number) {
		return fetchApi<{
			tracks: Track[];
			tidal_tracks: TidalDiscographyTrack[];
			album_tidal_id: number | null;
		}>(`/api/albums/${id}/tracks`);
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

	getArtistDiscography(id: number) {
		return fetchApi<{
			albums: TidalDiscographyAlbum[];
			top_tracks: TidalDiscographyTrack[];
			videos: TidalArtistVideo[];
			similar_artists: TidalSimilarArtist[];
			bio: TidalArtistBio | null;
			picture_url: string | null;
			available: boolean;
			reason?: string;
		}>(`/api/artists/${id}/discography`);
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

	searchTidalPlaylists(q: string, signal?: AbortSignal, opts?: { limit?: number; offset?: number }) {
		const limit = opts?.limit ?? 20;
		const offset = opts?.offset ?? 0;
		return fetchApi<{ playlists: TidalSearchPlaylist[] }>(
			`/api/tidal/playlists/search?q=${encodeURIComponent(q)}&limit=${limit}&offset=${offset}`,
			undefined,
			{ signal },
		);
	},

	/**
	 * Search Spotify (via the Sportify proxy) for playlists. Best-effort -
	 * the caller should swallow errors so a Sportify outage never breaks
	 * /search or Ctrl+K rendering.
	 */
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

	async searchSpotifyTracks(
		q: string,
		limit = 12,
		signal?: AbortSignal,
		offset = 0,
	): Promise<SpotifyTrackSearchItem[]> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/search`,
			{ q, type: 'track', limit: String(limit), offset: String(offset) },
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return pickArray(root, ['tracks'])
			.map(normalizeSpotifyTrackSearchItem)
			.filter((t): t is SpotifyTrackSearchItem => t !== null);
	},

	async searchSpotifyAlbums(
		q: string,
		limit = 12,
		signal?: AbortSignal,
		offset = 0,
	): Promise<SpotifyAlbumSearchItem[]> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/search`,
			{ q, type: 'album', limit: String(limit), offset: String(offset) },
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return pickArray(root, ['albums'])
			.map(normalizeSpotifyAlbumSearchItem)
			.filter((a): a is SpotifyAlbumSearchItem => a !== null);
	},

	async searchSpotifyArtists(
		q: string,
		limit = 12,
		signal?: AbortSignal,
		offset = 0,
	): Promise<SpotifyArtistSearchItem[]> {
		const raw = await fetchApi<unknown>(
			`/api/discovery/sportify/search`,
			{ q, type: 'artist', limit: String(limit), offset: String(offset) },
			{ signal },
		);
		const root = asRecord(raw) ?? {};
		return pickArray(root, ['artists'])
			.map(normalizeSpotifyArtistSearchItem)
			.filter((a): a is SpotifyArtistSearchItem => a !== null);
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

	async searchSpotifyPlaylists(
		q: string,
		limit = 12,
		signal?: AbortSignal,
		offset = 0,
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
				{ signal },
			);
			const playlists = fromResponse(res);
			if (playlists.length > 0 || offset > 0) return playlists;
		} catch (error) {
			if (signal?.aborted) throw error;
		}

		const fallback = await fetchApi<Resp>(
			'/api/search',
			{ q, limit: String(limit) },
			{ signal },
		);
		return fromResponse(fallback);
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
		return fetchApi<{ tracks: AudioSearchResult[] }>('/api/search/audio', undefined, {
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
		return fetchApi<{ state: PlaybackState; queue: QueueItem[] }>('/api/playback/automix', undefined, {
			method: 'POST',
			body: JSON.stringify({ enabled, crossfade_ms, discover_new, use_learning, allow_external }),
		});
	},

	addQueueTrack(trackId: number) {
		return fetchApi<{ queue: QueueItem[] }>('/api/playback/queue/add', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_id: trackId }),
		});
	},

	queuePlayNext(req: QueueExternalRequest) {
		return fetchApi<{ queue: QueueItem[] }>('/api/queue/play_next', undefined, {
			method: 'POST',
			body: JSON.stringify(req),
		});
	},

	queuePlayNextMany(items: QueueExternalRequest[]) {
		return fetchApi<{ queue: QueueItem[] }>('/api/queue/play_next_many', undefined, {
			method: 'POST',
			body: JSON.stringify({ items }),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
	},

	queueAppend(req: QueueExternalRequest) {
		return fetchApi<{ queue: QueueItem[] }>('/api/queue/append', undefined, {
			method: 'POST',
			body: JSON.stringify(req),
		});
	},

	queueAppendMany(items: QueueExternalRequest[]) {
		return fetchApi<{ queue: QueueItem[] }>('/api/queue/append_many', undefined, {
			method: 'POST',
			body: JSON.stringify({ items }),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
	},

	replacePlaybackQueue(
		trackIds: number[],
		reasons?: (string | null)[],
		pendingCandidates?: PendingCandidateInfo[],
		shuffleMode?: PlaybackState['shuffle_mode'],
	) {
		const body: Record<string, unknown> = { track_ids: trackIds };
		if (reasons) body.reasons = reasons;
		if (pendingCandidates?.length) body.pending_candidates = pendingCandidates;
		if (shuffleMode && shuffleMode !== 'off') body.shuffle_mode = shuffleMode;
		return fetchApi<{ queue: QueueItem[]; shuffle_debug?: ShuffleDebug | null }>(
			'/api/playback/queue',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify(body),
				timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
			}
		);
	},

	removeQueueTrack(queueItemId: number) {
		return fetchApi<{ queue: QueueItem[]; playback_state?: PlaybackState }>(
			'/api/playback/queue/remove',
			undefined,
			{
				method: 'POST',
				body: JSON.stringify({ queue_item_id: queueItemId }),
			}
		);
	},

	moveQueueTrack(itemId: number, newPos: number) {
		return fetchApi<{ queue: QueueItem[] }>('/api/playback/queue/move', undefined, {
			method: 'POST',
			body: JSON.stringify({ item_id: itemId, new_pos: newPos }),
		});
	},

	clearQueue() {
		return fetchApi<{ queue: QueueItem[]; playback_state?: PlaybackState }>('/api/playback/queue/clear', undefined, {
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

	// ─── TIDAL: Your Mixes ────────────────────────────────────────────────
	// 503 here means TIDAL isn't connected - the YourMixesShelf surfaces a
	// connect prompt rather than an error toast.
	getTidalMixes() {
		return fetchApi<TidalMixesResponse>('/api/tidal/mixes');
	},

	// Personal Radio Stations - same 503/connect-prompt contract as getTidalMixes.
	getTidalRadioStations() {
		return fetchApi<TidalRadioStationsResponse>('/api/tidal/radio-stations');
	},

	// Editorial home modules from TIDAL pages/home - drives the search-page
	// discover surface. 503 when TIDAL is disconnected.
	getTidalHomeModules() {
		return fetchApi<TidalHomeModulesResponse>('/api/tidal/home-modules');
	},

	// Generic editorial page fetch - drives /charts, /moods, and (eventually)
	// /genres / /new-releases. Backend whitelists the section + optional id.
	// Path is split on / so each segment is encoded individually (needed for
	// `mood/{id}` style two-segment paths).
	getTidalPage(path: string) {
		return fetchApi<TidalHomeModulesResponse>(
			`/api/tidal/page/${path.split('/').map(encodeURIComponent).join('/')}`,
		);
	},

	// TIDAL moods landing: returns the PAGE_LINKS category list (Party,
	// Workout, Focus, etc). Each entry has a slug that can be fed to
	// getTidalMoodPage for the drill-down content.
	getTidalMoods() {
		return fetchApi<{ categories: TidalMoodCategory[]; source: string; fallback?: boolean }>(
			'/api/tidal/moods',
		);
	},

	// Drill-down for one mood category. Backend proxies to pages/{slug} which
	// returns the standard editorial modules shape.
	getTidalMoodPage(slug: string) {
		return fetchApi<TidalHomeModulesResponse>(
			`/api/tidal/mood-page/${encodeURIComponent(slug)}`,
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
			`/api/tidal/mixes/${encodeURIComponent(mixId)}/tracks`
		);
	},

	// Play the first track of a mix immediately and queue the rest as
	// pending ephemeral tracks (server auto-advances on track-end).
	playTidalMix(tracks: TidalPlayable[], shuffleMode?: PlaybackState['shuffle_mode']) {
		const body: Record<string, unknown> = {
			tracks: tracks.map((t) => ({
				tidal_track_id: t.tidal_id,
				title: t.title,
				artist_name: t.artist_name ?? null,
				artist_tidal_id: t.artist_tidal_id ?? null,
				album_title: t.album_title ?? null,
				album_tidal_id: t.album_tidal_id ?? null,
				artwork_url: t.artwork_url ?? null,
				duration_ms: t.duration_ms ?? null,
			})),
		};
		if (shuffleMode && shuffleMode !== 'off') body.shuffle_mode = shuffleMode;
		return fetchApi<TidalMixPlaybackResponse>('/api/tidal/play-mix', undefined, {
			method: 'POST',
			body: JSON.stringify(body),
			timeoutMs: BULK_QUEUE_API_TIMEOUT_MS,
		});
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

	// ─── ACRCloud ─────────────────────────────────────────────────────

	getAcrCloudStatus() {
		return fetchApi<AcrCloudStatus>('/api/acrcloud/status');
	},

	configureAcrCloud(accessKey: string, accessSecret: string, region: string) {
		return fetchApi<{ status: string }>('/api/acrcloud/configure', undefined, {
			method: 'POST',
			body: JSON.stringify({ access_key: accessKey, access_secret: accessSecret, region }),
		});
	},

	deleteAcrCloudConfig() {
		return fetchApi<{ status: string }>('/api/acrcloud/configure', undefined, {
			method: 'DELETE',
		});
	},

	startAcrCloudScan() {
		return fetchApi<{ status: string }>('/api/library/acrcloud/scan', undefined, {
			method: 'POST',
		});
	},

	searchTidal(q: string, limit = 20, signal?: AbortSignal, offset = 0): Promise<TidalSearchResults> {
		return fetchApi<TidalSearchResults>(
			'/api/tidal/search',
			{ q, limit: String(limit), offset: String(offset) },
			{ signal },
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

	getTidalVideoMixItems(mixId: string | number): Promise<{ items: TidalVideoMixItem[] }> {
		return fetchApi<{ items: TidalVideoMixItem[] }>(
			`/api/tidal/video-mixes/${encodeURIComponent(String(mixId))}/items`
		);
	},

	playTidalTrack(track: TidalPlayable): Promise<void> {
		return fetchApi<void>('/api/tidal/play', undefined, {
			method: 'POST',
			body: JSON.stringify({
				tidal_track_id: track.tidal_id,
				title: track.title,
				artist_name: track.artist_name,
				album_title: track.album_title,
				artwork_url: track.artwork_url,
				duration_ms: track.duration_ms,
			}),
		});
	},

	getTidalArtistProfile(tidalArtistId: number): Promise<TidalArtistProfile> {
		return fetchApi<TidalArtistProfile>(`/api/tidal/artists/${tidalArtistId}`);
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
