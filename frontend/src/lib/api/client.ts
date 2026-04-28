import type { DiscoverySpaceMeta } from '$lib/components/Discover/discover.types';

const API_BASE = 'http://localhost:3334';

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

// Drop-in replacement for fetch() that attaches the Bearer token and fires
// the noor:unauthorized event on 401, matching the behaviour of fetchApiResponse.
export async function authFetch(url: string, init?: RequestInit): Promise<Response> {
	const token = getStoredToken();
	const headers = new Headers(init?.headers);
	if (token) headers.set('authorization', `Bearer ${token}`);
	const resp = await fetch(url, { ...init, headers });
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

export interface TidalDiscographyAlbum {
	tidal_id: number;
	local_id: number | null;
	title: string;
	artwork_url: string | null;
	release_date: string | null;
	release_type: string | null;
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

export interface TidalSearchPlaylist {
	uuid: string;
	title: string;
	description: string | null;
	number_of_tracks: number | null;
	square_image: string | null;
}

export interface TidalSearchResults {
	tracks: TidalSearchTrack[];
	albums: TidalSearchAlbum[];
	artists: TidalSearchArtist[];
}

export interface TidalArtistProfile {
	artist_name: string | null;
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
	is_in_library?: boolean;
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
}

export interface PlaybackState {
	current_track: Track | null;
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
}

export interface PlaybackSnapshot {
	state: PlaybackState;
	queue: QueueItem[];
}

export interface PlaybackRuntimeInfo {
	device_name: string;
	sample_rate: number;
	channels: number;
	active_track_id: number | null;
	last_error: string | null;
}

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
}

export interface PortableMusicBrainzSnapshotAction {
	status: 'exported' | 'imported';
	snapshot: PortableMusicBrainzSnapshotStatus;
	checked_inserted?: number;
	checked_skipped?: number;
	genre_inserted?: number;
	track_skipped?: number;
	genre_skipped?: number;
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
}

export interface AnalyticsTopArtist {
	artist_id: number;
	artist_name: string;
	listens: number;
	completed_listens: number;
	unique_tracks: number;
	total_listened_ms: number;
}

export interface AnalyticsGenreShare {
	genre_name: string;
	listens: number;
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
	latest_run: DiscoveryTrainingRun | null;
	coverage_ratio: number;
	playable_tracks: number;
	embedded_tracks: number;
	neighbor_tracks: number;
	clip_cache_tracks: number;
}

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

export interface HomeReleasesResponse {
	releases: RSSFeedItem[];
	source: string;
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
}

async function fetchApiResponse(
	path: string,
	params?: Record<string, string>,
	options?: RequestInit
): Promise<Response> {
	const url = new URL(`${getApiBase()}${path}`);
	if (params) {
		Object.entries(params).forEach(([k, v]) => url.searchParams.set(k, v));
	}

	const token = getStoredToken();
	const authHeader: Record<string, string> = token
		? { authorization: `Bearer ${token}` }
		: {};

	const resp = await fetch(url.toString(), {
		headers: {
			'content-type': 'application/json',
			...authHeader,
			...(options?.headers as Record<string, string> ?? {}),
		},
		...options,
	});

	if (resp.status === 401) {
		// Token was rejected — dispatch an event so the UI can show the connect screen
		window.dispatchEvent(new CustomEvent('noor:unauthorized'));
	}

	return resp;
}

async function fetchApi<T>(
	path: string,
	params?: Record<string, string>,
	options?: RequestInit
): Promise<T> {
	const resp = await fetchApiResponse(path, params, options);
	if (!resp.ok) {
		const errorBody = await resp.json().catch(() => null);
		const message =
			errorBody?.message ??
			errorBody?.details ??
			errorBody?.status ??
			`API error: ${resp.status}`;
		throw new Error(message);
	}
	return resp.json();
}

export const api = {
	getTracks(sortBy = 'date_added', sortDir = 'desc', limit = 50, offset = 0, favoriteOnly = true) {
		return fetchApi<{ tracks: Track[]; total: number }>('/api/tracks', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
			favorite_only: String(favoriteOnly),
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
		return fetchApi<{ tracks: Track[] }>(`/api/albums/${id}/tracks`);
	},

	getArtists(sortBy = 'name', sortDir = 'asc', limit = 50, offset = 0) {
		return fetchApi<{ artists: Artist[] }>('/api/artists', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
		});
	},

	getArtistTracks(id: number) {
		return fetchApi<{ tracks: Track[] }>(`/api/artists/${id}/tracks`);
	},

	getArtistDiscography(id: number) {
		return fetchApi<{
			albums: TidalDiscographyAlbum[];
			top_tracks: TidalDiscographyTrack[];
			available: boolean;
			reason?: string;
		}>(`/api/artists/${id}/discography`);
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
		return fetchApi<{ tidal_id: number; local_id: number }>('/api/tidal/tracks/import', undefined, {
			method: 'POST',
			body: JSON.stringify({
				tidal_id: track.tidal_id,
				title: track.title,
				artist_name: track.artist_name,
				artist_tidal_id: track.artist_tidal_id ?? null,
				album_title: track.album_title,
				duration_ms: track.duration_ms,
			}),
		});
	},

	getGenres() {
		return fetchApi<{ genres: Genre[] }>('/api/genres');
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

	searchTidalPlaylists(q: string) {
		return fetchApi<{ playlists: TidalSearchPlaylist[] }>(
			`/api/tidal/playlists/search?q=${encodeURIComponent(q)}`,
		);
	},

	getTidalPlaylistTracks(tidalUuid: string) {
		return fetchApi<{ tracks: TidalPlayable[] }>(
			`/api/tidal/playlists/${tidalUuid}/tracks`,
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
		return fetchApi<{ status: string; mode: string }>('/api/discovery/train', undefined, {
			method: 'POST',
			body: JSON.stringify({ mode, rebuild_audio }),
		});
	},

	stopDiscoveryTraining() {
		return fetchApi<{ status: string }>('/api/discovery/train/stop', undefined, {
			method: 'POST',
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

	search(query: string, limit = 20) {
		return fetchApi<SearchResults>('/api/search', { q: query, limit: String(limit) });
	},

	searchAudio(params: AudioSearchParams) {
		// Strip null/undefined fields before sending
		const body: Record<string, unknown> = {};
		for (const [k, v] of Object.entries(params)) {
			if (v !== null && v !== undefined) body[k] = v;
		}
		return fetchApi<{ tracks: AudioSearchResult[] }>('/api/search/audio', undefined, {
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

	setPlaybackPosition(positionMs: number) {
		return fetchApi<{ state: PlaybackState }>('/api/playback/position', undefined, {
			method: 'POST',
			body: JSON.stringify({ position_ms: positionMs }),
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

	replacePlaybackQueue(trackIds: number[]) {
		return fetchApi<{ queue: QueueItem[] }>('/api/playback/queue', undefined, {
			method: 'POST',
			body: JSON.stringify({ track_ids: trackIds }),
		});
	},

	removeQueueTrack(queueItemId: number) {
		return fetchApi<{ queue: QueueItem[] }>('/api/playback/queue/remove', undefined, {
			method: 'POST',
			body: JSON.stringify({ queue_item_id: queueItemId }),
		});
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

	stopAudioAnalysis() {
		return fetchApi<{ status: string }>('/api/library/analyze/stop', undefined, { method: 'POST' });
	},

	getTrackAudioFeatures(trackId: number) {
		return fetchApi<{ features: AudioDspFeatures | null }>(`/api/tracks/${trackId}/audio-features`);
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

	searchTidal(q: string, limit = 20): Promise<TidalSearchResults> {
		return fetchApi<TidalSearchResults>('/api/tidal/search', { q, limit: String(limit) });
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

	getDiscoverySpaceMeta() {
		return fetchApi<DiscoverySpaceMeta>('/api/discovery/space/meta');
	},

	getVibeTracksForTrack(trackId: number) {
		return fetchApi<{ tracks: VibeTrack[] }>(`/api/search/vibe?track_id=${trackId}`);
	},

	getUnderratedTracksForArtist(artistId: number) {
		return fetchApi<{ tracks: BasicTrack[] }>(`/api/search/underrated?artist_id=${artistId}`);
	},
};
