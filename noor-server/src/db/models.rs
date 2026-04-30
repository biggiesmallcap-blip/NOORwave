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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsTopArtist {
    pub artist_id: i64,
    pub artist_name: String,
    pub listens: i64,
    pub completed_listens: i64,
    pub unique_tracks: i64,
    pub total_listened_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticsGenreShare {
    pub genre_name: String,
    pub listens: i64,
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
pub struct AudioFingerprint {
    pub track_id: i64,
    pub hashes_blob: Option<Vec<u8>>,
    pub peak_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
