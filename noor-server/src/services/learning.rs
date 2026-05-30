use crate::AppEvent;
use crate::db::{
    Database,
    models::{
        DiscoveryNeighborReason, DiscoveryPreview, DiscoveryPreviewResult, DiscoveryProfilePreview,
        DiscoveryRadioResult, DiscoveryReason,
    },
    queries::{self, EmbeddingTrackRow, TrackSimilarityResult},
};
use crate::metadata::lastfm::{LastFmClient, LastFmSimilarTrack};
use crate::services::discovery::DiscoveryCandidateTrack;
use crate::services::discovery_trainer::{
    AUDIO_PROXY_FEATURE_VERSION, EvidenceKind, HeldoutExample, TrainerEdge, TrainerEvidenceGroup,
    TrainerExternalCandidate, TrainerExternalNeighbor, TrainerInput, TrainerSequenceGroup,
    TrainingProgressUpdate, run_discovery_training,
};
use crate::services::tidal::client::{TidalClient, TidalSearchTrack, TidalTrack};
use anyhow::{Context, Result, bail};
use rusqlite::OptionalExtension;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

const MODEL_FAMILY: &str = queries::DISCOVERY_ENGINE_V2_FAMILY;
const EXTERNAL_TRAINING_CANDIDATE_LIMIT: i64 = 1_000;
const EXTERNAL_TRAINING_RESOLVED_LASTFM_LIMIT: i64 = 5_000;
const LASTFM_DIRECT_EDGE_WEIGHT: f64 = 0.55;
const LASTFM_BRANCH_EDGE_WEIGHT: f64 = 0.35;
const EXTERNAL_REFRESH_MAX_SEED_TRACKS: usize = 100;
const EXTERNAL_REFRESH_LASTFM_ROWS_PER_SEED: usize = 20;
const EXTERNAL_REFRESH_LASTFM_BRANCH_SEED_TRACKS: usize = 25;
const EXTERNAL_REFRESH_LASTFM_BRANCHES_PER_SEED: usize = 2;
const EXTERNAL_REFRESH_LASTFM_BRANCH_ROWS: usize = 5;
const EXTERNAL_REFRESH_LASTFM_BRANCH_ATTENUATION: f64 = 0.65;
const EXTERNAL_REFRESH_LASTFM_BRANCH_MIN_PARENT_MATCH: f64 = 0.50;
const EXTERNAL_REFRESH_TIDAL_NEW_RELEASE_ROWS: usize = 500;
const EXTERNAL_REFRESH_TIDAL_SIMILAR_SEED_TRACKS: usize = 10;
const EXTERNAL_REFRESH_TIDAL_SIMILAR_ARTISTS_PER_SEED: i32 = 2;
const EXTERNAL_REFRESH_TIDAL_SIMILAR_TRACKS_PER_ARTIST: i32 = 2;
const EXTERNAL_REFRESH_STALE_HOURS: i64 = 24;
const EXTERNAL_REFRESH_LASTFM_DELAY_MS: u64 = 500;
const EXTERNAL_REFRESH_LASTFM_RATE_LIMIT_COOLDOWN_MS: u64 = 10 * 60 * 1000;
const EXTERNAL_TIDAL_RESOLUTION_FULL_LIMIT: i64 = 500;
const EXTERNAL_TIDAL_RESOLUTION_INCREMENTAL_LIMIT: i64 = 150;
const EXTERNAL_TIDAL_RESOLUTION_SEARCH_LIMIT: i32 = 10;
pub const DISCOVERY_TRAINING_SAFETY_TIMEOUT_MESSAGE: &str =
    "Laptop safety timeout stopped discovery training.";
const DISCOVERY_TRAINING_TIMEOUT_STANDARD_SECS: u64 = 30 * 60;
const DISCOVERY_TRAINING_TIMEOUT_MAX_SECS: u64 = 60 * 60;
const DISCOVERY_TRAINING_LAPTOP_MAX_WORKERS: usize = 4;
const DISCOVERY_TRAINING_BALANCED_MAX_WORKERS: usize = 8;
const DISCOVERY_TRAINING_PERFORMANCE_MAX_WORKERS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalProviderRefreshBudget {
    pub should_refresh: bool,
    pub seed_tracks: usize,
    pub lastfm_rows_per_seed: usize,
    pub tidal_new_release_rows: usize,
}

#[derive(Clone, Default)]
pub struct ExternalProviderRefreshClients {
    pub lastfm: Option<LastFmClient>,
    pub tidal: Option<TidalClient>,
}

#[derive(Debug, Clone)]
pub struct ExternalLastfmCandidate {
    pub artist: String,
    pub title: String,
    pub mbid: Option<String>,
    pub match_score: f64,
    pub branch_from: Option<String>,
}

impl From<LastFmSimilarTrack> for ExternalLastfmCandidate {
    fn from(value: LastFmSimilarTrack) -> Self {
        Self {
            artist: value.artist,
            title: value.title,
            mbid: value.mbid,
            match_score: value.match_score,
            branch_from: None,
        }
    }
}

impl ExternalLastfmCandidate {
    fn branch_from(value: LastFmSimilarTrack, parent: &ExternalLastfmCandidate) -> Self {
        Self {
            artist: value.artist,
            title: value.title,
            mbid: value.mbid,
            match_score: (parent.match_score
                * value.match_score
                * EXTERNAL_REFRESH_LASTFM_BRANCH_ATTENUATION)
                .clamp(0.0, 1.0),
            branch_from: Some(format!("{} - {}", parent.artist, parent.title)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalTidalCandidate {
    pub tidal_id: i64,
    pub artist_name: String,
    pub title: String,
    pub genre_tags: Vec<String>,
    pub duration_ms: Option<i64>,
}

impl From<TidalSearchTrack> for ExternalTidalCandidate {
    fn from(value: TidalSearchTrack) -> Self {
        let genre_tags = collect_tidal_candidate_genres(&value.extra);
        Self {
            tidal_id: value.id,
            artist_name: value
                .artist_name
                .unwrap_or_else(|| "Unknown Artist".to_string()),
            title: value.title,
            genre_tags,
            duration_ms: Some(value.duration.saturating_mul(1000)),
        }
    }
}

fn external_tidal_candidate_from_track(value: TidalTrack) -> ExternalTidalCandidate {
    let genre_tags = collect_tidal_candidate_genres(&value.extra);
    ExternalTidalCandidate {
        tidal_id: value.id,
        artist_name: value.artist.name,
        title: value.title,
        genre_tags,
        duration_ms: Some(value.duration.saturating_mul(1000)),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum TidalSearchResolutionDecision {
    Resolved(ExternalTidalCandidate),
    Rejected,
    Ambiguous,
    NoResult,
}

fn classify_tidal_search_resolution(
    title: &str,
    artist_name: &str,
    duration_ms: Option<i64>,
    results: &[TidalSearchTrack],
) -> TidalSearchResolutionDecision {
    if results.is_empty() {
        return TidalSearchResolutionDecision::NoResult;
    }

    let title_norm = normalize_resolution_text(title);
    let artist_norm = normalize_resolution_text(artist_name);
    let strong = results
        .iter()
        .filter(|track| {
            let Some(result_artist) = track.artist_name.as_deref() else {
                return false;
            };
            normalize_resolution_text(&track.title) == title_norm
                && normalize_resolution_text(result_artist) == artist_norm
                && duration_is_close(duration_ms, Some(track.duration.saturating_mul(1000)))
        })
        .cloned()
        .collect::<Vec<_>>();

    match strong.len() {
        0 => TidalSearchResolutionDecision::Rejected,
        1 => TidalSearchResolutionDecision::Resolved(ExternalTidalCandidate::from(
            strong.into_iter().next().expect("one strong match"),
        )),
        _ => TidalSearchResolutionDecision::Ambiguous,
    }
}

fn normalize_resolution_text(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn duration_is_close(left_ms: Option<i64>, right_ms: Option<i64>) -> bool {
    match (left_ms, right_ms) {
        (Some(left), Some(right)) if left > 0 && right > 0 => {
            let delta = (left - right).abs();
            let tolerance = 10_000.max((left.max(right) as f64 * 0.05).round() as i64);
            delta <= tolerance
        }
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExternalProviderRefreshReport {
    pub seed_tracks_scanned: usize,
    pub lastfm_rows_seen: usize,
    pub tidal_new_release_rows_seen: usize,
    pub candidates_upserted: usize,
    pub sightings_upserted: usize,
    pub lastfm_candidates_upserted: usize,
    pub lastfm_sightings_upserted: usize,
    pub lastfm_skipped_rows: usize,
    pub tidal_new_release_candidates_upserted: usize,
    pub tidal_new_release_sightings_upserted: usize,
    pub tidal_new_release_skipped_rows: usize,
    pub tidal_similar_rows_seen: usize,
    pub tidal_similar_candidates_upserted: usize,
    pub tidal_similar_sightings_upserted: usize,
    pub tidal_similar_provider_errors: usize,
    pub skipped_rows: usize,
    pub rate_limited: bool,
    pub cooldown_ms: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExternalTidalResolutionReport {
    pub attempted: usize,
    pub resolved: usize,
    pub rejected: usize,
    pub ambiguous: usize,
    pub no_result: usize,
    pub provider_errors: usize,
    pub playable_before: i64,
    pub playable_after: i64,
    pub rate_limited: bool,
    pub cooldown_ms: u64,
}

pub fn plan_external_provider_refresh(
    full_mode: bool,
    last_refresh_at: Option<chrono::NaiveDateTime>,
    available_seed_tracks: usize,
) -> ExternalProviderRefreshBudget {
    plan_external_provider_refresh_at(
        full_mode,
        last_refresh_at,
        available_seed_tracks,
        chrono::Utc::now().naive_utc(),
    )
}

fn plan_external_provider_refresh_at(
    full_mode: bool,
    last_refresh_at: Option<chrono::NaiveDateTime>,
    available_seed_tracks: usize,
    now: chrono::NaiveDateTime,
) -> ExternalProviderRefreshBudget {
    let stale = last_refresh_at
        .map(|last| now.signed_duration_since(last).num_hours() >= EXTERNAL_REFRESH_STALE_HOURS)
        .unwrap_or(true);
    if !full_mode && !stale {
        return ExternalProviderRefreshBudget {
            should_refresh: false,
            seed_tracks: 0,
            lastfm_rows_per_seed: 0,
            tidal_new_release_rows: 0,
        };
    }
    ExternalProviderRefreshBudget {
        should_refresh: true,
        seed_tracks: available_seed_tracks.min(EXTERNAL_REFRESH_MAX_SEED_TRACKS),
        lastfm_rows_per_seed: EXTERNAL_REFRESH_LASTFM_ROWS_PER_SEED,
        tidal_new_release_rows: EXTERNAL_REFRESH_TIDAL_NEW_RELEASE_ROWS,
    }
}

pub async fn refresh_external_provider_candidates(
    db: &Database,
    clients: &ExternalProviderRefreshClients,
    full_mode: bool,
    progress_tx: Option<&mpsc::UnboundedSender<TrainingProgressUpdate>>,
    progress_range: (f32, f32),
) -> Result<ExternalProviderRefreshReport> {
    // Split the corpus sub-range across the two network phases so each can tick
    // independently: first half is Last.fm, second half is TIDAL similar.
    let (progress_start, progress_end) = progress_range;
    let progress_mid = progress_start + (progress_end - progress_start) * 0.5;

    let last_refresh_at = load_external_provider_last_refresh(db)?;
    let seed_rows = db.with_conn(queries::get_embedding_track_rows)?;
    let budget = plan_external_provider_refresh(full_mode, last_refresh_at, seed_rows.len());
    if !budget.should_refresh {
        return Ok(ExternalProviderRefreshReport::default());
    }
    if clients.lastfm.is_none() && clients.tidal.is_none() {
        return Ok(ExternalProviderRefreshReport::default());
    }

    let seed_rows = seed_rows
        .into_iter()
        .take(budget.seed_tracks)
        .collect::<Vec<_>>();
    let mut lastfm_rows: HashMap<i64, Vec<ExternalLastfmCandidate>> = HashMap::new();
    if let Some(lastfm) = clients.lastfm.as_ref() {
        let lastfm_total = seed_rows.len().max(1) as f32;
        for (index, seed) in seed_rows.iter().enumerate() {
            let Some(artist) = seed
                .artist_name
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            else {
                continue;
            };
            if index > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    EXTERNAL_REFRESH_LASTFM_DELAY_MS,
                ))
                .await;
            }
            // Tick every ~10 seeds (and on the first): Last.fm getSimilar with
            // the 500ms cooldown can park the bar for 30-60s without feedback.
            if let Some(tx) = progress_tx
                && (index == 0 || index.is_multiple_of(10))
            {
                let frac = index as f32 / lastfm_total;
                let p = progress_start + (progress_mid - progress_start) * frac;
                let _ = tx.send(TrainingProgressUpdate::stage_only(
                    "corpus",
                    &format!("Tracing similar tracks ({}/{})", index, seed_rows.len()),
                    p,
                ));
            }
            match lastfm
                .track_get_similar(artist, &seed.title, budget.lastfm_rows_per_seed)
                .await
            {
                Ok(rows) => {
                    let direct_rows = rows
                        .into_iter()
                        .take(budget.lastfm_rows_per_seed)
                        .map(ExternalLastfmCandidate::from)
                        .collect::<Vec<_>>();
                    let branch_rows = if index < EXTERNAL_REFRESH_LASTFM_BRANCH_SEED_TRACKS {
                        refresh_lastfm_branch_rows(lastfm, seed.track_id, &direct_rows).await
                    } else {
                        Vec::new()
                    };
                    let mut rows = direct_rows;
                    rows.extend(branch_rows);
                    lastfm_rows.insert(seed.track_id, rows);
                }
                Err(error) => {
                    if is_provider_rate_limit_error(&error) {
                        tracing::warn!(
                            target: "noor.discovery.external",
                            seed_track_id = seed.track_id,
                            error = %error,
                            "Last.fm similar refresh hit rate limit"
                        );
                        let mut report = db.with_conn(|conn| {
                            persist_external_provider_refresh(
                                conn,
                                &seed_rows,
                                &lastfm_rows,
                                &[],
                                &HashMap::new(),
                                chrono::Utc::now().naive_utc(),
                            )
                        })?;
                        report.rate_limited = true;
                        report.cooldown_ms = EXTERNAL_REFRESH_LASTFM_RATE_LIMIT_COOLDOWN_MS;
                        return Ok(report);
                    }
                    tracing::warn!(
                        target: "noor.discovery.external",
                        seed_track_id = seed.track_id,
                        error = %error,
                        "Last.fm similar refresh failed"
                    );
                }
            }
        }
    }

    let tidal_rows = if let Some(tidal) = clients.tidal.as_ref() {
        match tidal
            .get_editorial_top_tracks(budget.tidal_new_release_rows as i32)
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .take(budget.tidal_new_release_rows)
                .map(ExternalTidalCandidate::from)
                .collect::<Vec<_>>(),
            Err(error) => {
                tracing::warn!(
                    target: "noor.discovery.external",
                    error = %error,
                    "TIDAL new-release refresh failed"
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let mut tidal_similar_provider_errors = 0usize;
    let mut tidal_similar_by_seed: HashMap<i64, Vec<ExternalTidalCandidate>> = HashMap::new();
    if let Some(tidal) = clients.tidal.as_ref() {
        let seed_limit = budget
            .seed_tracks
            .min(EXTERNAL_REFRESH_TIDAL_SIMILAR_SEED_TRACKS) as i64;
        let tidal_seed_rows = db
            .with_conn(|conn| queries::get_tidal_similar_seed_rows(conn, seed_limit))
            .unwrap_or_default();
        let tidal_total = tidal_seed_rows.len().max(1) as f32;
        let tidal_seed_count = tidal_seed_rows.len();
        'seed_loop: for (index, seed) in tidal_seed_rows.into_iter().enumerate() {
            // Tick on every seed: the cap is EXTERNAL_REFRESH_TIDAL_SIMILAR_SEED_TRACKS
            // (10), so each iteration is meaningful progress (~1.3% of the corpus span).
            if let Some(tx) = progress_tx {
                let frac = index as f32 / tidal_total;
                let p = progress_mid + (progress_end - progress_mid) * frac;
                let _ = tx.send(TrainingProgressUpdate::stage_only(
                    "corpus",
                    &format!(
                        "Mapping artist constellations ({}/{})",
                        index, tidal_seed_count
                    ),
                    p,
                ));
            }
            let similar_artists = match tidal
                .get_artist_similar(
                    seed.artist_tidal_id,
                    EXTERNAL_REFRESH_TIDAL_SIMILAR_ARTISTS_PER_SEED,
                    0,
                )
                .await
            {
                Ok(rows) => rows.items,
                Err(error) => {
                    tidal_similar_provider_errors += 1;
                    if is_provider_rate_limit_error(&error) {
                        tracing::warn!(
                            target: "noor.discovery.external",
                            seed_track_id = seed.track_id,
                            error = %error,
                            "TIDAL similar refresh hit rate limit"
                        );
                        break;
                    }
                    tracing::warn!(
                        target: "noor.discovery.external",
                        seed_track_id = seed.track_id,
                        error = %error,
                        "TIDAL similar artist refresh failed"
                    );
                    continue;
                }
            };
            for artist in similar_artists
                .into_iter()
                .take(EXTERNAL_REFRESH_TIDAL_SIMILAR_ARTISTS_PER_SEED as usize)
            {
                match tidal
                    .get_artist_top_tracks(
                        artist.id,
                        EXTERNAL_REFRESH_TIDAL_SIMILAR_TRACKS_PER_ARTIST,
                        0,
                    )
                    .await
                {
                    Ok(rows) => {
                        tidal_similar_by_seed
                            .entry(seed.track_id)
                            .or_default()
                            .extend(
                                rows.items
                                    .into_iter()
                                    .map(external_tidal_candidate_from_track),
                            );
                    }
                    Err(error) => {
                        tidal_similar_provider_errors += 1;
                        if is_provider_rate_limit_error(&error) {
                            tracing::warn!(
                                target: "noor.discovery.external",
                                seed_track_id = seed.track_id,
                                artist_tidal_id = artist.id,
                                error = %error,
                                "TIDAL similar top-tracks refresh hit rate limit"
                            );
                            break 'seed_loop;
                        }
                        tracing::warn!(
                            target: "noor.discovery.external",
                            seed_track_id = seed.track_id,
                            artist_tidal_id = artist.id,
                            error = %error,
                            "TIDAL similar top-tracks refresh failed"
                        );
                    }
                }
            }
        }
    }

    let mut report = db.with_conn(|conn| {
        persist_external_provider_refresh(
            conn,
            &seed_rows,
            &lastfm_rows,
            &tidal_rows,
            &tidal_similar_by_seed,
            chrono::Utc::now().naive_utc(),
        )
    })?;
    report.tidal_similar_provider_errors = tidal_similar_provider_errors;
    Ok(report)
}

async fn refresh_lastfm_branch_rows(
    lastfm: &LastFmClient,
    seed_track_id: i64,
    direct_rows: &[ExternalLastfmCandidate],
) -> Vec<ExternalLastfmCandidate> {
    let mut branch_rows = Vec::new();
    let mut branch_seen = direct_rows
        .iter()
        .map(|row| lastfm_candidate_key(&row.artist, &row.title))
        .collect::<HashSet<_>>();
    for parent in direct_rows
        .iter()
        .filter(|row| row.match_score >= EXTERNAL_REFRESH_LASTFM_BRANCH_MIN_PARENT_MATCH)
        .take(EXTERNAL_REFRESH_LASTFM_BRANCHES_PER_SEED)
    {
        tokio::time::sleep(std::time::Duration::from_millis(
            EXTERNAL_REFRESH_LASTFM_DELAY_MS,
        ))
        .await;
        let rows = match lastfm
            .track_get_similar(
                &parent.artist,
                &parent.title,
                EXTERNAL_REFRESH_LASTFM_BRANCH_ROWS,
            )
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(
                    target: "noor.discovery.external",
                    seed_track_id,
                    parent_artist = %parent.artist,
                    parent_title = %parent.title,
                    error = %error,
                    "Last.fm branch refresh failed"
                );
                continue;
            }
        };
        for row in rows.into_iter().take(EXTERNAL_REFRESH_LASTFM_BRANCH_ROWS) {
            let key = lastfm_candidate_key(&row.artist, &row.title);
            if branch_seen.insert(key) {
                branch_rows.push(ExternalLastfmCandidate::branch_from(row, parent));
            }
        }
    }
    branch_rows
}

fn lastfm_candidate_key(artist: &str, title: &str) -> String {
    format!(
        "{}\u{1f}{}",
        artist.trim().to_ascii_lowercase(),
        title.trim().to_ascii_lowercase()
    )
}

pub async fn resolve_external_tidal_candidates(
    db: &Database,
    tidal: Option<&TidalClient>,
    full_mode: bool,
    progress_tx: Option<&mpsc::UnboundedSender<TrainingProgressUpdate>>,
    progress_range: (f32, f32),
) -> Result<ExternalTidalResolutionReport> {
    let (progress_start, progress_end) = progress_range;
    let mut report = ExternalTidalResolutionReport {
        playable_before: db.with_conn(queries::count_playable_external_candidates)?,
        ..ExternalTidalResolutionReport::default()
    };
    let Some(tidal) = tidal else {
        report.playable_after = report.playable_before;
        return Ok(report);
    };

    let limit = if full_mode {
        EXTERNAL_TIDAL_RESOLUTION_FULL_LIMIT
    } else {
        EXTERNAL_TIDAL_RESOLUTION_INCREMENTAL_LIMIT
    };
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let candidates = db.with_conn(|conn| {
        queries::get_unresolved_lastfm_external_candidates_for_tidal_resolution(conn, &now, limit)
    })?;
    let mut failed_keys = HashSet::new();
    let candidate_total = candidates.len().max(1) as f32;
    let candidate_count = candidates.len();

    for (index, candidate) in candidates.into_iter().enumerate() {
        // Tick every ~10 candidates: TIDAL search latency varies and this loop
        // can chew through hundreds of items on a full refresh.
        if let Some(tx) = progress_tx
            && (index == 0 || index.is_multiple_of(10))
        {
            let frac = index as f32 / candidate_total;
            let p = progress_start + (progress_end - progress_start) * frac;
            let _ = tx.send(TrainingProgressUpdate::stage_only(
                "corpus",
                &format!("Resolving external matches ({}/{})", index, candidate_count),
                p,
            ));
        }

        let failure_key = format!(
            "{}\u{1f}{}",
            normalize_resolution_text(&candidate.artist_name),
            normalize_resolution_text(&candidate.title)
        );
        if failed_keys.contains(&failure_key) {
            continue;
        }

        report.attempted += 1;
        let query = format!("{} {}", candidate.artist_name, candidate.title);
        let results = match cached_tidal_track_search(
            db,
            tidal,
            &query,
            EXTERNAL_TIDAL_RESOLUTION_SEARCH_LIMIT,
        )
        .await
        {
            Ok(results) => results,
            Err(error) => {
                report.provider_errors += 1;
                failed_keys.insert(failure_key);
                if is_provider_rate_limit_error(&error) {
                    report.rate_limited = true;
                    report.cooldown_ms = EXTERNAL_REFRESH_LASTFM_RATE_LIMIT_COOLDOWN_MS;
                    tracing::warn!(
                        target: "noor.discovery.external",
                        candidate_id = candidate.id,
                        error = %error,
                        "TIDAL candidate resolution hit rate limit"
                    );
                    break;
                }
                tracing::warn!(
                    target: "noor.discovery.external",
                    candidate_id = candidate.id,
                    error = %error,
                    "TIDAL candidate resolution failed"
                );
                continue;
            }
        };

        match classify_tidal_search_resolution(
            &candidate.title,
            &candidate.artist_name,
            candidate.duration_ms,
            &results,
        ) {
            TidalSearchResolutionDecision::Resolved(match_candidate) => {
                let genre_tags_json = if match_candidate.genre_tags.is_empty() {
                    None
                } else {
                    Some(serde_json::to_string(&match_candidate.genre_tags)?)
                };
                db.with_conn(|conn| {
                    queries::resolve_external_candidate_tidal_metadata(
                        conn,
                        candidate.id,
                        &queries::ExternalCandidateTidalResolution {
                            tidal_id: match_candidate.tidal_id,
                            genre_tags_json,
                            duration_ms: match_candidate.duration_ms,
                        },
                    )
                })?;
                report.resolved += 1;
            }
            TidalSearchResolutionDecision::Rejected => {
                report.rejected += 1;
                failed_keys.insert(failure_key);
            }
            TidalSearchResolutionDecision::Ambiguous => {
                report.ambiguous += 1;
                failed_keys.insert(failure_key);
            }
            TidalSearchResolutionDecision::NoResult => {
                report.no_result += 1;
                failed_keys.insert(failure_key);
            }
        }
    }

    report.playable_after = db.with_conn(queries::count_playable_external_candidates)?;
    Ok(report)
}

async fn cached_tidal_track_search(
    db: &Database,
    tidal: &TidalClient,
    query: &str,
    limit: i32,
) -> Result<Vec<TidalSearchTrack>> {
    if let Some(cached) = db.with_conn(|conn| {
        crate::services::tidal::cache::get_search(
            conn,
            &crate::services::tidal::cache::TidalSearchCacheConfig::default(),
            query,
            limit,
            0,
        )
    })? {
        return Ok(cached.tracks);
    }

    let catalog = tidal.search_catalog(query, limit, 0).await?;
    db.with_conn(|conn| crate::services::tidal::cache::put_search(conn, query, limit, 0, &catalog))
        .unwrap_or_else(|error| {
            tracing::warn!("tidal_search_cache write failed during external resolution: {error}");
        });
    Ok(catalog.tracks)
}

fn is_provider_rate_limit_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let text = cause.to_string().to_ascii_lowercase();
        text.contains("429") || text.contains("rate limit") || text.contains("too many requests")
    })
}

pub fn persist_external_provider_refresh(
    conn: &rusqlite::Connection,
    seeds: &[EmbeddingTrackRow],
    lastfm_by_seed: &HashMap<i64, Vec<ExternalLastfmCandidate>>,
    tidal_candidates: &[ExternalTidalCandidate],
    tidal_similar_by_seed: &HashMap<i64, Vec<ExternalTidalCandidate>>,
    now: chrono::NaiveDateTime,
) -> Result<ExternalProviderRefreshReport> {
    let expires_at = now + chrono::Duration::days(30);
    let expires_at = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();
    let now_string = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let mut report = ExternalProviderRefreshReport {
        seed_tracks_scanned: seeds.len(),
        lastfm_rows_seen: lastfm_by_seed.values().map(Vec::len).sum(),
        tidal_new_release_rows_seen: tidal_candidates.len(),
        tidal_similar_rows_seen: tidal_similar_by_seed.values().map(Vec::len).sum(),
        candidates_upserted: 0,
        sightings_upserted: 0,
        skipped_rows: 0,
        rate_limited: false,
        cooldown_ms: 0,
        ..ExternalProviderRefreshReport::default()
    };

    let seed_ids = seeds
        .iter()
        .map(|seed| seed.track_id)
        .collect::<HashSet<_>>();
    for (&seed_track_id, rows) in lastfm_by_seed {
        if !seed_ids.contains(&seed_track_id) {
            report.skipped_rows += rows.len();
            report.lastfm_skipped_rows += rows.len();
            continue;
        }
        for row in rows {
            if row.title.trim().is_empty() || row.artist.trim().is_empty() {
                report.skipped_rows += 1;
                report.lastfm_skipped_rows += 1;
                continue;
            }
            let candidate = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: None,
                    mbid: row.mbid.clone(),
                    dedupe_key: external_candidate_dedupe_key(
                        None,
                        row.mbid.as_deref(),
                        &row.artist,
                        &row.title,
                        None,
                    ),
                    title: row.title.clone(),
                    artist_name: row.artist.clone(),
                    genre_tags_json: None,
                    duration_ms: None,
                    expires_at: expires_at.clone(),
                },
            )?;
            report.candidates_upserted += 1;
            report.lastfm_candidates_upserted += 1;
            queries::upsert_external_candidate_sighting(
                conn,
                &queries::ExternalCandidateSightingUpsert {
                    candidate_id: candidate.id,
                    seed_track_id,
                    source: "lastfm_similar".to_string(),
                    source_payload_json: Some(
                        serde_json::json!({
                            "match_score": row.match_score,
                            "mbid": row.mbid,
                            "branch_from": row.branch_from,
                        })
                        .to_string(),
                    ),
                    similarity: Some(row.match_score),
                    expires_at: expires_at.clone(),
                },
            )?;
            report.sightings_upserted += 1;
            report.lastfm_sightings_upserted += 1;
        }
    }

    let tidal_seed_track_id = seeds.first().map(|seed| seed.track_id);
    for row in tidal_candidates {
        if row.tidal_id <= 0 || row.title.trim().is_empty() || row.artist_name.trim().is_empty() {
            report.skipped_rows += 1;
            report.tidal_new_release_skipped_rows += 1;
            continue;
        }
        let genre_tags_json = if row.genre_tags.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&row.genre_tags)?)
        };
        let candidate = queries::upsert_external_track_candidate(
            conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(row.tidal_id),
                mbid: None,
                dedupe_key: external_candidate_dedupe_key(
                    Some(row.tidal_id),
                    None,
                    &row.artist_name,
                    &row.title,
                    row.duration_ms,
                ),
                title: row.title.clone(),
                artist_name: row.artist_name.clone(),
                genre_tags_json,
                duration_ms: row.duration_ms,
                expires_at: expires_at.clone(),
            },
        )?;
        report.candidates_upserted += 1;
        report.tidal_new_release_candidates_upserted += 1;
        if let Some(seed_track_id) = tidal_seed_track_id {
            queries::upsert_external_candidate_sighting(
                conn,
                &queries::ExternalCandidateSightingUpsert {
                    candidate_id: candidate.id,
                    seed_track_id,
                    source: "tidal_new_release".to_string(),
                    source_payload_json: Some(
                        serde_json::json!({
                            "tidal_id": row.tidal_id,
                        })
                        .to_string(),
                    ),
                    similarity: None,
                    expires_at: expires_at.clone(),
                },
            )?;
            report.sightings_upserted += 1;
            report.tidal_new_release_sightings_upserted += 1;
        }
    }

    for (&seed_track_id, rows) in tidal_similar_by_seed {
        if !seed_ids.contains(&seed_track_id) {
            report.skipped_rows += rows.len();
            continue;
        }
        for row in rows {
            if row.tidal_id <= 0 || row.title.trim().is_empty() || row.artist_name.trim().is_empty()
            {
                report.skipped_rows += 1;
                continue;
            }
            let genre_tags_json = if row.genre_tags.is_empty() {
                None
            } else {
                Some(serde_json::to_string(&row.genre_tags)?)
            };
            let candidate = queries::upsert_external_track_candidate(
                conn,
                &queries::ExternalTrackCandidateUpsert {
                    tidal_id: Some(row.tidal_id),
                    mbid: None,
                    dedupe_key: external_candidate_dedupe_key(
                        Some(row.tidal_id),
                        None,
                        &row.artist_name,
                        &row.title,
                        row.duration_ms,
                    ),
                    title: row.title.clone(),
                    artist_name: row.artist_name.clone(),
                    genre_tags_json,
                    duration_ms: row.duration_ms,
                    expires_at: expires_at.clone(),
                },
            )?;
            report.candidates_upserted += 1;
            report.tidal_similar_candidates_upserted += 1;
            queries::upsert_external_candidate_sighting(
                conn,
                &queries::ExternalCandidateSightingUpsert {
                    candidate_id: candidate.id,
                    seed_track_id,
                    source: "tidal_similar".to_string(),
                    source_payload_json: Some(
                        serde_json::json!({
                            "tidal_id": row.tidal_id,
                        })
                        .to_string(),
                    ),
                    similarity: None,
                    expires_at: expires_at.clone(),
                },
            )?;
            report.sightings_upserted += 1;
            report.tidal_similar_sightings_upserted += 1;
        }
    }

    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('discovery_external_refresh_at', ?1)",
        rusqlite::params![now_string],
    )?;
    Ok(report)
}

fn external_candidate_dedupe_key(
    tidal_id: Option<i64>,
    mbid: Option<&str>,
    artist_name: &str,
    title: &str,
    duration_ms: Option<i64>,
) -> String {
    if let Some(tidal_id) = tidal_id.filter(|id| *id > 0) {
        return format!("tidal:{tidal_id}");
    }
    if let Some(mbid) = mbid.map(str::trim).filter(|value| !value.is_empty()) {
        return format!("mbid:{}", mbid.to_ascii_lowercase());
    }
    format!(
        "text:{}:{}:{}",
        normalize_external_component(artist_name),
        normalize_external_component(title),
        duration_ms.map(|value| value / 30_000).unwrap_or(0)
    )
}

fn normalize_external_component(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn collect_tidal_candidate_genres(extra: &HashMap<String, serde_json::Value>) -> Vec<String> {
    let mut tags = Vec::new();
    for key in ["genre", "genres", "category", "categories"] {
        if let Some(value) = extra.get(key) {
            collect_json_strings(value, &mut tags);
        }
    }
    tags.sort();
    tags.dedup();
    tags.truncate(8);
    tags
}

fn collect_json_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) if !text.trim().is_empty() => {
            out.push(text.trim().to_string());
        }
        serde_json::Value::Array(values) => {
            for item in values {
                collect_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for key in ["name", "title", "label"] {
                if let Some(value) = map.get(key) {
                    collect_json_strings(value, out);
                }
            }
        }
        _ => {}
    }
}

// Training intensity tier. Drives the cost/quality knobs the user picked in
// settings. Higher tiers train a richer model at the cost of CPU time and
// peak RAM; lower tiers stay responsive on weaker hardware. Persisted as a
// string in `server_config["discovery_intensity"]` so it survives restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryIntensity {
    Max,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy)]
pub struct IntensityParams {
    pub dimension: i32,
    pub top_k: usize,
    pub window_size: usize,
    pub include_audio_proxy: bool,
}

impl DiscoveryIntensity {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "max" => DiscoveryIntensity::Max,
            "low" => DiscoveryIntensity::Low,
            _ => DiscoveryIntensity::Medium,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryIntensity::Max => "max",
            DiscoveryIntensity::Medium => "medium",
            DiscoveryIntensity::Low => "low",
        }
    }

    // Cost/quality trade-offs per tier:
    //   Max: 96-dim fused, top-64 neighbors, 8-track context window. The
    //            full model. Best radio quality; on a 30k-track library this
    //            is the ~6-12 minute run that motivated the safeguards.
    //   Medium: 64-dim, top-32, window 5. Roughly 50% of Max's wall time.
    //            Indistinguishable in subjective radio quality for libraries
    //            under ~20k tracks; the default.
    //   Low: 48-dim, top-24, window 3, **skips the audio-proxy stage**.
    //            Pure behavioral co-occurrence. Trains in roughly a quarter
    //            of Max's time. Picks degrade for cold tracks (no metadata
    //            contribution to fusion), but the engine stays usable on
    //            modest hardware.
    pub fn params(self) -> IntensityParams {
        match self {
            DiscoveryIntensity::Max => IntensityParams {
                dimension: 96,
                top_k: 64,
                window_size: 8,
                include_audio_proxy: true,
            },
            DiscoveryIntensity::Medium => IntensityParams {
                dimension: 64,
                top_k: 32,
                window_size: 5,
                include_audio_proxy: true,
            },
            DiscoveryIntensity::Low => IntensityParams {
                dimension: 48,
                top_k: 24,
                window_size: 3,
                include_audio_proxy: false,
            },
        }
    }
}

pub fn load_discovery_intensity(db: &Database) -> DiscoveryIntensity {
    db.with_conn(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_intensity'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(raw)
    })
    .ok()
    .flatten()
    .map(|s| DiscoveryIntensity::parse(&s))
    .unwrap_or(DiscoveryIntensity::Medium)
}

pub fn set_discovery_intensity(db: &Database, intensity: DiscoveryIntensity) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_intensity', ?1)",
            rusqlite::params![intensity.as_str()],
        )?;
        Ok(())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryEngine {
    V2,
    V1,
}

impl DiscoveryEngine {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "v1" | "legacy" => DiscoveryEngine::V1,
            _ => DiscoveryEngine::V2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryEngine::V2 => queries::DISCOVERY_ENGINE_V2,
            DiscoveryEngine::V1 => queries::DISCOVERY_ENGINE_V1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DiscoveryEngine::V2 => "V2 recommended",
            DiscoveryEngine::V1 => "V1 legacy",
        }
    }

    pub fn family(self) -> &'static str {
        match self {
            DiscoveryEngine::V2 => queries::DISCOVERY_ENGINE_V2_FAMILY,
            DiscoveryEngine::V1 => queries::DISCOVERY_ENGINE_V1_FAMILY,
        }
    }

    pub fn supports_training(self) -> bool {
        matches!(self, DiscoveryEngine::V2)
    }
}

pub fn load_discovery_engine(db: &Database) -> DiscoveryEngine {
    db.with_conn(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_engine'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(raw)
    })
    .ok()
    .flatten()
    .map(|s| DiscoveryEngine::parse(&s))
    .unwrap_or(DiscoveryEngine::V2)
}

pub fn set_discovery_engine(db: &Database, engine: DiscoveryEngine) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_engine', ?1)",
            rusqlite::params![engine.as_str()],
        )?;
        Ok(())
    })
}

pub fn discovery_training_safety_timeout(intensity: DiscoveryIntensity) -> Duration {
    let seconds = match intensity {
        DiscoveryIntensity::Max => DISCOVERY_TRAINING_TIMEOUT_MAX_SECS,
        DiscoveryIntensity::Medium | DiscoveryIntensity::Low => {
            DISCOVERY_TRAINING_TIMEOUT_STANDARD_SECS
        }
    };
    Duration::from_secs(seconds)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryTrainingSafetyProfile {
    LaptopSafe,
    Balanced,
    Performance,
}

impl DiscoveryTrainingSafetyProfile {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "laptop_safe" | "laptop-safe" | "safe" => DiscoveryTrainingSafetyProfile::LaptopSafe,
            "performance" | "fast" => DiscoveryTrainingSafetyProfile::Performance,
            _ => DiscoveryTrainingSafetyProfile::Balanced,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DiscoveryTrainingSafetyProfile::LaptopSafe => "laptop_safe",
            DiscoveryTrainingSafetyProfile::Balanced => "balanced",
            DiscoveryTrainingSafetyProfile::Performance => "performance",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DiscoveryTrainingSafetyProfile::LaptopSafe => "Laptop-safe",
            DiscoveryTrainingSafetyProfile::Balanced => "Balanced",
            DiscoveryTrainingSafetyProfile::Performance => "Performance",
        }
    }
}

pub fn load_discovery_training_safety_profile(db: &Database) -> DiscoveryTrainingSafetyProfile {
    db.with_conn(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_training_safety_profile'",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(raw)
    })
    .ok()
    .flatten()
    .map(|s| DiscoveryTrainingSafetyProfile::parse(&s))
    .unwrap_or(DiscoveryTrainingSafetyProfile::Balanced)
}

pub fn set_discovery_training_safety_profile(
    db: &Database,
    profile: DiscoveryTrainingSafetyProfile,
) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES ('discovery_training_safety_profile', ?1)",
            rusqlite::params![profile.as_str()],
        )?;
        Ok(())
    })
}

pub fn discovery_training_worker_threads_for_available(
    profile: DiscoveryTrainingSafetyProfile,
    available_threads: usize,
) -> usize {
    match profile {
        DiscoveryTrainingSafetyProfile::LaptopSafe => available_threads
            .saturating_sub(1)
            .max(1)
            .min(DISCOVERY_TRAINING_LAPTOP_MAX_WORKERS),
        DiscoveryTrainingSafetyProfile::Balanced => available_threads
            .saturating_sub(2)
            .max(1)
            .min(DISCOVERY_TRAINING_BALANCED_MAX_WORKERS),
        DiscoveryTrainingSafetyProfile::Performance => available_threads
            .saturating_sub(1)
            .max(1)
            .min(DISCOVERY_TRAINING_PERFORMANCE_MAX_WORKERS),
    }
}

pub fn discovery_training_worker_threads(profile: DiscoveryTrainingSafetyProfile) -> usize {
    let available_threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1);
    discovery_training_worker_threads_for_available(profile, available_threads)
}

fn load_external_provider_last_refresh(db: &Database) -> Result<Option<chrono::NaiveDateTime>> {
    db.with_conn(|conn| {
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_external_refresh_at'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|value| {
            chrono::NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S").ok()
        }))
    })
}

#[derive(Debug, Clone)]
pub struct ActiveLearningModel {
    pub model_id: i64,
    pub model_key: String,
    #[allow(dead_code)]
    pub family: String,
    /// Vector dimension for this trained model. Authoritative for any code
    /// that allocates buffers compared against `vectors`: the legacy 96d
    /// constant is wrong on Medium (64) and Low (48) intensity tiers.
    pub dimension: usize,
    pub vectors: HashMap<i64, Vec<f64>>,
}

pub async fn start_training(
    db: Database,
    event_tx: Sender<AppEvent>,
    full_mode: bool,
    rebuild_audio: bool,
    cancel: Arc<AtomicBool>,
    external_refresh_clients: ExternalProviderRefreshClients,
) -> Result<()> {
    let engine = load_discovery_engine(&db);
    if !engine.supports_training() {
        bail!("legacy discovery engine cannot be trained in this build");
    }
    let intensity = load_discovery_intensity(&db);
    let intensity_params = intensity.params();
    let safety_profile = load_discovery_training_safety_profile(&db);
    let safety_timeout = discovery_training_safety_timeout(intensity);
    let worker_threads = discovery_training_worker_threads(safety_profile);
    let (model, run) = db.with_conn(|conn| {
        let run = queries::create_training_run(conn, None, "corpus", "running")?;
        let model_key = format!("{MODEL_FAMILY}:{}", run.id);
        let config_json = serde_json::json!({
            "mode": if full_mode { "full" } else { "incremental" },
            "rebuild_audio": rebuild_audio,
            "dimension": intensity_params.dimension,
            "top_k": intensity_params.top_k,
            "intensity": intensity.as_str(),
            "engine": engine.as_str(),
            "safety_profile": safety_profile.as_str(),
            "safety_timeout_seconds": safety_timeout.as_secs(),
            "worker_threads": worker_threads,
            "trainer": "rust",
            "trainer_config_version": 2,
            "run_id": run.id,
        })
        .to_string();
        let model = queries::create_embedding_model(
            conn,
            &model_key,
            MODEL_FAMILY,
            intensity_params.dimension,
            "training",
            Some(&config_json),
        )?;
        queries::update_training_run_model(conn, run.id, model.id)?;
        Ok((model, run))
    })?;

    // Progress channel: broadcasts to WebSocket + logs to tracing + mirrors to
    // the training run row. Created here (not after corpus) so the slow corpus
    // stage can emit intermediate progress instead of parking the UI at 5% for
    // minutes while external refresh runs.
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<TrainingProgressUpdate>();
    let run_id = run.id;
    let db_clone = db.clone();
    let event_tx_clone = event_tx.clone();
    let log_task = tokio::spawn(async move {
        while let Some(update) = progress_rx.recv().await {
            tracing::info!(target: "noor.discovery.training", run_id, %update.message, "training progress");

            // Broadcast to WebSocket
            let _ = event_tx_clone.send(AppEvent::TrainingProgress {
                stage: update.stage.clone(),
                progress: update.progress,
                message: update.message.clone(),
                current_track_id: update.current_track_id,
                current_track_title: update.current_track_title,
                tracks_done: update.tracks_done,
                tracks_total: update.tracks_total,
            });

            // Update DB progress
            let _ = db_clone.with_conn(|conn| {
                queries::update_training_run_progress(
                    conn,
                    run_id,
                    &update.stage,
                    "running",
                    update.progress as f64,
                    None,
                    0,
                )
            });
        }
    });

    macro_rules! fail_training_on_err {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(error) => {
                    let error: anyhow::Error = error.into();
                    mark_training_failure(&db, run.id, model.id, &error);
                    return Err(error);
                }
            }
        };
    }

    // If cancel is requested at any stage boundary, mark the run as cancelled
    // and skip remaining persistence + model activation. Callers MUST `return Ok(())`
    // when this returns `Ok(true)`: otherwise a later stage may double-finish the run.
    let watchdog_tripped = Arc::new(AtomicBool::new(false));
    let bail_if_cancelled = |stage: &str| -> Result<bool> {
        if cancel.load(Ordering::Relaxed) {
            if watchdog_tripped.load(Ordering::Relaxed) {
                tracing::warn!(
                    target: "noor.discovery.training",
                    run_id = run.id,
                    stage = stage,
                    timeout_seconds = safety_timeout.as_secs(),
                    "discovery training stopped by laptop safety timeout"
                );
                db.with_conn(|conn| {
                    queries::finish_training_run_with_error(
                        conn,
                        run.id,
                        "cancelled",
                        DISCOVERY_TRAINING_SAFETY_TIMEOUT_MESSAGE,
                    )
                })?;
            } else {
                tracing::info!(
                    target: "noor.discovery.training",
                    run_id = run.id,
                    stage = stage,
                    "discovery training cancelled by user"
                );
                db.with_conn(|conn| queries::finish_training_run(conn, run.id, "cancelled"))?;
            }
            return Ok(true);
        }
        Ok(false)
    };

    // Synchronous DB write so the "corpus 0.05" milestone is durable before any
    // fallible work runs (preserves the invariant that a mid-corpus failure
    // still leaves run.progress == 0.05. See test
    // `start_training_marks_run_and_model_failed_when_setup_errors_after_creation`).
    fail_training_on_err!(db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "corpus", "running", 0.05, None, 0)
    }));
    // Also fan out over the channel so any subscribed WS client sees the start
    // signal without polling.
    let _ = progress_tx.send(TrainingProgressUpdate::stage_only(
        "corpus",
        "Starting corpus build...",
        0.05,
    ));

    let _ = progress_tx.send(TrainingProgressUpdate::stage_only(
        "corpus",
        "Reading listening memory...",
        0.06,
    ));

    // Backfill listen_history columns added in MIGRATION_023, exactly once per
    // database lifetime. The trainer is the natural trigger: sequence-aware
    // features depend on session_id and transition_from_track_id, so we do
    // this before the corpus build runs.
    if let Some(report) =
        fail_training_on_err!(db.with_conn(crate::services::listen_history_backfill::run_if_needed))
    {
        tracing::info!(
            target: "noor.discovery.training",
            rows_updated = report.rows_updated,
            sessions_created = report.sessions_created,
            already_populated = report.already_populated,
            "backfilled listen_history columns",
        );
    }

    let external_last_refresh_at = fail_training_on_err!(load_external_provider_last_refresh(&db));
    let external_refresh_report = match refresh_external_provider_candidates(
        &db,
        &external_refresh_clients,
        full_mode,
        Some(&progress_tx),
        (0.07, 0.15),
    )
    .await
    {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!(
                target: "noor.discovery.external",
                error = %error,
                "external provider refresh failed"
            );
            ExternalProviderRefreshReport::default()
        }
    };
    let external_resolution_report = match resolve_external_tidal_candidates(
        &db,
        external_refresh_clients.tidal.as_ref(),
        full_mode,
        Some(&progress_tx),
        (0.15, 0.18),
    )
    .await
    {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!(
                target: "noor.discovery.external",
                error = %error,
                "external TIDAL resolution failed"
            );
            ExternalTidalResolutionReport {
                playable_before: db
                    .with_conn(queries::count_playable_external_candidates)
                    .unwrap_or_default(),
                playable_after: db
                    .with_conn(queries::count_playable_external_candidates)
                    .unwrap_or_default(),
                provider_errors: 1,
                ..ExternalTidalResolutionReport::default()
            }
        }
    };

    let _ = progress_tx.send(TrainingProgressUpdate::stage_only(
        "corpus",
        "Building trainer input…",
        0.18,
    ));

    // Build trainer input directly from DB (no JSON round-trip). The intensity
    // tier overrides dimension / top_k / window_size and decides whether to
    // include the audio-proxy stage (Low skips it).
    let input = fail_training_on_err!(db.with_conn(|conn| build_trainer_input(
        conn,
        intensity_params,
        full_mode,
        rebuild_audio
    )));
    let provider_budget =
        plan_external_provider_refresh(full_mode, external_last_refresh_at, input.tracks.len());
    let heldout_examples = input.heldout_examples.clone();

    let _ = progress_tx.send(TrainingProgressUpdate::stage_only(
        "behavioral",
        "Hashing behavioral trails…",
        0.20,
    ));

    // Run the trainer directly. No subprocess.
    let progress_tx_clone = progress_tx.clone();
    let cancel_for_trainer = cancel.clone();
    let watchdog_cancel = cancel.clone();
    let watchdog_flag = watchdog_tripped.clone();
    let watchdog_task = tokio::spawn(async move {
        tokio::time::sleep(safety_timeout).await;
        watchdog_flag.store(true, Ordering::Relaxed);
        watchdog_cancel.store(true, Ordering::Relaxed);
    });
    let output_join = tokio::task::spawn_blocking(move || -> Result<_> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_threads)
            .thread_name(|idx| format!("discovery-v2-{idx}"))
            .build()
            .context("create discovery trainer worker pool")?;
        Ok(pool.install(|| {
            run_discovery_training(input, Some(&progress_tx_clone), Some(&cancel_for_trainer))
        }))
    })
    .await
    .context("discovery trainer panicked");
    let output_result = match output_join {
        Ok(output_result) => output_result,
        Err(error) => {
            watchdog_task.abort();
            mark_training_failure(&db, run.id, model.id, &error);
            return Err(error);
        }
    };
    watchdog_task.abort();
    let mut output = fail_training_on_err!(output_result);
    output.metrics.insert(
        "safety.timeout_seconds".to_string(),
        safety_timeout.as_secs() as f64,
    );
    output
        .metrics
        .insert("safety.worker_threads".to_string(), worker_threads as f64);
    output.metrics.insert(
        "external_refresh_budget.should_refresh".to_string(),
        if provider_budget.should_refresh {
            1.0
        } else {
            0.0
        },
    );
    output.metrics.insert(
        "external_refresh_budget.seed_tracks".to_string(),
        provider_budget.seed_tracks as f64,
    );
    output.metrics.insert(
        "external_refresh_budget.lastfm_rows_per_seed".to_string(),
        provider_budget.lastfm_rows_per_seed as f64,
    );
    output.metrics.insert(
        "external_refresh_budget.tidal_new_release_rows".to_string(),
        provider_budget.tidal_new_release_rows as f64,
    );
    output.metrics.insert(
        "external_refresh.seed_tracks_scanned".to_string(),
        external_refresh_report.seed_tracks_scanned as f64,
    );
    output.metrics.insert(
        "external_refresh.lastfm_rows_seen".to_string(),
        external_refresh_report.lastfm_rows_seen as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_new_release_rows_seen".to_string(),
        external_refresh_report.tidal_new_release_rows_seen as f64,
    );
    output.metrics.insert(
        "external_refresh.candidates_upserted".to_string(),
        external_refresh_report.candidates_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.sightings_upserted".to_string(),
        external_refresh_report.sightings_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.skipped_rows".to_string(),
        external_refresh_report.skipped_rows as f64,
    );
    output.metrics.insert(
        "external_refresh.lastfm_similar.candidates_upserted".to_string(),
        external_refresh_report.lastfm_candidates_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.lastfm_similar.sightings_upserted".to_string(),
        external_refresh_report.lastfm_sightings_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.lastfm_similar.skipped_rows".to_string(),
        external_refresh_report.lastfm_skipped_rows as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_new_release.candidates_upserted".to_string(),
        external_refresh_report.tidal_new_release_candidates_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_new_release.sightings_upserted".to_string(),
        external_refresh_report.tidal_new_release_sightings_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_new_release.skipped_rows".to_string(),
        external_refresh_report.tidal_new_release_skipped_rows as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_similar.rows_seen".to_string(),
        external_refresh_report.tidal_similar_rows_seen as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_similar.candidates_upserted".to_string(),
        external_refresh_report.tidal_similar_candidates_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_similar.sightings_upserted".to_string(),
        external_refresh_report.tidal_similar_sightings_upserted as f64,
    );
    output.metrics.insert(
        "external_refresh.tidal_similar.provider_errors".to_string(),
        external_refresh_report.tidal_similar_provider_errors as f64,
    );
    output.metrics.insert(
        "external_refresh.rate_limited".to_string(),
        if external_refresh_report.rate_limited {
            1.0
        } else {
            0.0
        },
    );
    output.metrics.insert(
        "external_refresh.cooldown_ms".to_string(),
        external_refresh_report.cooldown_ms as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.attempted".to_string(),
        external_resolution_report.attempted as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.resolved".to_string(),
        external_resolution_report.resolved as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.rejected".to_string(),
        external_resolution_report.rejected as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.ambiguous".to_string(),
        external_resolution_report.ambiguous as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.no_result".to_string(),
        external_resolution_report.no_result as f64,
    );
    output.metrics.insert(
        "external_resolution.tidal_search.provider_errors".to_string(),
        external_resolution_report.provider_errors as f64,
    );
    output.metrics.insert(
        "external_resolution.playable_before".to_string(),
        external_resolution_report.playable_before as f64,
    );
    output.metrics.insert(
        "external_resolution.playable_after".to_string(),
        external_resolution_report.playable_after as f64,
    );

    // Wait for progress logging to finish
    drop(progress_tx);
    let _ = log_task.await;

    if fail_training_on_err!(bail_if_cancelled("audio")) {
        return Ok(());
    }
    fail_training_on_err!(db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "audio", "running", 0.55, None, 0)
    }));

    let audio_features = output
        .audio_features
        .iter()
        .map(|(&track_id, feature)| {
            (
                track_id,
                feature.feature_version.clone(),
                pack_vector_f64(&feature.vector),
                feature.clip_start_ms,
                feature.clip_duration_ms,
            )
        })
        .collect::<Vec<_>>();

    if fail_training_on_err!(bail_if_cancelled("audio_features")) {
        return Ok(());
    }
    fail_training_on_err!(
        db.with_conn(|conn| queries::replace_track_audio_features(conn, &audio_features))
    );
    fail_training_on_err!(db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "fusion", "running", 0.72, None, 0)
    }));

    let embeddings = output
        .fusion_embeddings
        .iter()
        .map(|(&track_id, vector)| {
            let norm = l2_norm(vector);
            (track_id, pack_vector_f64(vector), norm)
        })
        .collect::<Vec<_>>();
    if fail_training_on_err!(bail_if_cancelled("fusion")) {
        return Ok(());
    }
    fail_training_on_err!(db.with_conn(|conn| queries::replace_track_embeddings(
        conn,
        model.id,
        &embeddings
    )));

    let neighbors = output
        .neighbors
        .iter()
        .map(|neighbor| {
            let reason_objects = neighbor
                .reason_tags
                .iter()
                .map(|key| DiscoveryNeighborReason {
                    key: key.clone(),
                    label: reason_label(key).to_string(),
                    weight: 1.0,
                })
                .collect::<Vec<_>>();
            let reason_json = serde_json::to_string(&reason_objects).ok();
            queries::NeighborWriteRow {
                track_id: neighbor.track_id,
                neighbor_track_id: neighbor.neighbor_track_id,
                rank: neighbor.rank,
                score: neighbor.score,
                behavioral_score: neighbor.behavioral_score,
                audio_score: neighbor.audio_score,
                metadata_score: neighbor.metadata_score,
                reason_json,
                primary_reason: neighbor.primary_reason.clone(),
                confidence: neighbor.confidence,
                support_count: neighbor.support_count,
                support_transition: neighbor.support_transition,
                support_colisten: neighbor.support_colisten,
                support_structure: neighbor.support_structure,
                support_metadata: neighbor.support_metadata,
                candidate_in_degree: neighbor.candidate_in_degree,
                candidate_in_degree_percentile: neighbor.candidate_in_degree_percentile,
                play_count_seed: neighbor.play_count_seed,
                play_count_candidate: neighbor.play_count_candidate,
            }
        })
        .collect::<Vec<_>>();
    if fail_training_on_err!(bail_if_cancelled("neighbors")) {
        return Ok(());
    }
    fail_training_on_err!(db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "neighbors", "running", 0.88, None, 0)?;
        queries::replace_track_neighbors(conn, model.id, &neighbors)?;
        persist_external_neighbors(conn, model.id, &output.external_neighbors)
    }));

    // Persist per-reason hit-rate diagnostics. The trainer's primary_reason
    // tags drive these rows; the next iteration of metadata-bonus tuning reads
    // them to decide whether harmonic_match's 0.14 weight is justified or
    // whether genre_branch should outrank it.
    let reason_rows: Vec<queries::ReasonHitRateRow> = output
        .reason_hit_rates
        .iter()
        .map(|r| queries::ReasonHitRateRow {
            primary_reason: r.primary_reason.clone(),
            impressions: r.impressions,
            hits: r.hits,
            hit_rate: r.hit_rate,
            mean_rank: r.mean_rank,
            mrr_contribution: r.mrr_contribution,
            insufficient_data: r.insufficient_data,
        })
        .collect();
    fail_training_on_err!(db.with_conn(|conn| queries::replace_discovery_diagnostics(
        conn,
        model.id,
        &reason_rows
    )));

    fail_training_on_err!(append_active_baseline_metrics(
        &db,
        &mut output.metrics,
        &heldout_examples
    ));

    let metrics_json = fail_training_on_err!(serde_json::to_string(&output.metrics));
    let coverage = output.metrics.get("coverage_ratio").copied().unwrap_or(0.0);
    let recall = output
        .metrics
        .get("transition_recall_at_10")
        .or_else(|| output.metrics.get("recall_at_10"))
        .copied()
        .unwrap_or(0.0);
    // Thresholds scale with how much real playback signal exists. The strict
    // recall@10 gate is only meaningful when the held-out set is big enough
    // for the metric to be stable: `build_trainer_input` carves held-out
    // pairs from playback_transitions and playlist sequences, so a user with
    // a couple of plays has ≤10 held-out pairs and recall@10 collapses to
    // noise (0% or 14%, neither carries information).
    //
    // Three tiers:
    //   - 0 real plays         → coverage ≥ 0.5 (cold start, recall ignored)
    //   - 1 ≤ plays < 50       → coverage ≥ 0.7 (warm, recall too noisy to gate on)
    //   - ≥ 50 real plays      → coverage ≥ 0.85 ∧ recall ≥ 0.15 (full gate)
    //
    // 50 is the rough point where held-out has ~10+ pairs and a single hit
    // no longer flips the metric by 10pp.
    //
    // Real-play counts MUST come from playback_transitions / listen_history
    // only. Library-derived sequences (album / artist / genre / playlist /
    // favorites) reflect what's been synced, not what's been listened to.
    let playback_seqs = output
        .metrics
        .get("sequence_count.playback_transitions")
        .copied()
        .unwrap_or(0.0);
    let listen_seqs = output
        .metrics
        .get("sequence_count.listen_history")
        .copied()
        .unwrap_or(0.0);
    let playback_evidence = output
        .metrics
        .get("evidence_count.playback_transitions")
        .copied()
        .unwrap_or(0.0);
    let listen_evidence = output
        .metrics
        .get("evidence_count.listen_history")
        .copied()
        .unwrap_or(0.0);
    let real_play_seqs = playback_seqs + listen_seqs + playback_evidence + listen_evidence;
    let baseline_gate = output
        .metrics
        .get("baseline_transition_recall_at_10")
        .is_none_or(|baseline| recall >= *baseline);
    let should_activate = baseline_gate
        && if real_play_seqs >= 50.0 {
            coverage >= 0.85 && recall >= 0.15
        } else if real_play_seqs >= 1.0 {
            coverage >= 0.7
        } else {
            coverage >= 0.5
        };
    if fail_training_on_err!(bail_if_cancelled("evaluate")) {
        return Ok(());
    }
    fail_training_on_err!(db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "evaluate", "running", 0.96, None, 0)?;
        queries::update_embedding_model_metrics(conn, model.id, "ready", Some(&metrics_json))?;
        if should_activate {
            queries::activate_embedding_model(conn, model.id)?;
        }
        queries::finish_training_run(conn, run.id, "completed")
    }));

    Ok(())
}

fn mark_training_failure(db: &Database, run_id: i64, model_id: i64, error: &anyhow::Error) {
    let error_text = error.to_string();
    if let Err(mark_error) = db.with_conn(|conn| {
        queries::fail_training_run(conn, run_id, &error_text)?;
        queries::fail_embedding_model(conn, model_id)
    }) {
        tracing::error!(
            target: "noor.discovery.training",
            run_id,
            model_id,
            original_error = %error,
            mark_error = %mark_error,
            "failed to persist discovery training failure"
        );
    }
}

pub fn load_active_learning_model(db: &Database) -> Result<Option<ActiveLearningModel>> {
    db.with_conn(|conn| {
        let Some(model) = queries::get_selected_discovery_embedding_model(conn)? else {
            return Ok(None);
        };
        let vectors = queries::get_model_embeddings(conn, model.id)?
            .into_iter()
            .map(|row| (row.track_id, unpack_vector_blob(&row.vector_blob)))
            .collect::<HashMap<_, _>>();
        Ok(Some(ActiveLearningModel {
            model_id: model.id,
            model_key: model.model_key,
            family: model.family,
            dimension: model.dimension.max(0) as usize,
            vectors,
        }))
    })
}

pub fn radio_from_neighbors(
    db: &Database,
    seed_track_id: i64,
    exclude_ids: &[i64],
    limit: i64,
    creativity: f64,
) -> Result<Option<Vec<DiscoveryRadioResult>>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(None);
    };
    db.with_conn(|conn| {
        let neighbors = queries::get_track_neighbors(
            conn,
            active.model_id,
            seed_track_id,
            limit * 3,
            exclude_ids,
        )?;
        if neighbors.is_empty() {
            return Ok(Some(Vec::new()));
        }

        let mut rows = neighbors
            .into_iter()
            .map(|neighbor| {
                let adjusted = neighbor.score * (1.0 - creativity.clamp(0.0, 1.0) * 0.35);
                let reasons = parse_reason_tags(neighbor.reason_json.as_deref());
                DiscoveryRadioResult {
                    track_id: neighbor.track_id,
                    title: neighbor.title,
                    artist_name: neighbor.artist_name,
                    album_title: neighbor.album_title,
                    artwork_url: neighbor.artwork_url,
                    duration_ms: neighbor.duration_ms,
                    best_quality: neighbor.best_quality,
                    similarity_score: neighbor.score,
                    adjusted_score: adjusted,
                    co_listen_score: neighbor.behavioral_score,
                    co_album_score: neighbor.metadata_score,
                    co_artist_score: neighbor.audio_score,
                    genre_proximity: neighbor.metadata_score,
                    reason_tags: reasons,
                    model_key: Some(active.model_key.clone()),
                    source_mode: "embedding".to_string(),
                    confidence: neighbor.confidence,
                    support_count: neighbor.support_count,
                    candidate_in_degree: neighbor.candidate_in_degree,
                    candidate_in_degree_percentile: neighbor.candidate_in_degree_percentile,
                    primary_reason: neighbor.primary_reason,
                }
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| {
            right
                .adjusted_score
                .partial_cmp(&left.adjusted_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows.truncate(limit.max(1) as usize);
        Ok(Some(rows))
    })
}

pub fn build_prompt_preview(
    db: &Database,
    prompt: &str,
    mode: &str,
    services: &[String],
    limit: usize,
    recent_tracks: &[TrackSimilarityResult],
) -> Result<Option<DiscoveryPreview>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(None);
    };
    db.with_conn(|conn| {
        let candidates = queries::get_discovery_candidate_tracks(conn, 120)?;
        let tracks = queries::get_embedding_track_rows(conn)?;
        let track_map = tracks
            .into_iter()
            .map(|row| (row.track_id, row))
            .collect::<HashMap<_, _>>();
        let anchor_ids = resolve_prompt_anchor_ids(prompt, &candidates, &track_map);
        if anchor_ids.is_empty() {
            return Ok(None);
        }
        let centroid = centroid_for_ids(&active, &anchor_ids);
        let results = rank_preview_candidates(&active, &candidates, &anchor_ids, &centroid, limit);
        let summary = format!(
            "Learned {} discovery anchored by {} library seed(s).",
            mode,
            anchor_ids.len()
        );
        let profile = DiscoveryProfilePreview {
            prompt: prompt.trim().to_string(),
            mode: mode.to_string(),
            services: services.to_vec(),
            prompt_terms: tokenize(prompt),
            prompt_genres: Vec::new(),
            top_artists: anchor_ids.iter().filter_map(|id| track_map.get(id).and_then(|row| row.artist_name.clone())).take(3).collect(),
            top_genres: Vec::new(),
            recent_tracks: recent_tracks.iter().take(5).map(|row| row.title.clone()).collect(),
            favorite_ratio: 0.0,
            completion_rate: 0.0,
            summary,
        };
        let reasons = vec![
            DiscoveryReason {
                label: "Anchor tracks".to_string(),
                detail: format!("Prompt resolved to {} anchor tracks in your library.", anchor_ids.len()),
                weight: 82,
            },
            DiscoveryReason {
                label: "Embedding centroid".to_string(),
                detail: "Results come from the learned neighborhood around those anchors, not raw text hits.".to_string(),
                weight: 77,
            },
        ];
        Ok(Some(DiscoveryPreview {
            profile,
            reasons,
            results,
        }))
    })
}

pub fn compute_external_embedding_scores(
    db: &Database,
    prompt: &str,
    candidates: &[DiscoveryCandidateTrack],
) -> Result<HashMap<String, f64>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(HashMap::new());
    };
    db.with_conn(|conn| {
        let library_candidates = queries::get_discovery_candidate_tracks(conn, 120)?;
        let track_rows = queries::get_embedding_track_rows(conn)?;
        let track_map = track_rows
            .into_iter()
            .map(|row| (row.track_id, row))
            .collect::<HashMap<_, _>>();
        let anchor_ids = resolve_prompt_anchor_ids(prompt, &library_candidates, &track_map);
        if anchor_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let centroid = centroid_for_ids(&active, &anchor_ids);
        let mut scores = HashMap::new();
        for candidate in candidates {
            let proxy = external_candidate_proxy_vector(candidate, active.dimension);
            scores.insert(
                candidate.provider_track_id.clone(),
                cosine_similarity(&centroid, &proxy),
            );
        }
        Ok(scores)
    })
}

pub fn inject_query_seeds_from_neighbors(
    db: &Database,
    seed_track_id: i64,
    limit: usize,
) -> Result<Vec<String>> {
    let Some(active) = load_active_learning_model(db)? else {
        return Ok(Vec::new());
    };
    db.with_conn(|conn| {
        let neighbors =
            queries::get_track_neighbors(conn, active.model_id, seed_track_id, limit as i64, &[])?;
        let mut queries = Vec::new();
        for neighbor in neighbors {
            if let Some(artist) = neighbor.artist_name {
                queries.push(artist);
            }
            queries.push(neighbor.title);
            if let Some(album) = neighbor.album_title {
                queries.push(album);
            }
        }
        queries.sort();
        queries.dedup();
        Ok(queries)
    })
}

fn stable_edge_bucket(
    event_id: &str,
    from_track_id: i64,
    to_track_id: i64,
    kind: EvidenceKind,
) -> u64 {
    let mut hash = 14_695_981_039_346_656_037u64;
    for byte in event_id
        .bytes()
        .chain(from_track_id.to_le_bytes())
        .chain(to_track_id.to_le_bytes())
        .chain((kind as u8).to_le_bytes())
    {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1_099_511_628_211);
    }
    hash % 10
}

fn trainer_edge_with_heldout(
    event_id: String,
    from_track_id: i64,
    to_track_id: i64,
    weight: f64,
    evidence_kind: EvidenceKind,
) -> (TrainerEdge, Option<HeldoutExample>) {
    let edge = TrainerEdge {
        event_id,
        from_track_id,
        to_track_id,
        weight,
        evidence_kind,
    };
    let heldout = if stable_edge_bucket(
        &edge.event_id,
        edge.from_track_id,
        edge.to_track_id,
        edge.evidence_kind,
    ) == 0
    {
        Some(HeldoutExample {
            event_id: edge.event_id.clone(),
            from_track_id: edge.from_track_id,
            to_track_id: edge.to_track_id,
            evidence_kind: edge.evidence_kind,
            weight: edge.weight,
        })
    } else {
        None
    };
    (edge, heldout)
}

fn transition_evidence_weight(source: Option<&str>, completed_prev: bool, base_weight: f64) -> f64 {
    let source_multiplier = match source.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "manual" | "queue" | "user" | "command" => 1.2,
        "playlist" | "playlist_track" => 1.0,
        "album" | "album_track" => 0.9,
        "radio" | "lastfm" | "lastfm_radio" => 0.75,
        "automix" | "automix-new" | "automix_new" => 0.65,
        "" => 0.85,
        _ => 0.85,
    };
    let completion_multiplier = if completed_prev { 1.0 } else { 0.75 };
    (base_weight * source_multiplier * completion_multiplier).clamp(0.05, 1.5)
}

fn trainer_external_candidates_from_rows(
    rows: Vec<queries::ExternalTrackCandidateRow>,
) -> Vec<TrainerExternalCandidate> {
    rows.into_iter()
        .map(|row| {
            let genre_tags = row
                .genre_tags_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
                .unwrap_or_default();
            let source_tags = row
                .source_tags_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
                .unwrap_or_default();
            TrainerExternalCandidate {
                candidate_id: row.id,
                tidal_id: row.tidal_id,
                title: row.title,
                artist_name: row.artist_name,
                genre_tags,
                source_tags,
                freshness_bucket: external_freshness_bucket(&row.updated_at),
                duration_ms: row.duration_ms,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LastfmRecallKind {
    Direct,
    Branch,
}

fn lastfm_recall_kind(source_payload_json: Option<&str>) -> LastfmRecallKind {
    let has_branch_parent = source_payload_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| {
            value
                .get("branch_from")
                .and_then(|branch| branch.as_str())
                .map(str::trim)
                .map(str::is_empty)
        })
        .is_some_and(|empty| !empty);
    if has_branch_parent {
        LastfmRecallKind::Branch
    } else {
        LastfmRecallKind::Direct
    }
}

fn lastfm_resolved_edges_from_rows(
    rows: Vec<queries::ResolvedLastfmExternalSightingRow>,
) -> Vec<TrainerEdge> {
    let mut grouped: HashMap<(i64, i64, LastfmRecallKind), f64> = HashMap::new();
    for row in rows {
        let kind = lastfm_recall_kind(row.source_payload_json.as_deref());
        let weight = row.similarity.clamp(0.0, 1.0)
            * match kind {
                LastfmRecallKind::Direct => LASTFM_DIRECT_EDGE_WEIGHT,
                LastfmRecallKind::Branch => LASTFM_BRANCH_EDGE_WEIGHT,
            };
        grouped
            .entry((row.seed_track_id, row.resolved_track_id, kind))
            .and_modify(|existing| {
                if weight > *existing {
                    *existing = weight;
                }
            })
            .or_insert(weight);
    }

    grouped
        .into_iter()
        .map(
            |((seed_track_id, resolved_track_id, kind), weight)| TrainerEdge {
                event_id: format!(
                    "lastfm-resolved:{}:{}:{}",
                    seed_track_id,
                    resolved_track_id,
                    match kind {
                        LastfmRecallKind::Direct => "direct",
                        LastfmRecallKind::Branch => "branch",
                    }
                ),
                from_track_id: seed_track_id,
                to_track_id: resolved_track_id,
                weight,
                evidence_kind: match kind {
                    LastfmRecallKind::Direct => EvidenceKind::LastfmDirectSimilarity,
                    LastfmRecallKind::Branch => EvidenceKind::LastfmBranchSimilarity,
                },
            },
        )
        .collect()
}

fn external_freshness_bucket(updated_at: &str) -> Option<String> {
    let updated = chrono::NaiveDateTime::parse_from_str(updated_at, "%Y-%m-%d %H:%M:%S").ok()?;
    let now = chrono::Utc::now().naive_utc();
    let age_days = now.signed_duration_since(updated).num_days();
    let bucket = if age_days <= 7 {
        "fresh_7d"
    } else if age_days <= 30 {
        "fresh_30d"
    } else {
        "fresh_old"
    };
    Some(bucket.to_string())
}

fn persist_external_neighbors(
    conn: &rusqlite::Connection,
    model_id: i64,
    neighbors: &[TrainerExternalNeighbor],
) -> Result<()> {
    let mut grouped: HashMap<i64, Vec<queries::ExternalCandidateNeighborWriteRow>> = HashMap::new();
    for neighbor in neighbors {
        let reason_json = if neighbor.reason_tags.is_empty() {
            None
        } else {
            Some(
                serde_json::to_string(
                    &neighbor
                        .reason_tags
                        .iter()
                        .map(|key| serde_json::json!({ "key": key }))
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_default(),
            )
        };
        grouped.entry(neighbor.library_track_id).or_default().push(
            queries::ExternalCandidateNeighborWriteRow {
                candidate_id: neighbor.candidate_id,
                rank: neighbor.rank,
                score: neighbor.score,
                audio_score: neighbor.audio_score,
                metadata_score: neighbor.metadata_score,
                reason_json,
            },
        );
    }
    for (library_track_id, rows) in grouped {
        queries::replace_external_candidate_neighbors(conn, model_id, library_track_id, &rows)?;
    }
    Ok(())
}

fn build_trainer_input(
    conn: &rusqlite::Connection,
    intensity: IntensityParams,
    full_mode: bool,
    rebuild_audio: bool,
) -> Result<TrainerInput> {
    let tracks = queries::get_embedding_track_rows(conn)?;
    let external_candidates =
        trainer_external_candidates_from_rows(queries::get_external_track_candidates_for_training(
            conn,
            &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            EXTERNAL_TRAINING_CANDIDATE_LIMIT,
        )?);
    let playback_transition_rows = queries::get_playback_transition_edges(conn)?;
    let listen_transition_rows = queries::get_listen_history_transition_edges(conn)?;
    let listen_colisten_rows = queries::get_completion_weighted_listen_edges(conn, 45)?;
    let lastfm_resolved_edges = lastfm_resolved_edges_from_rows(
        queries::get_resolved_lastfm_external_sightings_for_training(
            conn,
            &chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            EXTERNAL_TRAINING_RESOLVED_LASTFM_LIMIT,
        )?,
    );
    let playlist_sequences = queries::get_playlist_sequences(conn)?;
    let album_sequences = queries::get_album_sequences(conn)?;
    let artist_sequences = queries::get_artist_sequences(conn)?;
    let genre_sequences = queries::get_genre_sequences(conn)?;
    let favorite_ids = queries::get_favorite_track_ids(conn)?;
    let favorite_sequences = favorite_ids
        .chunks(8)
        .filter(|chunk| chunk.len() > 1)
        .map(|chunk| chunk.to_vec())
        .collect::<Vec<_>>();

    let mut heldout_examples = Vec::new();
    let mut heldout_pairs = Vec::new();
    let playback_transition_edges = playback_transition_rows
        .into_iter()
        .map(|row| {
            let (edge, heldout) = trainer_edge_with_heldout(
                row.event_id,
                row.from_track_id,
                row.to_track_id,
                transition_evidence_weight(
                    row.source.as_deref(),
                    row.completed_prev.unwrap_or(true),
                    row.weight,
                ),
                EvidenceKind::DirectTransition,
            );
            if let Some(example) = heldout {
                heldout_pairs.push((example.from_track_id, example.to_track_id));
                heldout_examples.push(example);
            }
            edge
        })
        .collect::<Vec<_>>();
    let listen_transition_edges = listen_transition_rows
        .into_iter()
        .map(|row| {
            let (edge, heldout) = trainer_edge_with_heldout(
                row.event_id,
                row.from_track_id,
                row.to_track_id,
                transition_evidence_weight(row.source.as_deref(), true, row.weight),
                EvidenceKind::DirectTransition,
            );
            if let Some(example) = heldout {
                heldout_pairs.push((example.from_track_id, example.to_track_id));
                heldout_examples.push(example);
            }
            edge
        })
        .collect::<Vec<_>>();
    let listen_colisten_edges = listen_colisten_rows
        .into_iter()
        .map(|row| {
            let (edge, heldout) = trainer_edge_with_heldout(
                row.event_id,
                row.from_track_id,
                row.to_track_id,
                transition_evidence_weight(row.source.as_deref(), true, row.weight),
                EvidenceKind::SessionCoListen,
            );
            if let Some(example) = heldout {
                heldout_pairs.push((example.from_track_id, example.to_track_id));
                heldout_examples.push(example);
            }
            edge
        })
        .collect::<Vec<_>>();

    // Incremental Refresh: try to hydrate cached audio features from the prior
    // run. We only reuse rows whose stored vector dim matches the current
    // intensity tier: flipping Max to Medium changes the vector size, in which
    // case the cache is invalid and we recompute. None when full_mode is true,
    // the caller requested an audio refit, intensity skips audio entirely
    // (Low), or no cache rows match.
    let cached_audio_features =
        if should_reuse_cached_audio_features(intensity, full_mode, rebuild_audio) {
            let expected_dim = intensity.dimension as usize;
            let expected_track_ids = tracks
                .iter()
                .map(|track| track.track_id)
                .collect::<HashSet<_>>();
            hydrate_cached_audio_features(
                queries::get_cached_audio_features(conn)?,
                expected_dim,
                &expected_track_ids,
            )
        } else {
            None
        };

    Ok(TrainerInput {
        seed: 13,
        dimension: intensity.dimension as usize,
        window_size: intensity.window_size,
        min_count: 1,
        top_k: intensity.top_k,
        include_audio_proxy: intensity.include_audio_proxy,
        tracks,
        external_candidates,
        sequences: vec![
            TrainerSequenceGroup {
                label: "playlist_tracks".to_string(),
                weight: 1.1,
                sequences: playlist_sequences,
            },
            TrainerSequenceGroup {
                label: "album_tracks".to_string(),
                weight: 0.7,
                sequences: album_sequences,
            },
            TrainerSequenceGroup {
                label: "artist_tracks".to_string(),
                weight: 0.35,
                sequences: artist_sequences,
            },
            TrainerSequenceGroup {
                label: "genre_tracks".to_string(),
                weight: 0.3,
                sequences: genre_sequences,
            },
            TrainerSequenceGroup {
                label: "favorites".to_string(),
                weight: 1.2,
                sequences: favorite_sequences,
            },
        ],
        evidence_groups: vec![
            TrainerEvidenceGroup {
                label: "playback_transitions".to_string(),
                base_weight: 2.0,
                edges: playback_transition_edges,
            },
            TrainerEvidenceGroup {
                label: "listen_history".to_string(),
                base_weight: 2.0,
                edges: listen_transition_edges,
            },
            TrainerEvidenceGroup {
                label: "listen_history".to_string(),
                base_weight: 1.3,
                edges: listen_colisten_edges,
            },
            TrainerEvidenceGroup {
                label: "lastfm_resolved".to_string(),
                base_weight: 1.0,
                edges: lastfm_resolved_edges,
            },
        ],
        heldout_pairs,
        heldout_examples,
        cached_audio_features,
    })
}

fn should_reuse_cached_audio_features(
    intensity: IntensityParams,
    full_mode: bool,
    rebuild_audio: bool,
) -> bool {
    !full_mode && !rebuild_audio && intensity.include_audio_proxy
}

fn hydrate_cached_audio_features(
    rows: Vec<queries::CachedAudioFeatureRow>,
    expected_dim: usize,
    expected_track_ids: &HashSet<i64>,
) -> Option<HashMap<i64, crate::services::discovery_trainer::TrainerAudioFeature>> {
    let map: HashMap<i64, crate::services::discovery_trainer::TrainerAudioFeature> = rows
        .into_iter()
        .filter_map(|row| {
            if !expected_track_ids.contains(&row.track_id) {
                return None;
            }
            if row.feature_version != AUDIO_PROXY_FEATURE_VERSION {
                return None;
            }
            let vector = unpack_vector_blob(&row.vector_blob);
            if vector.len() != expected_dim {
                return None;
            }
            Some((
                row.track_id,
                crate::services::discovery_trainer::TrainerAudioFeature {
                    vector,
                    clip_start_ms: row.clip_start_ms,
                    clip_duration_ms: row.clip_duration_ms,
                    feature_version: row.feature_version,
                },
            ))
        })
        .collect();
    if !expected_track_ids.is_empty()
        && map.len() == expected_track_ids.len()
        && expected_track_ids
            .iter()
            .all(|track_id| map.contains_key(track_id))
    {
        Some(map)
    } else {
        None
    }
}

fn centroid_for_ids(active: &ActiveLearningModel, anchor_ids: &[i64]) -> Vec<f64> {
    let dim = active.dimension;
    let mut centroid = vec![0.0; dim];
    let mut count = 0.0;
    for track_id in anchor_ids {
        if let Some(vector) = active.vectors.get(track_id) {
            // Defensive: skip vectors that don't match the model's stated dim.
            // Shouldn't happen post-fix, but a pre-fix DB may have stale rows
            // from a prior intensity tier and a mismatched accumulate would
            // index out-of-range or silently leave the tail at zero.
            if vector.len() != dim {
                continue;
            }
            for (index, value) in vector.iter().enumerate() {
                centroid[index] += value;
            }
            count += 1.0;
        }
    }
    if count > 0.0 {
        for value in &mut centroid {
            *value /= count;
        }
    }
    normalize_vector(&centroid)
}

fn rank_preview_candidates(
    active: &ActiveLearningModel,
    candidates: &[crate::db::models::Track],
    anchor_ids: &[i64],
    centroid: &[f64],
    limit: usize,
) -> Vec<DiscoveryPreviewResult> {
    let anchor_set = anchor_ids.iter().copied().collect::<HashSet<_>>();
    let mut results = candidates
        .iter()
        .filter(|track| !anchor_set.contains(&track.id))
        .filter_map(|track| {
            active.vectors.get(&track.id).map(|vector| {
                let score = (cosine_similarity(centroid, vector).max(0.0) * 100.0).round() as i32;
                DiscoveryPreviewResult {
                    track_id: track.id,
                    title: track.title.clone(),
                    artist_name: track.artist_name.clone(),
                    album_title: track.album_title.clone(),
                    artwork_url: track.artwork_url.clone(),
                    duration_ms: track.duration_ms,
                    service: track.source.clone(),
                    service_track_id: track
                        .tidal_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| track.id.to_string()),
                    score,
                    tags: vec!["embedding centroid".to_string()],
                }
            })
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| right.score.cmp(&left.score));
    results.truncate(limit.max(1));
    results
}

fn resolve_prompt_anchor_ids(
    prompt: &str,
    candidates: &[crate::db::models::Track],
    track_map: &HashMap<i64, EmbeddingTrackRow>,
) -> Vec<i64> {
    let tokens = tokenize(prompt);
    let mut scored = candidates
        .iter()
        .filter_map(|track| {
            let mut score = 0_i32;
            let haystack = format!(
                "{} {} {}",
                track.title,
                track.artist_name.as_deref().unwrap_or_default(),
                track.album_title.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();
            for token in &tokens {
                if haystack.contains(token) {
                    score += 4;
                }
            }
            if let Some(row) = track_map.get(&track.id) {
                for genre in &row.genre_paths {
                    let genre_lower = genre.to_ascii_lowercase();
                    for token in &tokens {
                        if genre_lower.contains(token) {
                            score += 3;
                        }
                    }
                }
            }
            if score > 0 {
                Some((track.id, score))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| right.1.cmp(&left.1));
    scored
        .into_iter()
        .take(3)
        .map(|(track_id, _)| track_id)
        .collect()
}

fn external_candidate_proxy_vector(candidate: &DiscoveryCandidateTrack, dim: usize) -> Vec<f64> {
    let mut tokens = tokenize(&candidate.title);
    if let Some(artist) = candidate.artist_name.as_deref() {
        tokens.extend(tokenize(artist));
    }
    if let Some(album) = candidate.album_title.as_deref() {
        tokens.extend(tokenize(album));
    }
    for tag in candidate
        .raw_genre_hints
        .iter()
        .chain(candidate.lastfm_tags.iter())
        .chain(candidate.discogs_styles.iter())
        .chain(candidate.discogs_genres.iter())
    {
        tokens.extend(tokenize(tag));
    }
    hashed_token_vector(&tokens, dim)
}

pub fn pack_vector_f64(vector: &[f64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        bytes.extend_from_slice(&(*value as f32).to_le_bytes());
    }
    bytes
}

pub fn unpack_vector_blob(blob: &[u8]) -> Vec<f64> {
    blob.chunks_exact(4)
        .map(|chunk| {
            let bytes: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
            f32::from_le_bytes(bytes) as f64
        })
        .collect()
}

fn l2_norm(vector: &[f64]) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

fn normalize_vector(vector: &[f64]) -> Vec<f64> {
    let norm = l2_norm(vector);
    if norm <= 1e-12 {
        return vec![0.0; vector.len()];
    }
    vector.iter().map(|value| value / norm).collect()
}

fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    left.iter()
        .zip(right.iter())
        .map(|(a, b)| a * b)
        .sum::<f64>()
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn hashed_token_vector(tokens: &[String], dim: usize) -> Vec<f64> {
    let mut vector = vec![0.0; dim];
    for token in tokens {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish();
        for offset in 0..4 {
            let bucket = ((hash >> (offset * 8)) as usize) % dim;
            let sign = if ((hash >> (offset * 8 + 1)) & 1) == 0 {
                1.0
            } else {
                -1.0
            };
            vector[bucket] += sign * 0.5;
        }
    }
    normalize_vector(&vector)
}

fn evaluate_stored_neighbors_for_heldout(
    grouped_neighbors: &HashMap<i64, Vec<queries::EmbeddingNeighborRow>>,
    heldout_examples: &[HeldoutExample],
) -> HashMap<String, f64> {
    let mut metrics = HashMap::new();
    let transition_examples = heldout_examples
        .iter()
        .filter(|example| example.evidence_kind == EvidenceKind::DirectTransition)
        .collect::<Vec<_>>();
    if transition_examples.is_empty() {
        return metrics;
    }

    let mut hits = 0usize;
    let mut reciprocal_rank = 0.0f64;
    for example in &transition_examples {
        let ranked = grouped_neighbors
            .get(&example.from_track_id)
            .map(|rows| rows.as_slice())
            .unwrap_or(&[]);
        if ranked
            .iter()
            .take(10)
            .any(|row| row.track_id == example.to_track_id)
        {
            hits += 1;
        }
        if let Some(pos) = ranked
            .iter()
            .take(20)
            .position(|row| row.track_id == example.to_track_id)
        {
            reciprocal_rank += 1.0 / (pos as f64 + 1.0);
        }
    }

    let total = transition_examples.len() as f64;
    metrics.insert("baseline_heldout_count.transition".to_string(), total);
    metrics.insert(
        "baseline_transition_recall_at_10".to_string(),
        hits as f64 / total,
    );
    metrics.insert(
        "baseline_transition_mrr_at_20".to_string(),
        reciprocal_rank / total,
    );
    metrics
}

fn append_active_baseline_metrics(
    db: &Database,
    metrics: &mut HashMap<String, f64>,
    heldout_examples: &[HeldoutExample],
) -> Result<()> {
    let seed_ids = heldout_examples
        .iter()
        .map(|example| example.from_track_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if seed_ids.is_empty() {
        return Ok(());
    }
    let baseline = db.with_conn(|conn| {
        let Some(active) = queries::get_selected_discovery_embedding_model(conn)? else {
            return Ok(None);
        };
        let grouped = queries::get_track_neighbors_for_seeds(conn, active.id, &seed_ids, 20)?;
        Ok(Some((
            active.id,
            evaluate_stored_neighbors_for_heldout(&grouped, heldout_examples),
        )))
    })?;
    let Some((model_id, baseline_metrics)) = baseline else {
        return Ok(());
    };
    metrics.insert("baseline_active_model_id".to_string(), model_id as f64);
    metrics.extend(baseline_metrics);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_audio_features_reject_old_feature_version() {
        let rows = vec![queries::CachedAudioFeatureRow {
            track_id: 1,
            feature_version: "metadata-audio-proxy-v1".to_string(),
            vector_blob: pack_vector_f64(&[0.5, 0.5]),
            clip_start_ms: 0,
            clip_duration_ms: 20_000,
        }];

        let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1]));

        assert!(
            hydrated.is_none(),
            "v1 proxy vectors must not be reused after v2 DSP token expansion"
        );
    }

    #[test]
    fn cached_audio_features_reject_partial_track_coverage() {
        let rows = vec![queries::CachedAudioFeatureRow {
            track_id: 1,
            feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
            vector_blob: pack_vector_f64(&[0.5, 0.5]),
            clip_start_ms: 0,
            clip_duration_ms: 20_000,
        }];

        let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1, 2]));

        assert!(
            hydrated.is_none(),
            "partial audio cache must recompute instead of silently dropping uncached tracks to behavioral-only fusion"
        );
    }

    #[test]
    fn cached_audio_features_ignore_stale_rows_outside_training_corpus() {
        let rows = vec![
            queries::CachedAudioFeatureRow {
                track_id: 1,
                feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
                vector_blob: pack_vector_f64(&[0.5, 0.5]),
                clip_start_ms: 0,
                clip_duration_ms: 20_000,
            },
            queries::CachedAudioFeatureRow {
                track_id: 99,
                feature_version: AUDIO_PROXY_FEATURE_VERSION.to_string(),
                vector_blob: pack_vector_f64(&[0.25, 0.75]),
                clip_start_ms: 0,
                clip_duration_ms: 20_000,
            },
        ];

        let hydrated = hydrate_cached_audio_features(rows, 2, &HashSet::from([1]))
            .expect("stale cache row should not force recompute");

        assert_eq!(hydrated.len(), 1);
        assert!(hydrated.contains_key(&1));
        assert!(!hydrated.contains_key(&99));
    }

    #[test]
    fn cached_audio_features_are_skipped_when_audio_rebuild_requested() {
        let medium = DiscoveryIntensity::Medium.params();
        let low = DiscoveryIntensity::Low.params();

        assert!(should_reuse_cached_audio_features(medium, false, false));
        assert!(!should_reuse_cached_audio_features(medium, false, true));
        assert!(!should_reuse_cached_audio_features(medium, true, false));
        assert!(!should_reuse_cached_audio_features(low, false, false));
    }

    #[test]
    fn stored_neighbor_baseline_uses_same_typed_heldout_examples() {
        let mut grouped = HashMap::new();
        grouped.insert(
            1,
            vec![queries::EmbeddingNeighborRow {
                track_id: 2,
                title: "Target".to_string(),
                artist_name: None,
                album_title: None,
                artwork_url: None,
                duration_ms: None,
                best_quality: None,
                score: 1.0,
                behavioral_score: 1.0,
                audio_score: 0.0,
                metadata_score: 0.0,
                reason_json: None,
                confidence: 1.0,
                support_count: 1,
                support_transition: 1.0,
                support_colisten: 0.0,
                support_structure: 0.0,
                support_metadata: 0.0,
                candidate_in_degree: 0,
                candidate_in_degree_percentile: 0.0,
                play_count_seed: 0,
                play_count_candidate: 0,
                primary_reason: None,
            }],
        );
        let examples = vec![HeldoutExample {
            event_id: "transition:1".to_string(),
            from_track_id: 1,
            to_track_id: 2,
            evidence_kind: EvidenceKind::DirectTransition,
            weight: 1.0,
        }];

        let metrics = evaluate_stored_neighbors_for_heldout(&grouped, &examples);

        assert_eq!(metrics.get("baseline_heldout_count.transition"), Some(&1.0));
        assert_eq!(metrics.get("baseline_transition_recall_at_10"), Some(&1.0));
        assert_eq!(metrics.get("baseline_transition_mrr_at_20"), Some(&1.0));
    }

    #[test]
    fn external_provider_refresh_budget_caps_full_runs() {
        let budget = plan_external_provider_refresh(true, None, 400);

        assert!(budget.should_refresh);
        assert_eq!(budget.seed_tracks, 100);
        assert_eq!(budget.lastfm_rows_per_seed, 20);
        assert_eq!(budget.tidal_new_release_rows, 500);
    }

    #[test]
    fn discovery_engine_defaults_to_v2_and_round_trips_legacy_choice() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.run_migrations().expect("migrations");

        assert_eq!(load_discovery_engine(&db), DiscoveryEngine::V2);

        set_discovery_engine(&db, DiscoveryEngine::V1).expect("set legacy engine");

        assert_eq!(load_discovery_engine(&db), DiscoveryEngine::V1);
        assert_eq!(load_discovery_engine(&db).family(), "discovery-fusion");
        assert!(!load_discovery_engine(&db).supports_training());
    }

    #[test]
    fn training_safety_timeout_scales_by_intensity() {
        assert_eq!(
            discovery_training_safety_timeout(DiscoveryIntensity::Low).as_secs(),
            30 * 60
        );
        assert_eq!(
            discovery_training_safety_timeout(DiscoveryIntensity::Medium).as_secs(),
            30 * 60
        );
        assert_eq!(
            discovery_training_safety_timeout(DiscoveryIntensity::Max).as_secs(),
            60 * 60
        );
    }

    #[test]
    fn training_worker_cap_adapts_by_safety_profile() {
        assert_eq!(
            discovery_training_worker_threads_for_available(
                DiscoveryTrainingSafetyProfile::LaptopSafe,
                16
            ),
            4
        );
        assert_eq!(
            discovery_training_worker_threads_for_available(
                DiscoveryTrainingSafetyProfile::Balanced,
                16
            ),
            8
        );
        assert_eq!(
            discovery_training_worker_threads_for_available(
                DiscoveryTrainingSafetyProfile::Performance,
                24
            ),
            16
        );
        assert_eq!(
            discovery_training_worker_threads_for_available(
                DiscoveryTrainingSafetyProfile::Balanced,
                2
            ),
            1
        );
    }

    #[test]
    fn training_safety_profile_defaults_to_balanced_and_round_trips() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.run_migrations().expect("migrations");

        assert_eq!(
            load_discovery_training_safety_profile(&db),
            DiscoveryTrainingSafetyProfile::Balanced
        );

        set_discovery_training_safety_profile(&db, DiscoveryTrainingSafetyProfile::Performance)
            .expect("set profile");

        assert_eq!(
            load_discovery_training_safety_profile(&db),
            DiscoveryTrainingSafetyProfile::Performance
        );
    }

    #[tokio::test]
    async fn start_training_refuses_legacy_engine_without_starting_v2() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.run_migrations().expect("migrations");
        set_discovery_engine(&db, DiscoveryEngine::V1).expect("select legacy engine");
        let (event_tx, _) = tokio::sync::broadcast::channel::<AppEvent>(1);

        let err = start_training(
            db.clone(),
            event_tx,
            false,
            false,
            Arc::new(AtomicBool::new(false)),
            ExternalProviderRefreshClients::default(),
        )
        .await
        .expect_err("legacy engine must not train through v2 path");

        assert!(
            err.to_string().contains("legacy discovery engine"),
            "unexpected error: {err}"
        );
        let model_count = db
            .with_conn(|conn| {
                let count: i64 =
                    conn.query_row("SELECT COUNT(*) FROM embedding_models", [], |row| {
                        row.get(0)
                    })?;
                Ok(count)
            })
            .expect("count models");
        assert_eq!(model_count, 0);
    }

    #[tokio::test]
    async fn start_training_marks_run_and_model_failed_when_setup_errors_after_creation() {
        let db = Database::open_in_memory().expect("in-memory db");
        db.run_migrations().expect("migrations");
        db.with_conn(|conn| {
            conn.execute("DROP TABLE tracks", [])?;
            Ok(())
        })
        .expect("break trainer input setup");
        let (event_tx, _) = tokio::sync::broadcast::channel::<AppEvent>(1);

        let err = start_training(
            db.clone(),
            event_tx,
            false,
            false,
            Arc::new(AtomicBool::new(false)),
            ExternalProviderRefreshClients::default(),
        )
        .await
        .expect_err("broken setup should fail training");

        assert!(
            err.to_string().contains("no such table: tracks"),
            "unexpected error: {err}"
        );
        let (run_status, run_progress, run_error, model_status) = db
            .with_conn(|conn| {
                let run =
                    queries::get_latest_training_run(conn)?.expect("training run should exist");
                let model_id = run.model_id.expect("training run should have model");
                let model_status: String = conn.query_row(
                    "SELECT status FROM embedding_models WHERE id = ?1",
                    [model_id],
                    |row| row.get(0),
                )?;
                Ok((run.status, run.progress, run.error_text, model_status))
            })
            .expect("read failed run");

        assert_eq!(run_status, "failed");
        assert_eq!(run_progress, 0.05);
        assert!(
            run_error
                .as_deref()
                .is_some_and(|text| text.contains("no such table: tracks")),
            "run should store failure text, got {run_error:?}"
        );
        assert_eq!(model_status, "failed");
    }

    #[test]
    fn external_provider_refresh_budget_skips_fresh_incremental_runs() {
        let now = chrono::NaiveDateTime::parse_from_str("2026-02-02 12:00:00", "%Y-%m-%d %H:%M:%S")
            .unwrap();
        let last_refresh =
            chrono::NaiveDateTime::parse_from_str("2026-02-02 01:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap();

        let budget = plan_external_provider_refresh_at(false, Some(last_refresh), 400, now);

        assert!(!budget.should_refresh);
        assert_eq!(budget.seed_tracks, 0);
        assert_eq!(budget.lastfm_rows_per_seed, 0);
        assert_eq!(budget.tidal_new_release_rows, 0);
    }

    #[test]
    fn external_provider_refresh_persists_lastfm_and_tidal_candidates() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Seed Artist')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, duration_ms, tidal_id)
             VALUES (1, 'Seed Track', 1, 200000, 101)",
            [],
        )
        .unwrap();
        let seeds = vec![EmbeddingTrackRow {
            track_id: 1,
            title: "Seed Track".to_string(),
            artist_name: Some("Seed Artist".to_string()),
            album_title: None,
            duration_ms: Some(200_000),
            best_quality: Some("LOSSLESS".to_string()),
            source: "tidal".to_string(),
            play_count: 0,
            is_favorite: false,
            playlist_memberships: 0,
            genre_paths: vec![],
            bpm: None,
            energy: None,
            camelot_key: None,
            danceability: None,
            beat_strength: None,
            loudness_lufs: None,
        }];
        let mut lastfm = HashMap::new();
        lastfm.insert(
            1,
            vec![ExternalLastfmCandidate {
                artist: "Similar Artist".to_string(),
                title: "Similar Track".to_string(),
                mbid: Some("mbid-1".to_string()),
                match_score: 0.91,
                branch_from: Some("Branch Artist - Branch Track".to_string()),
            }],
        );
        let tidal = vec![ExternalTidalCandidate {
            tidal_id: 9001,
            artist_name: "New Artist".to_string(),
            title: "New Track".to_string(),
            genre_tags: vec!["new-release".to_string()],
            duration_ms: Some(180_000),
        }];
        let mut tidal_similar = HashMap::new();
        tidal_similar.insert(
            1,
            vec![ExternalTidalCandidate {
                tidal_id: 9002,
                artist_name: "Similar Tidal Artist".to_string(),
                title: "Similar Tidal Track".to_string(),
                genre_tags: vec!["tidal-similar".to_string()],
                duration_ms: Some(181_000),
            }],
        );

        let report = persist_external_provider_refresh(
            &conn,
            &seeds,
            &lastfm,
            &tidal,
            &tidal_similar,
            chrono::NaiveDateTime::parse_from_str("2026-02-02 12:00:00", "%Y-%m-%d %H:%M:%S")
                .unwrap(),
        )
        .unwrap();

        assert_eq!(report.candidates_upserted, 3);
        assert_eq!(report.sightings_upserted, 3);
        assert_eq!(report.lastfm_candidates_upserted, 1);
        assert_eq!(report.lastfm_sightings_upserted, 1);
        assert_eq!(report.tidal_new_release_candidates_upserted, 1);
        assert_eq!(report.tidal_new_release_sightings_upserted, 1);
        assert_eq!(report.tidal_similar_rows_seen, 1);
        assert_eq!(report.tidal_similar_candidates_upserted, 1);
        assert_eq!(report.tidal_similar_sightings_upserted, 1);
        let sources = conn
            .prepare("SELECT source FROM external_track_candidate_sightings ORDER BY source")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            sources,
            vec!["lastfm_similar", "tidal_new_release", "tidal_similar"]
        );
        let refresh_at: String = conn
            .query_row(
                "SELECT value FROM server_config WHERE key = 'discovery_external_refresh_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(refresh_at, "2026-02-02 12:00:00");
    }

    #[test]
    fn lastfm_branch_candidate_attenuates_parent_and_child_match() {
        let parent = ExternalLastfmCandidate {
            artist: "Parent Artist".to_string(),
            title: "Parent Track".to_string(),
            mbid: None,
            match_score: 0.8,
            branch_from: None,
        };
        let child = LastFmSimilarTrack {
            artist: "Child Artist".to_string(),
            title: "Child Track".to_string(),
            mbid: Some("child-mbid".to_string()),
            match_score: 0.5,
        };

        let branched = ExternalLastfmCandidate::branch_from(child, &parent);

        assert_eq!(branched.artist, "Child Artist");
        assert_eq!(branched.title, "Child Track");
        assert_eq!(branched.mbid.as_deref(), Some("child-mbid"));
        assert_eq!(
            branched.branch_from.as_deref(),
            Some("Parent Artist - Parent Track")
        );
        assert!((branched.match_score - 0.26).abs() < f64::EPSILON);
    }

    #[test]
    fn lastfm_resolved_edges_weight_direct_above_branch() {
        let edges = lastfm_resolved_edges_from_rows(vec![
            queries::ResolvedLastfmExternalSightingRow {
                seed_track_id: 1,
                resolved_track_id: 2,
                similarity: 0.8,
                source_payload_json: Some(r#"{"match":0.8}"#.to_string()),
            },
            queries::ResolvedLastfmExternalSightingRow {
                seed_track_id: 1,
                resolved_track_id: 3,
                similarity: 0.8,
                source_payload_json: Some(
                    r#"{"match":0.8,"branch_from":"Parent - Track"}"#.to_string(),
                ),
            },
        ]);

        let direct = edges
            .iter()
            .find(|edge| edge.evidence_kind == EvidenceKind::LastfmDirectSimilarity)
            .expect("direct edge");
        let branch = edges
            .iter()
            .find(|edge| edge.evidence_kind == EvidenceKind::LastfmBranchSimilarity)
            .expect("branch edge");
        assert_eq!(edges.len(), 2);
        assert!(direct.weight > branch.weight);
        assert_eq!(direct.from_track_id, 1);
        assert_eq!(direct.to_track_id, 2);
    }

    #[test]
    fn lastfm_resolved_edges_dedupe_same_seed_and_track_by_kind() {
        let edges = lastfm_resolved_edges_from_rows(vec![
            queries::ResolvedLastfmExternalSightingRow {
                seed_track_id: 1,
                resolved_track_id: 2,
                similarity: 0.4,
                source_payload_json: Some(r#"{"match":0.4}"#.to_string()),
            },
            queries::ResolvedLastfmExternalSightingRow {
                seed_track_id: 1,
                resolved_track_id: 2,
                similarity: 0.9,
                source_payload_json: Some(r#"{"match":0.9}"#.to_string()),
            },
        ]);

        assert_eq!(edges.len(), 1);
        assert!((edges[0].weight - (0.9 * LASTFM_DIRECT_EDGE_WEIGHT)).abs() < f64::EPSILON);
    }

    fn tidal_search_track(
        id: i64,
        title: &str,
        artist_name: &str,
        duration_seconds: i64,
    ) -> TidalSearchTrack {
        TidalSearchTrack {
            id,
            title: title.to_string(),
            duration: duration_seconds,
            artist_name: Some(artist_name.to_string()),
            ..TidalSearchTrack::default()
        }
    }

    #[test]
    fn tidal_resolution_exact_artist_title_match_resolves() {
        let decision = classify_tidal_search_resolution(
            "Heroes",
            "David Bowie",
            Some(183_000),
            &[tidal_search_track(42, "Heroes", "David Bowie", 183)],
        );

        assert!(matches!(
            decision,
            TidalSearchResolutionDecision::Resolved(candidate) if candidate.tidal_id == 42
        ));
    }

    #[test]
    fn tidal_resolution_punctuation_and_case_differences_resolve() {
        let decision = classify_tidal_search_resolution(
            "B.O.B.",
            "OutKast",
            Some(304_000),
            &[tidal_search_track(43, "B O B", "OUTKAST", 305)],
        );

        assert!(matches!(
            decision,
            TidalSearchResolutionDecision::Resolved(candidate) if candidate.tidal_id == 43
        ));
    }

    #[test]
    fn tidal_resolution_wrong_artist_or_title_rejects() {
        let wrong_artist = classify_tidal_search_resolution(
            "Teardrop",
            "Massive Attack",
            Some(330_000),
            &[tidal_search_track(44, "Teardrop", "Newton Faulkner", 330)],
        );
        let wrong_title = classify_tidal_search_resolution(
            "Teardrop",
            "Massive Attack",
            Some(330_000),
            &[tidal_search_track(45, "Angel", "Massive Attack", 330)],
        );

        assert_eq!(wrong_artist, TidalSearchResolutionDecision::Rejected);
        assert_eq!(wrong_title, TidalSearchResolutionDecision::Rejected);
    }

    #[test]
    fn tidal_resolution_duration_mismatch_rejects() {
        let decision = classify_tidal_search_resolution(
            "Windowlicker",
            "Aphex Twin",
            Some(367_000),
            &[tidal_search_track(46, "Windowlicker", "Aphex Twin", 120)],
        );

        assert_eq!(decision, TidalSearchResolutionDecision::Rejected);
    }

    #[test]
    fn tidal_resolution_multiple_strong_candidates_are_ambiguous() {
        let decision = classify_tidal_search_resolution(
            "Midnight City",
            "M83",
            Some(244_000),
            &[
                tidal_search_track(47, "Midnight City", "M83", 244),
                tidal_search_track(48, "Midnight City", "M83", 245),
            ],
        );

        assert_eq!(decision, TidalSearchResolutionDecision::Ambiguous);
    }

    #[test]
    fn external_tidal_resolution_updates_sidecar_without_importing_track() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let candidate = queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: None,
                mbid: None,
                dedupe_key: "lastfm:burial:archangel".to_string(),
                title: "Archangel".to_string(),
                artist_name: "Burial".to_string(),
                genre_tags_json: None,
                duration_ms: None,
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .unwrap();

        let updated = queries::resolve_external_candidate_tidal_metadata(
            &conn,
            candidate.id,
            &queries::ExternalCandidateTidalResolution {
                tidal_id: 9001,
                genre_tags_json: Some(r#"["dubstep"]"#.to_string()),
                duration_ms: Some(244_000),
            },
        )
        .unwrap();

        assert_eq!(updated.tidal_id, Some(9001));
        assert_eq!(updated.resolved_track_id, None);
        assert_eq!(updated.title, "Archangel");
        assert_eq!(updated.artist_name, "Burial");
        let track_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))
            .unwrap();
        assert_eq!(track_count, 0);
    }

    #[test]
    fn provider_rate_limit_errors_are_detected_for_cooldown() {
        let error = anyhow::anyhow!("Last.fm HTTP 429: too many requests");

        assert!(is_provider_rate_limit_error(&error));
    }

    #[test]
    fn transition_source_weighting_prefers_manual_completed_edges() {
        let manual = transition_evidence_weight(Some("queue"), true, 1.0);
        let passive = transition_evidence_weight(Some("automix"), false, 1.0);

        assert!(manual > passive);
        assert!(passive < 1.0);
    }
}

fn parse_reason_tags(reason_json: Option<&str>) -> Vec<String> {
    serde_json::from_str::<Vec<DiscoveryNeighborReason>>(reason_json.unwrap_or("[]"))
        .unwrap_or_default()
        .into_iter()
        .map(|reason| reason.label)
        .collect()
}

fn reason_label(key: &str) -> &'static str {
    match key {
        "behavioral" => "same pocket",
        "audio_texture" => "audio texture",
        "album_context" => "album-adjacent",
        "artist_affinity" => "session neighbor",
        "genre_branch" => "genre branch",
        "lastfm_direct" => "Last.fm direct",
        "lastfm_branch" => "Last.fm branch",
        _ => "learned signal",
    }
}
