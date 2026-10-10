// Wire types for the NOOR server API. `client.ts` re-exports all of them, so
// callers keep importing from '$lib/api/client'.

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

/** Settings > Library > Artwork cache. `max_mb` 0 is off. */
export type ArtworkCacheSettings = { max_mb: number; used_bytes: number; options_mb: number[] };

/** How much background video discovery the server runs. */
export type VideoDiscoverySetting = 'full' | 'limited' | 'off';

export interface VideoDiscoveryStatus {
	setting: VideoDiscoverySetting;
	mode: string;
	calls_last_hour: number;
	calls_today: number;
	artists_with_videos: number;
	catalog_videos: number;
}

export interface VideoStationCard {
	id: string;
	group: 'spotlight' | 'for_you' | 'genres' | 'vibes' | 'explore' | 'themes' | 'charts';
	title: string;
	subtitle: string;
	unwatched_count: number;
	preview: TidalSearchVideo[];
}

export interface VideoStationScene {
	slug: string;
	title: string;
	subtitle: string;
}

export interface VideoStationSettings {
	enabled: boolean;
	hidden: string[];
	scenes: VideoStationScene[];
}

export interface VideoStationsResponse {
	day: string | null;
	building: boolean;
	catalog_videos: number;
	discovery_setting: VideoDiscoverySetting;
	stations: VideoStationCard[];
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

/** One video in your watch history: the card, when you last watched it, how
 *  far you got, whether you finished it and how many times you watched it. */
export interface VideoHistoryEntry {
	video: TidalSearchVideo;
	watched_at: string;
	watched_ms: number | null;
	duration_ms: number | null;
	completed: boolean;
	plays: number;
}

export type TidalVideoMixItem = TidalSearchVideo & {
	mix_id?: string | number | null;
};

/** One editorial video set from /api/videos/discover. Items are shaped like
 *  TidalSearchVideo so they feed playVideo() queues directly; `why` is the
 *  optional per-pick reason from the server's shaping layer. */
export interface VideoDiscoverSet {
	slug: string;
	bucket_key: string;
	title: string;
	blurb: string;
	items: (TidalSearchVideo & { why?: string })[];
	/** True when this snapshot is from an older bucket while today's builds. */
	stale?: boolean;
}

export interface VideoDiscoverResponse {
	sets: VideoDiscoverSet[];
	/** True when a background build for the current bucket is in flight. */
	building: boolean;
}

/** One video of a liked song. `tidal_video_id` / `duration_ms` / `artwork_url`
 *  mirror TidalSearchVideo so a version lifts straight into a playVideo()
 *  queue. */
export interface LikedVideoVersion {
	track_id: number;
	tidal_video_id: number;
	video_title: string;
	duration_ms: number | null;
	artwork_url: string | null;
	match_score: number;
	/** What separates two versions when their titles do not. Rides along with
	 *  the artist-videos payload; not guaranteed to be there. */
	release_year: number | null;
}

/** One card on the liked-videos wall: a song, with every video found for it.
 *  A song favorited twice collapses to one card, and one video reached through
 *  both of those likes is one version - but genuinely different videos (the
 *  official cut, alternate edits, live takes) all survive in `versions`, best
 *  match first. */
export interface LikedVideo {
	song_key: string;
	track_title: string;
	artist_name: string | null;
	/** Local library id, for grouping only. Never put it in a video item. */
	artist_id: number | null;
	artist_tidal_id: number | null;
	album_year: number | null;
	genre: string | null;
	liked_at: string | null;
	/** Every liked row behind this card; hiding a version needs all of them. */
	track_ids: number[];
	versions: LikedVideoVersion[];
}

export interface LikedVideosResponse {
	videos: LikedVideo[];
	/** Resolve progress, so a first run reads as filling in rather than empty. */
	scanned_artists: number;
	total_artists: number;
	/** True while the background resolve pass is working. */
	running: boolean;
	tidal_connected: boolean;
}

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

export interface SpotifyLibrarySaveResponse {
	imported: number;
	totalTracks: number;
	resolvedCount: number;
	unresolvedCount: number;
	importFailures: number;
	localIds: number[];
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

export interface SpotifyPlaylistMeta {
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
}

export interface SpotifyPlaylistDetail extends SpotifyPlaylistMeta {
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
	videos: TidalArtistVideo[];
	similar_artists: TidalSimilarArtist[];
	bio: TidalArtistBio | null;
	available: boolean;
	/**
	 * Names of TIDAL sub-fetches that failed or timed out for this payload
	 * (e.g. "videos", "similar_artists"). The see-all view can use this to
	 * retry a release filter without treating an empty result as complete.
	 */
	sections_failed?: string[];
	release_filter_status?: ArtistReleaseFilterStatuses;
}

export type ArtistReleaseFilter = 'ALBUMS' | 'EPSANDSINGLES' | 'COMPILATIONS';

export interface ArtistReleaseFilterStatus {
	failed: boolean;
	has_more: boolean | null;
}

export type ArtistReleaseFilterStatuses = Record<ArtistReleaseFilter, ArtistReleaseFilterStatus>;

export interface TidalArtistReleasePage {
	albums: TidalDiscographyAlbum[];
	status: ArtistReleaseFilterStatus;
}

export interface TidalArtistCore {
	artist_name: string | null;
	picture_url: string | null;
	top_tracks: TidalDiscographyTrack[];
	available: boolean;
	sections_failed?: string[];
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

export type SampleDataSource = 'fingerprint';

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

/**
 * One row of an ordered mixed queue (POST /api/playback/queue).
 * `track_id` set: a library track that plays through the local pipeline.
 * Otherwise: an unresolved external track, resolved lazily by tidal id /
 * artist+title as a pending queue row. The display metadata is persisted on
 * the pending row so the queue renders richly before resolution.
 */
export interface MixedQueueItem {
	track_id?: number | null;
	tidal_id?: number | null;
	artist?: string | null;
	title?: string | null;
	album_title?: string | null;
	artwork_url?: string | null;
	duration_ms?: number | null;
	artist_tidal_id?: number | null;
	album_tidal_id?: number | null;
	reason?: string | null;
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
	queue_revision?: number;
	shuffle_debug?: ShuffleDebug | null;
}

export interface QueueSnapshot {
	queue: QueueItem[];
	queue_revision?: number;
}

export interface ShuffleDebug {
	mode: PlaybackState['shuffle_mode'];
	seed: number;
	scope: string;
	locked_count: number;
	candidate_count: number;
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

export type DjStrategy = 'adaptive' | 'wildcard' | 'smooth_blend' | 'club_mix' | 'quick_mix' | 'energy_lift' | 'energy_reset' | 'drop_swap' | 'bass_swap' | 'cut';

export type DjMixIntentResponse = {
	intent: DjMixIntent;
};

export type DjPolicyResponse = {
	mix_intent: DjMixIntent;
	transition_speed_bias: DjTransitionSpeedBias;
	preferred_strategy?: DjStrategy;
};

export type DjAutomationEvent = {
	param: Partial<Record<'DeckGain' | 'LowGain' | 'MidGain' | 'HighGain' | 'PlaybackRate', 'A' | 'B'>>;
	start_sample: number;
	end_sample: number;
	from: number;
	to: number;
	curve: 'Linear' | 'EqualPowerIn' | 'EqualPowerOut' | 'Cosine';
};

export type DjTransitionProgram = {
	template: string;
	tier: 'SafeCrossfade' | 'FullBlend';
	sample_rate: number;
	channels: number;
	deck_a_start_frame: number;
	deck_b_start_frame: number;
	sync_start: number;
	intro_start: number;
	swap_start: number;
	fade_start: number;
	resolve_at: number;
	loops: Array<{ deck: 'A' | 'B'; start_frame: number; end_frame: number }>;
	automation: DjAutomationEvent[];
	decision?: {
		strategy: string;
		confidence: number;
		score: number;
		reason: string;
		energy_direction: string;
		incoming_entry_seconds: number;
		incoming_drop_seconds?: number;
		outgoing_window: string;
		duration_beats: number;
		candidates: Array<{
			strategy: string;
			score: number;
			quality_score: number;
			entry_seconds: number;
			duration_seconds: number;
			reason: string;
			components: Array<{ name: string; value: number; weight: number }>;
		}>;
	};
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
	beat_confidence?: number;
	grid_is_synthetic?: boolean;
	analysis_scope_ms?: number;
	energy?: number;
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
	| 'handoff_seam_too_late'
	| string;

export type DjStatusResponse = {
	enabled: boolean;
	transition_plan?: DjTransitionProgram;
	playback_position_ms?: number;
	active_transition?: {
		event_id: number;
		outgoing: DjDeckStatus;
		incoming: DjDeckStatus;
		program: DjTransitionProgram;
		start_ms: number;
		actual_start_ms?: number;
		elapsed_ms: number;
	};
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
	runtime_planned_start_ms?: number;
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
	feedback_transition_event_id?: number;
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
	runtime_planned_start_ms?: number;
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

export type RadioSource = 'library' | 'lastfm' | 'engine' | 'tidal';

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
	queue_revision?: number;
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
	kind: 'track' | 'album' | 'playlist' | 'video';
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

export interface TidalMoodsResponse {
	categories: TidalMoodCategory[];
	source: string;
	fallback?: boolean;
	cached?: boolean;
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
	/**
	 * Set on an album item that TIDAL only has as a single. Last.fm's top-albums
	 * feed does not distinguish the two, so a famous 7" arrives as an album with
	 * no album behind it. `tidal_id` carries the track, and the card seeds song
	 * radio from it instead of opening a tracklist that does not exist.
	 */
	is_single?: boolean;
}

export interface ProviderRecommendationShelf {
	provider: 'lastfm' | 'listenbrainz' | string;
	entity_type?: 'track' | 'artist' | 'album' | string;
	title: string;
	/**
	 * `warming` means the shelf is empty *right now* but a rebuild is running
	 * behind it. Shelves publish one at a time, so the artist and album rails
	 * report this for a few seconds while the track mural is already painted.
	 * Never render an empty state for it - keep whatever is on screen.
	 */
	status: 'ok' | 'empty' | 'error' | 'warming' | string;
	message?: string;
	items: ProviderRecommendationItem[];
}

export interface HomeRecommendationsResponse {
	shelves: ProviderRecommendationShelf[];
}

/**
 * Hidden-gem picks for the Library home "Suggested tracks / albums" murals.
 * The server owns seed selection, recency exclusion and ranking; the client
 * renders both lists in the order given. `albums` is a first-class list, not
 * something derived from `tracks` - never-opened albums have their own recall
 * path server-side. Either list may come back short (thin library / cold
 * learning model); the murals render what they get rather than topping up.
 */
/**
 * Database size breakdown for the Settings maintenance panel.
 *
 * `reclaimable_bytes` is space already on the freelist that a VACUUM would
 * return to disk. `retired_neighbor_rows` is what the background pruner has yet
 * to delete - it becomes reclaimable once it has run. Deleting rows never
 * shrinks the file on its own (auto_vacuum is NONE).
 */
export interface DatabaseStats {
	file_bytes: number;
	wal_bytes: number;
	page_size: number;
	page_count: number;
	freelist_pages: number;
	/** Free pages only. Near zero while the bloat is still live retired rows. */
	freelist_bytes: number;
	/** What Compact would free: freelist plus the retired rows it deletes. */
	estimated_reclaimable_bytes: number;
	estimated_after_bytes: number;
	retired_models: number;
	retired_neighbor_rows: number;
}

export interface SuggestedAlbum {
	id: number;
	title: string;
	artist_id: number | null;
	artist_name: string | null;
	artwork_url: string | null;
}

export interface HomeSuggestionsResponse {
	tracks: Track[];
	albums: SuggestedAlbum[];
}

/// Library shuffle picks for the "Random tracks" / "Random albums" murals. The
/// server samples both in one request and keys the sample to a five-minute
/// bucket, so repeated calls inside the window return the same picks.
export interface HomeShufflePicksResponse {
	tracks: Track[];
	albums: Album[];
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
	// Raw user genre tokens ("rock", "hip-hop"); the server resolves them
	// against slug/name case-insensitively and expands to all descendants.
	genre_slugs?: string[];
	artist_contains?: string | null;
	album_contains?: string | null;
	is_instrumental?: boolean | null;
	limit?: number;
	// Page past the 50-row display cap ("Show more"). Ignored for shuffle.
	offset?: number;
	// Ask the server for a true random sample of the full matching set (library
	// Shuffle) instead of the deterministic display ranking.
	shuffle?: boolean;
	// Restrict matches to user-liked tracks (the Liked tab).
	liked_only?: boolean;
}

export interface AudioSearchResponse {
	tracks: AudioSearchResult[];
	// Full matching-set size, independent of the display LIMIT.
	total: number;
	// genre_slugs tokens that resolved to no known genre.
	unmatched_genres: string[];
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
	/** When true, an explicit pause frees the exclusive device immediately (re-grabbed on play). */
	exclusive_release_on_pause: boolean;
}
