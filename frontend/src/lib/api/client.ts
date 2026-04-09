const API_BASE = 'http://localhost:3333';

export function getApiBase(): string {
	if (typeof window === 'undefined') {
		return API_BASE;
	}

	const { protocol, hostname } = window.location;
	return `${protocol}//${hostname}:3333`;
}

export interface Track {
	id: number;
	title: string;
	artist_id: number;
	artist_name: string | null;
	album_id: number | null;
	album_title: string | null;
	disc_number: number | null;
	track_number: number | null;
	duration_ms: number | null;
	tidal_id: number | null;
	best_quality: string | null;
	best_source: string | null;
	fidelity_score: number;
	is_favorite: boolean;
	play_count: number;
	date_added: string | null;
	source: string;
	artwork_url: string | null;
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
}

// ─── Smart Playlist Rule Types ───────────────────────────────────────────────

export type LogicOp = 'AND' | 'OR';
export type NumberOp = 'eq' | 'gte' | 'lte' | 'gt' | 'lt' | 'between_inclusive';
export type QualityTier = 'lossy' | 'lossless' | 'hi_res';
export type DateField = 'date_added' | 'last_played_at';

export type RuleClause =
	| { type: 'group'; op: LogicOp; clauses: RuleClause[] }
	| { type: 'genre'; names: string[]; match_descendants: boolean }
	| { type: 'artist'; names: string[] }
	| { type: 'date_range'; field: DateField; range: { start: string | null; end: string | null } }
	| { type: 'play_count'; op: NumberOp; value: number; value_max?: number | null }
	| { type: 'quality'; minimum: QualityTier }
	| { type: 'not_in_playlist'; playlist_ids: number[] };

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

async function fetchApiResponse(
	path: string,
	params?: Record<string, string>,
	options?: RequestInit
): Promise<Response> {
	const url = new URL(`${getApiBase()}${path}`);
	if (params) {
		Object.entries(params).forEach(([k, v]) => url.searchParams.set(k, v));
	}

	return fetch(url.toString(), {
		headers: {
			'content-type': 'application/json',
			...(options?.headers ?? {}),
		},
		...options,
	});
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
	getTracks(sortBy = 'date_added', sortDir = 'desc', limit = 50, offset = 0) {
		return fetchApi<{ tracks: Track[]; total: number }>('/api/tracks', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
		});
	},

	getAlbums(sortBy = 'title', sortDir = 'asc', limit = 50, offset = 0) {
		return fetchApi<{ albums: Album[]; total: number }>('/api/albums', {
			sort_by: sortBy,
			sort_dir: sortDir,
			limit: String(limit),
			offset: String(offset),
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

	getGenres() {
		return fetchApi<{ genres: Genre[] }>('/api/genres');
	},

	getGenreHeat(days = 90) {
		return fetchApi<{ heat: GenreHeat[] }>('/api/genres/heat', {
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

	search(query: string, limit = 20) {
		return fetchApi<SearchResults>('/api/search', { q: query, limit: String(limit) });
	},

	getStatus() {
		return fetchApi<{ name: string; version: string; status: string }>('/api/status');
	},

	getPlaybackState() {
		return fetchApi<PlaybackSnapshot>('/api/playback/state');
	},

	getPlaybackRuntime() {
		return fetchApi<{ available: boolean; runtime: PlaybackRuntimeInfo | null }>(
			'/api/playback/runtime'
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

	setPlaybackAutomix(enabled: boolean, crossfade_ms?: number, discover_new?: boolean) {
		return fetchApi<{ state: PlaybackState; queue: QueueItem[] }>('/api/playback/automix', undefined, {
			method: 'POST',
			body: JSON.stringify({ enabled, crossfade_ms, discover_new }),
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
};
