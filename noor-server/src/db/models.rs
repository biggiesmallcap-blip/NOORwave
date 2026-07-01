use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub tidal_id: Option<i64>,
    pub ytmusic_id: Option<String>,
    pub soundcloud_id: Option<i64>,
    pub name: String,
    pub name_sort: Option<String>,
    pub biography: Option<String>,
    pub photo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub tidal_id: Option<i64>,
    pub ytmusic_id: Option<String>,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: Option<String>,
    pub year: Option<i32>,
    pub artwork_url: Option<String>,
    pub release_type: Option<String>,
    pub label: Option<String>,
    pub track_count: Option<i32>,
    pub is_favorite: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist_id: i64,
    pub artist_name: Option<String>,
    pub album_id: Option<i64>,
    pub album_title: Option<String>,
    pub disc_number: Option<i32>,
    pub track_number: Option<i32>,
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    pub tidal_id: Option<i64>,
    // TIDAL identity for the artist/album, so a non-library TIDAL track (mix,
    // playlist, radio, search) keeps clickable artist/album links + menus through
    // the whole player. Library tracks leave these None and resolve via
    // artist_id/album_id. `#[serde(default)]` so older payloads still deserialize.
    #[serde(default)]
    pub artist_tidal_id: Option<i64>,
    #[serde(default)]
    pub album_tidal_id: Option<i64>,
    pub ytmusic_id: Option<String>,
    pub soundcloud_id: Option<i64>,
    pub best_quality: Option<String>,
    pub best_source: Option<String>,
    pub fidelity_score: i32,
    pub is_favorite: bool,
    pub play_count: i32,
    pub last_played_at: Option<String>,
    pub date_added: Option<String>,
    pub source: String,
    pub artwork_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playlist {
    pub id: i64,
    pub tidal_uuid: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub is_smart: bool,
    pub smart_rules: Option<String>,
    pub is_synced: bool,
    pub track_count: i32,
    pub is_favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub parent_id: Option<i64>,
    pub children: Vec<Genre>,
    pub track_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    pub current_track: Option<Track>,
    /// Queue item id of the currently-playing row. Set even when current_track_id
    /// is NULL (pending rows). Used for position scans that must work for both
    /// library and pending rows.
    #[serde(default)]
    pub current_queue_item_id: Option<i64>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub volume: f64,
    pub shuffle_mode: String,
    pub repeat_mode: String,
    pub automix_enabled: bool,
    pub crossfade_ms: i32,
    pub automix_discover_new: bool,
    pub automix_use_learning: bool,
    pub automix_allow_external: bool,
    /// How many ms of the currently-playing track are decoded into the
    /// playback buffer. 0 when no track is active or before the audio
    /// callback has published the first value. Drives the buffered-bar
    /// scrubber on the frontend and the route-side seek ack: a seek target
    /// greater than `buffered_ms` is rejected with HTTP 409 instead of
    /// dispatched to the runtime (which would WARN-spam and snap-back).
    ///
    /// `#[serde(default)]` so existing JSON payloads / construction sites
    /// that predate this field keep deserializing/compiling without a
    /// per-site change.
    #[serde(default)]
    pub buffered_ms: i64,
    /// Track-time offset (ms) where the audibly-current engine's decoded
    /// audio begins. 0 for a fresh-from-start engine; non-zero only after a
    /// true DASH segment seek (option C). The route-side SeekTo handler uses
    /// this as the LOWER bound of the in-buffer fast path: a target inside
    /// `[buffered_start_ms, buffered_ms]` lands in the existing buffer; a
    /// target outside takes the segment-seek restart path. `#[serde(default)]`
    /// for backwards-compat with pre-option-C consumers.
    #[serde(default)]
    pub buffered_start_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub track: Track,
    pub position: i32,
    pub source: String,
    #[serde(default)]
    pub reason: Option<String>,
    /// true when track_id IS NULL (Tidal resolution not yet complete).
    /// Synthesised at query time from queue.track_id; not a stored column.
    #[serde(default)]
    pub is_pending: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsOverview {
    pub tracks: i64,
    pub albums: i64,
    pub artists: i64,
    pub playlists: i64,
    pub smart_playlists: i64,
    pub tagged_tracks: i64,
    pub total_listens: i64,
    pub favorite_tracks: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenSource {
    Manual,
    Radio,
    Playlist,
    Album,
    Artist,
    Search,
    Automix,
    Unknown,
}

impl ListenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ListenSource::Manual => "manual",
            ListenSource::Radio => "radio",
            ListenSource::Playlist => "playlist",
            ListenSource::Album => "album",
            ListenSource::Artist => "artist",
            ListenSource::Search => "search",
            ListenSource::Automix => "automix",
            ListenSource::Unknown => "unknown",
        }
    }

    pub fn parse(raw: &str) -> Option<ListenSource> {
        match raw {
            "manual" => Some(ListenSource::Manual),
            "radio" => Some(ListenSource::Radio),
            "playlist" => Some(ListenSource::Playlist),
            "album" => Some(ListenSource::Album),
            "artist" => Some(ListenSource::Artist),
            "search" => Some(ListenSource::Search),
            "automix" => Some(ListenSource::Automix),
            "unknown" => Some(ListenSource::Unknown),
            _ => None,
        }
    }

    // Backfilled rows have unknown provenance, so the trainer downweights edges
    // supported only by them. Live-recorded rows count at full strength.
    pub fn confidence_multiplier(self) -> f64 {
        match self {
            ListenSource::Unknown => 0.5,
            _ => 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenHistoryEntry {
    pub id: i64,
    pub track_id: i64,
    pub track_title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub started_at: String,
    pub duration_listened_ms: i64,
    pub completed: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub source: Option<ListenSource>,
    #[serde(default)]
    pub position_in_session: Option<i32>,
    #[serde(default)]
    pub transition_from_track_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsTopTrack {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub listens: i64,
    pub completed_listens: i64,
    pub total_listened_ms: i64,
    pub completion_rate: Option<f64>,
    pub share_of_window_listened_ms: Option<f64>,
    pub previous_rank: Option<i64>,
    pub rank_delta: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsTopArtist {
    pub artist_id: i64,
    pub artist_name: String,
    pub listens: i64,
    pub completed_listens: i64,
    pub unique_tracks: i64,
    pub total_listened_ms: i64,
    pub completion_rate: Option<f64>,
    pub share_of_window_listened_ms: Option<f64>,
    pub previous_rank: Option<i64>,
    pub rank_delta: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsGenreShare {
    pub genre_name: String,
    pub listens: i64,
    pub share_of_window_listens: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreHeat {
    pub genre_id: i64,
    pub genre_name: String,
    pub listen_count: i64,
    pub total_listened_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsActivityPoint {
    pub day: String,
    pub listens: i64,
    pub completed_listens: i64,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsBehavior {
    pub total_listened_ms: i64,
    pub total_listens: i64,
    pub completed_listens: i64,
    pub skipped_listens: i64,
    pub completion_rate: f64,
    pub average_listen_ms: i64,
    pub unique_tracks: i64,
    pub repeat_track_count: i64,
    pub active_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsDashboard {
    pub overview: AnalyticsOverview,
    pub recent_listens: Vec<ListenHistoryEntry>,
    pub top_tracks: Vec<AnalyticsTopTrack>,
    pub top_artists: Vec<AnalyticsTopArtist>,
    pub top_genres: Vec<AnalyticsGenreShare>,
    pub activity: Vec<AnalyticsActivityPoint>,
    pub behavior: AnalyticsBehavior,
}

// ─────────────────────────────────────────────────────────────────────────────
// Analytics signals — response shape for GET /api/analytics/signals.
// Contract: noor-server/tests/fixtures/signals-schema.json
// JSON schema: noor-server/tests/fixtures/signals-schema.json
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Day,
    Week,
    Month,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsSignals {
    pub window: SignalsWindow,
    pub totals: AnalyticsTotals,
    pub kpis: SignalsKpis,
    pub tempo: TempoView,
    pub sonic_field: SonicView,
    pub ridgeline: Vec<RidgeRow>,
    pub top_tracks: Vec<AnalyticsTopTrack>,
    pub top_artists: Vec<AnalyticsTopArtist>,
    pub top_genres: Vec<AnalyticsGenreShare>,
    pub cohorts: Vec<Cohort>,
    pub audio_profile: AudioProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalsWindow {
    pub days: i64,
    pub started_at: String,
    pub previous_started_at: String,
    pub generated_at: String,
    pub granularity: Granularity,
    pub display_caps: DisplayCaps,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayCaps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ridgeline_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo_rows: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsTotals {
    pub listens: i64,
    pub listened_ms: i64,
    pub distinct_tracks: i64,
    pub tagged_listens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalsKpis {
    pub listened_ms: KpiPairInt,
    pub sessions: KpiPairInt,
    pub completion: KpiPairFloat,
    pub skip_rate: KpiPairFloat,
    pub daily: Vec<DailyKpi>,
    pub hero_stats: HeroStats,
    pub sessions_coverage: SessionsCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KpiPairInt {
    pub current: i64,
    pub previous: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KpiPairFloat {
    pub current: Option<f64>,
    pub previous: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyKpi {
    pub day: String,
    pub listens: i64,
    pub listened_ms: i64,
    pub completed: i64,
    pub sessions: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeroStats {
    pub peak_hour: Option<i32>,
    pub rhythm: Option<i32>,
    pub night_share: Option<f64>,
    pub morning_share: Option<f64>,
    // Single-day mode (days <= 1) only — None in default mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longest_session_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distinct_tracks: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsCoverage {
    // Listens with non-null session_id (post-MIGRATION_023 rows).
    pub tracked: i64,
    // Listens with null session_id (pre-MIGRATION_023 history).
    pub untracked: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoView {
    pub bucket_axis: BucketAxis,
    pub rows: Vec<TempoRow>,
    pub stats: TempoStats,
    pub coverage: Coverage,
    pub ridge_amp_max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketAxis {
    pub min: i32,
    pub max: i32,
    pub step: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoRow {
    pub label: String,
    pub granularity: Granularity,
    // Dense over `bucket_axis` — every (max-min)/step + 1 bucket present, zero-filled.
    pub buckets: Vec<BpmBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpmBucket {
    pub bucket: i32,
    pub listens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TempoStats {
    pub median: Option<f64>,
    pub mode: Option<f64>,
    pub sigma: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Coverage {
    pub analyzed: i64,
    pub total_listened: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonicView {
    pub tracks: Vec<SonicTrack>,
    pub total: i64,
    pub coverage: Coverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SonicTrack {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album: Option<String>,
    pub artwork_path: Option<String>,
    pub file_path: Option<String>,
    pub e: f64,
    pub d: f64,
    pub bpm: f64,
    pub listens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RidgeRow {
    pub date: String,
    // Always exactly 24 entries, zero-filled in Rust.
    pub hourly: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cohort {
    pub key: String,
    pub label: String,
    pub tracks: i64,
    pub listened_ms: i64,
    pub sessions: i64,
    pub completion: Option<f64>,
    pub skip_rate: Option<f64>,
    pub new_artists: i64,
    pub repeat_rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioProfile {
    pub dynamic_range_dr: Option<f64>,
    pub loudness_lufs: Option<f64>,
    pub bass_tilt: Option<f64>,
    pub treble_tilt: Option<f64>,
    pub coverage: Coverage,
    pub track_coverage: Coverage,
    pub listen_coverage: Coverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPreset {
    pub id: i64,
    pub name: String,
    pub prompt: String,
    pub mode: String,
    pub services: Vec<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryProfilePreview {
    pub prompt: String,
    pub mode: String,
    pub services: Vec<String>,
    pub prompt_terms: Vec<String>,
    pub prompt_genres: Vec<String>,
    pub top_artists: Vec<String>,
    pub top_genres: Vec<String>,
    pub recent_tracks: Vec<String>,
    pub favorite_ratio: f64,
    pub completion_rate: f64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReason {
    pub label: String,
    pub detail: String,
    pub weight: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPreviewResult {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub service: String,
    pub service_track_id: String,
    pub score: i32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryPreview {
    pub profile: DiscoveryProfilePreview,
    pub reasons: Vec<DiscoveryReason>,
    pub results: Vec<DiscoveryPreviewResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryProviderCapability {
    pub provider: String,
    pub can_save: bool,
    pub can_play_inline: bool,
    pub can_fetch_connections: bool,
    pub can_map_genres: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryExternalResult {
    pub provider: String,
    pub provider_track_id: String,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub audio_quality: Option<String>,
    pub normalized_genres: Vec<String>,
    pub lastfm_tags: Vec<String>,
    pub lastfm_similarity_score: Option<f64>,
    pub discogs_genres: Vec<String>,
    pub discogs_styles: Vec<String>,
    pub discogs_label: Option<String>,
    pub discogs_year: Option<i32>,
    pub discogs_confidence: Option<f64>,
    pub in_library: bool,
    pub is_saved: bool,
    pub is_playable: bool,
    pub embedding_score: Option<f64>,
    pub score: i32,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConnectionTrailItem {
    pub provider: String,
    pub provider_track_id: String,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub normalized_genres: Vec<String>,
    pub connection_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryExternalFeed {
    pub profile: DiscoveryProfilePreview,
    pub reasons: Vec<DiscoveryReason>,
    pub results: Vec<DiscoveryExternalResult>,
    pub capabilities: Vec<DiscoveryProviderCapability>,
    pub trail_item: Option<DiscoveryConnectionTrailItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryNeighborReason {
    pub key: String,
    pub label: String,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryStatus {
    pub fallback_active: bool,
    pub active_model: Option<EmbeddingModel>,
    pub selected_engine: String,
    pub selected_engine_family: String,
    pub selected_engine_trainable: bool,
    pub latest_run: Option<DiscoveryTrainingRun>,
    pub coverage_ratio: f64,
    pub playable_tracks: i64,
    pub embedded_tracks: i64,
    pub neighbor_tracks: i64,
    pub clip_cache_tracks: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingModel {
    pub id: i64,
    pub model_key: String,
    pub family: String,
    pub dimension: i32,
    pub status: String,
    pub is_active: bool,
    pub trained_at: Option<String>,
    pub config_json: Option<String>,
    pub metrics_json: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryTrainingRun {
    pub id: i64,
    pub model_id: Option<i64>,
    pub stage: String,
    pub status: String,
    pub progress: f64,
    pub items_total: Option<i64>,
    pub items_done: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryRadioResult {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub similarity_score: f64,
    pub adjusted_score: f64,
    pub co_listen_score: f64,
    pub co_album_score: f64,
    pub co_artist_score: f64,
    pub genre_proximity: f64,
    pub reason_tags: Vec<String>,
    pub model_key: Option<String>,
    pub source_mode: String,
    // Tier 1 diagnostics surfaced from the neighbor row. None when read from a
    // pre-Tier-1 model that was trained before MIGRATION_022.
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub support_count: i64,
    #[serde(default)]
    pub candidate_in_degree: i64,
    #[serde(default)]
    pub candidate_in_degree_percentile: f64,
    #[serde(default)]
    pub primary_reason: Option<String>,
}

// ─── Audio DSP Features ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDspFeatures {
    pub track_id: i64,
    pub bpm: Option<f64>,
    pub key_signature: Option<String>,
    pub camelot_key: Option<String>,
    pub loudness_lufs: Option<f64>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub beat_strength: Option<f64>,
    pub spectral_centroid: Option<f64>,
    pub stereo_width: Option<f64>,
    pub is_instrumental: bool,
    pub analysis_source: String,
    pub analysis_offset_ms: i64,
    pub samples_analyzed: Option<i64>,
    pub analyzed_at: String,
    pub analysis_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDjProfileKey {
    pub media_ref_kind: String,
    pub media_ref_id: String,
}

#[derive(Debug, Clone)]
pub struct AudioDjProfileRow {
    pub media_ref_kind: String,
    pub media_ref_id: String,
    pub track_id: Option<i64>,
    pub queue_item_id: Option<i64>,
    pub tidal_id: Option<i64>,
    pub profile_version: String,
    pub beat_grid_blob: Vec<u8>,
    pub downbeats_blob: Vec<u8>,
    pub phrase_boundaries_blob: Vec<u8>,
    pub mix_in_blob: Vec<u8>,
    pub mix_out_blob: Vec<u8>,
    pub intro_end_seconds: Option<f64>,
    pub outro_start_seconds: Option<f64>,
    pub breakdown_blob: Vec<u8>,
    pub drop_blob: Vec<u8>,
    pub safe_transition_windows_blob: Vec<u8>,
    pub energy_contour_blob: Vec<u8>,
    pub vocal_presence_blob: Vec<u8>,
    pub vocal_density_blob: Vec<u8>,
    pub waveform_peaks_blob: Vec<u8>,
    pub lufs_loud_body: Option<f64>,
    pub true_peak_dbtp: Option<f64>,
    pub beat_confidence: Option<f64>,
    pub profile_confidence: f64,
    pub analysis_scope_ms: i64,
    pub is_temporary: bool,
    pub source: String,
    pub computed_at: String,
}

#[derive(Debug, Clone)]
pub struct AudioDjProfileCorrectionRow {
    pub media_ref_kind: String,
    pub media_ref_id: String,
    pub bpm_multiplier: Option<f64>,
    pub downbeat_offset_beats: Option<i64>,
    pub phrase_offset_bars: Option<i64>,
    pub safe_crossfade_only: bool,
    pub transition_speed_bias: Option<String>,
    pub manual_drop_blob: Vec<u8>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFeaturesStats {
    pub total_analyzed: i64,
    pub avg_bpm: Option<f64>,
    pub top_key: Option<String>,
    pub avg_energy: Option<f64>,
    pub key_distribution: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenreAudioMetrics {
    pub genre_id: i64,
    pub genre_name: String,
    pub avg_bpm: Option<f64>,
    pub avg_energy: Option<f64>,
    pub avg_danceability: Option<f64>,
    pub analyzed_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AudioFingerprint {
    pub track_id: i64,
    pub hashes_blob: Option<Vec<u8>>,
    pub peak_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct AcrCloudResult {
    pub id: i64,
    pub track_id: i64,
    pub original_title: Option<String>,
    pub original_artist: Option<String>,
    pub original_album: Option<String>,
    pub original_year: Option<i32>,
    pub confidence_score: Option<f64>,
    pub sample_start_ms: Option<i64>,
    pub sample_end_ms: Option<i64>,
    pub isrc: Option<String>,
    pub matched_at: String,
    pub api_response_json: Option<String>,
}
