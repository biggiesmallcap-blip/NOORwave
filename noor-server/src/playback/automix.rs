//! DJ-style automix engine.
//!
//! Owns the four `automix_*` flags on `playback_state`, the queue-depth
//! orchestrator that keeps the upcoming list refilled, and the scoring +
//! shuffle logic that decides what to append. The user-facing "Why" string for
//! each picked track is emitted by the same scoring pass that ranked it, so
//! the explanation can never drift from the score it justifies.
//!
//! Tests for this module live in `playback::player::tests` because they share
//! the in-memory database fixture; the automix items those tests reach into
//! are `pub(crate)`. The reverse reach (this module into `player::*`) is via
//! `pub(super)` for sibling-only helpers (`playback_anchor_index`,
//! `normalize_genre_key`) and `pub` / `pub(crate)` for items player.rs already
//! exposed (`load_state`, `load_snapshot`, `build_session_taste_profile`).

use crate::db::{
    models::{AudioDspFeatures, QueueItem, Track},
    queries,
};
use crate::playback::dj_queue_ranker::{
    GeneratedCandidate, append_dj_reason, rank_generated_candidates,
};
use crate::playback::player::{
    PlaybackSnapshot, build_session_taste_profile, load_snapshot, load_state, normalize_genre_key,
    playback_anchor_index,
};
use crate::playback::queue::{self, ShuffleMode};
use crate::playback::shuffle::{
    WeightedShuffleProfile, genre_shuffle, genre_shuffle_with_rng, seeded_rng, true_shuffle,
    true_shuffle_with_rng,
};
use crate::services::audio_analysis::{
    CamelotRelation, camelot_relation, compute_harmonic_multiplier,
};
use crate::smart::taste_vector::adapters::from_session_profile;
use crate::smart::taste_vector::{SeedContext, TasteVector};
use anyhow::Result;
use rusqlite::{Connection, params};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

pub const AUTOMIX_MIN_UPCOMING: usize = 8;
const AUTOMIX_BATCH_SIZE: usize = 12;
const TRUE_SHUFFLE_POOL_MULTIPLIER: usize = 12;

#[derive(Debug, Clone)]
struct ScoredTrack {
    track: Track,
    score: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct AutomixSelection {
    pub(crate) track: Track,
    reason: Option<String>,
}

impl AutomixSelection {
    fn new(track: Track, reason: impl Into<String>) -> Self {
        Self {
            track,
            reason: Some(reason.into()),
        }
    }

    fn into_queue_pair(self) -> (Track, Option<String>) {
        (self.track, self.reason)
    }
}

/// Whether a scoring factor helped a candidate get picked or worked against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomixSignalKind {
    Boost,
    Penalty,
}

/// One human-readable factor that moved a candidate's automix score, tagged
/// with its direction. Emitted by `automix_score` itself so the user-facing
/// "Why" is derived from the same pass that ranked the track and can never
/// contradict it.
#[derive(Debug, Clone)]
pub(crate) struct AutomixSignal {
    label: &'static str,
    kind: AutomixSignalKind,
}

impl AutomixSignal {
    fn boost(label: &'static str) -> Self {
        Self {
            label,
            kind: AutomixSignalKind::Boost,
        }
    }

    fn penalty(label: &'static str) -> Self {
        Self {
            label,
            kind: AutomixSignalKind::Penalty,
        }
    }
}

/// A candidate's automix score plus the signals that produced it.
#[derive(Debug, Clone)]
pub(crate) struct AutomixScore {
    pub(crate) value: f64,
    signals: Vec<AutomixSignal>,
}

pub fn set_automix_enabled(conn: &Connection, enabled: bool) -> Result<PlaybackSnapshot> {
    conn.execute(
        "UPDATE playback_state SET automix_enabled = ?1 WHERE id = 1",
        params![enabled],
    )?;
    load_snapshot(conn)
}

pub fn set_automix_discover_new(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_discover_new = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_automix_use_learning(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_use_learning = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn set_automix_allow_external(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET automix_allow_external = ?1 WHERE id = 1",
        params![enabled],
    )?;
    Ok(())
}

pub fn ensure_automix_queue_depth(
    conn: &Connection,
    target_upcoming: usize,
    recently_cleared: bool,
) -> Result<Vec<QueueItem>> {
    let state = load_state(conn)?;
    let queue_items = queue::load_queue(conn)?;

    if !state.automix_enabled || state.repeat_mode == "one" {
        return Ok(queue_items);
    }

    // User just manually cleared the queue (within the suppression window);
    // refilling now would instantly negate that action. Caller resets the
    // window on any new user-driven play, so this only suppresses while the
    // user is actively in the "I cleared, I'm done" state.
    if recently_cleared {
        return Ok(queue_items);
    }

    let Some(current_track) = state.current_track.as_ref() else {
        return Ok(queue_items);
    };

    let current_index = playback_anchor_index(
        &queue_items,
        Some(current_track.id),
        state.current_queue_item_id,
    );

    // If the current track isn't found in the queue (e.g. queue was replaced or cleared),
    // treat upcoming count as 0 so automix still extends rather than bailing.
    let upcoming_count = current_index
        .map(|idx| queue_items.len().saturating_sub(idx + 1))
        .unwrap_or(0);

    if upcoming_count >= target_upcoming {
        return Ok(queue_items);
    }

    let needed = (target_upcoming - upcoming_count).max(AUTOMIX_BATCH_SIZE);
    let shuffle_mode = ShuffleMode::parse(&state.shuffle_mode);
    let shuffle_seed = if shuffle_mode != ShuffleMode::Off {
        conn.query_row(
            "SELECT shuffle_seed FROM playback_state WHERE id = 1",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?
    } else {
        None
    };

    let extension = build_automix_extension_with_reasons(
        conn,
        current_track,
        &queue_items,
        shuffle_mode,
        shuffle_seed,
        needed,
        state.automix_use_learning,
    )?;

    let mut appended = false;
    let generated_count = extension.len();
    if !extension.is_empty() {
        let extension = extension
            .into_iter()
            .map(AutomixSelection::into_queue_pair)
            .collect::<Vec<_>>();
        queue::append_tracks_with_reasons(conn, &extension, "automix")?;
        appended = true;
    }

    if state.automix_allow_external
        && let Some(model) = queries::get_selected_discovery_embedding_model(conn)
            .ok()
            .flatten()
    {
        let external_needed = needed.saturating_sub(generated_count).max(1);
        let appended_external =
            append_automix_external_candidates(conn, model.id, current_track.id, external_needed)?;
        appended |= appended_external > 0;
    }

    if !appended {
        return Ok(queue_items);
    }

    queue::load_queue(conn)
}

fn append_automix_external_candidates(
    conn: &Connection,
    model_id: i64,
    seed_track_id: i64,
    limit: usize,
) -> Result<usize> {
    let (queued_tidal_ids, queued_pairs) = load_queued_external_identities(conn)?;
    let rows = queries::get_external_candidate_neighbors(
        conn,
        model_id,
        seed_track_id,
        (limit.max(1) * 4).max(12) as i64,
        true,
    )?;
    let mut candidates = Vec::new();
    for row in rows {
        if let Some(tidal_id) = row.tidal_id
            && queued_tidal_ids.contains(&tidal_id)
        {
            continue;
        }
        let pair = normalize_external_pair(&row.artist_name, &row.title);
        if queued_pairs.contains(&pair) {
            continue;
        }
        candidates.push(row);
    }

    let generated = candidates
        .into_iter()
        .map(|row| GeneratedCandidate {
            track_id: None,
            tidal_id: row.tidal_id,
            item: row,
        })
        .collect::<Vec<_>>();
    let fallback = generated
        .iter()
        .map(|candidate| RankedExternalCandidate {
            row: candidate.item.clone(),
            score: 1.0,
            reasons: Vec::new(),
        })
        .collect::<Vec<_>>();
    let ranked = rank_generated_candidates(conn, seed_track_id, generated)
        .map(|ranked| {
            ranked
                .into_iter()
                .map(|ranked| RankedExternalCandidate {
                    row: ranked.item,
                    score: ranked.score,
                    reasons: ranked.reasons,
                })
                .collect()
        })
        .unwrap_or(fallback);
    let mut appended = 0usize;
    for ranked in ranked {
        let row = ranked.row;
        let reason = append_dj_reason("external similarity", ranked.score, &ranked.reasons);
        queue::append_external_track(
            conn,
            &queue::ExternalTrackInsert {
                artist: &row.artist_name,
                title: &row.title,
                source: "automix-new",
                reason: Some(&reason),
                tidal_id_hint: row.tidal_id,
                local_track_id: None,
            },
        )?;
        appended += 1;
        if appended >= limit {
            break;
        }
    }
    Ok(appended)
}

struct RankedExternalCandidate {
    row: queries::ExternalCandidateNeighborRow,
    score: f64,
    reasons: Vec<&'static str>,
}

fn rank_automix_selections(
    conn: &Connection,
    seed_track_id: i64,
    selections: Vec<AutomixSelection>,
) -> Vec<AutomixSelection> {
    let generated = selections
        .into_iter()
        .map(|selection| GeneratedCandidate {
            track_id: Some(selection.track.id),
            tidal_id: selection.track.tidal_id,
            item: selection,
        })
        .collect::<Vec<_>>();
    let fallback = generated
        .iter()
        .map(|candidate| candidate.item.clone())
        .collect::<Vec<_>>();
    rank_generated_candidates(conn, seed_track_id, generated)
        .map(|ranked| {
            ranked
                .into_iter()
                .map(|ranked| {
                    let mut selection = ranked.item;
                    if let Some(reason) = selection.reason.as_deref() {
                        selection.reason =
                            Some(append_dj_reason(reason, ranked.score, &ranked.reasons));
                    }
                    selection
                })
                .collect()
        })
        .unwrap_or(fallback)
}

fn load_queued_external_identities(
    conn: &Connection,
) -> Result<(HashSet<i64>, HashSet<(String, String)>)> {
    let mut stmt = conn.prepare(
        "SELECT pending_artist, pending_title, tidal_id_hint
         FROM queue
         WHERE source = 'automix-new'
           AND track_id IS NULL",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut tidal_ids = HashSet::new();
    let mut pairs = HashSet::new();
    for (artist, title, tidal_id) in rows {
        if let Some(tidal_id) = tidal_id.filter(|id| *id > 0) {
            tidal_ids.insert(tidal_id);
        }
        if let (Some(artist), Some(title)) = (artist, title) {
            pairs.insert(normalize_external_pair(&artist, &title));
        }
    }
    Ok((tidal_ids, pairs))
}

fn normalize_external_pair(artist: &str, title: &str) -> (String, String) {
    (
        artist.trim().to_ascii_lowercase(),
        title.trim().to_ascii_lowercase(),
    )
}

pub(crate) fn build_automix_extension_with_reasons(
    conn: &Connection,
    current_track: &Track,
    queue_items: &[QueueItem],
    mode: ShuffleMode,
    shuffle_seed: Option<i64>,
    needed: usize,
    use_learning: bool,
) -> Result<Vec<AutomixSelection>> {
    if use_learning
        && let Some(model) = queries::get_selected_discovery_embedding_model(conn)
            .ok()
            .flatten()
    {
        let excluded = queue_items
            .iter()
            .map(|item| item.track.id)
            .collect::<Vec<_>>();
        let neighbors = queries::get_track_neighbors(
            conn,
            model.id,
            current_track.id,
            (needed * 4).max(24) as i64,
            &excluded,
        )?;
        if !neighbors.is_empty() {
            let neighbor_reasons = neighbors
                .iter()
                .map(|row| (row.track_id, automix_neighbor_reason(row)))
                .collect::<HashMap<_, _>>();
            let neighbor_ids = neighbors.iter().map(|row| row.track_id).collect::<Vec<_>>();
            let tracks = queue::get_tracks_by_ids(conn, &neighbor_ids)?;
            let track_map = tracks
                .into_iter()
                .map(|track| (track.id, track))
                .collect::<HashMap<_, _>>();
            let mut ordered = neighbor_ids
                .into_iter()
                .filter_map(|track_id| {
                    track_map.get(&track_id).cloned().map(|track| {
                        AutomixSelection::new(
                            track,
                            neighbor_reasons
                                .get(&track_id)
                                .cloned()
                                .unwrap_or_else(|| "automix: learned similarity".to_string()),
                        )
                    })
                })
                .collect::<Vec<_>>();
            if let Some(seed) = shuffle_seed
                && mode != ShuffleMode::Off
            {
                let tracks = ordered
                    .iter()
                    .map(|selection| selection.track.clone())
                    .collect::<Vec<_>>();
                let shuffled =
                    queue::reorder_tracks_with_seed(conn, &tracks, mode, seed, "automix_learned")?;
                let mut by_track = ordered
                    .into_iter()
                    .map(|selection| (selection.track.id, selection))
                    .collect::<HashMap<_, _>>();
                ordered = shuffled
                    .into_iter()
                    .filter_map(|track| by_track.remove(&track.id))
                    .collect();
            }
            ordered = rank_automix_selections(conn, current_track.id, ordered);
            ordered.truncate(needed);
            if !ordered.is_empty() {
                return Ok(ordered);
            }
        }
    }

    let session_profile = build_session_taste_profile(conn, current_track)?;
    let mut excluded_track_ids = queue_items
        .iter()
        .map(|item| item.track.id)
        .collect::<Vec<_>>();
    excluded_track_ids.extend(session_profile.recent_track_ids.iter().copied());
    // Convert once, after recent_track_ids has been read for exclusions, so
    // the move into TasteVector below doesn't force an extra clone.
    let (taste, seed) = from_session_profile(&session_profile);
    excluded_track_ids.sort_unstable();
    excluded_track_ids.dedup();

    // Load at most 500 candidates to keep memory bounded while still
    // providing enough diversity for scoring and genre shuffling.
    const MAX_CANDIDATES: usize = 500;

    // Preferred recall: precomputed track_similarity (co-album/artist/genre/duration).
    // Floor at `needed` so we always have at least the batch size to score and decluster;
    // below that, widen to the random pool. Library coverage is ~78% of seeds at this floor.
    let similar = queries::get_similar_tracks(
        conn,
        current_track.id,
        MAX_CANDIDATES as i64,
        &excluded_track_ids,
    )
    .unwrap_or_default();

    // Phase 2c hotfix: if we got here, the embedding fast-path produced
    // nothing usable (model missing, no neighbours, or filtered to
    // empty). If the precomputed similarity table is also empty for
    // this seed, the track has no learned recommendation signal. Rather
    // than filling with a 500-track random library pool (which reads as
    // "the system is broken"), cascade through metadata: same-artist,
    // then same-album, then shared genre. This keeps the queue alive
    // for seeds that haven't been embedded yet (e.g. tracks without a
    // service ID, or library additions since the last training run).
    if similar.is_empty() {
        let fallback = build_metadata_fallback(conn, current_track, &excluded_track_ids, needed)?;
        if fallback.is_empty() {
            tracing::debug!(
                seed_track_id = current_track.id,
                "automix: skipping extension - seed has no recommendation signal and no artist/album/genre matches"
            );
        }
        return Ok(fallback
            .into_iter()
            .map(|track| {
                let reason = automix_metadata_reason(current_track, &track);
                AutomixSelection::new(track, reason)
            })
            .collect());
    }

    let mut candidates: Vec<Track> = if similar.len() >= needed {
        let similar_ids = similar.iter().map(|r| r.track_id).collect::<Vec<_>>();
        queue::get_tracks_by_ids(conn, &similar_ids)?
    } else {
        queries::get_tracks_excluding_with_limit(conn, &excluded_track_ids, MAX_CANDIDATES)?
    };
    if candidates.is_empty() {
        let queue_track_ids = queue_items
            .iter()
            .map(|item| item.track.id)
            .collect::<Vec<_>>();
        candidates =
            queries::get_tracks_excluding_with_limit(conn, &queue_track_ids, MAX_CANDIDATES)?;
    }

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_genres = queue::get_track_genres(conn, &candidates)?;

    // Load DSP features for seed + all candidates (ignore errors - fall back to behavioural score).
    let seed_features = queries::get_audio_dsp_features(conn, current_track.id)
        .ok()
        .flatten();
    let mut candidate_features: HashMap<i64, AudioDspFeatures> = HashMap::new();
    for track in &candidates {
        if let Ok(Some(features)) = queries::get_audio_dsp_features(conn, track.id) {
            candidate_features.insert(track.id, features);
        }
    }

    let ordered = order_automix_candidates(
        mode,
        candidates,
        &candidate_genres,
        &taste,
        &seed,
        needed,
        shuffle_seed,
        seed_features.as_ref(),
        &candidate_features,
    );
    let ordered = decluster_by_album(ordered);
    Ok(ordered
        .into_iter()
        .take(needed)
        .map(|track| {
            let genres = candidate_genres
                .get(&track.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let score = automix_score(
                &track,
                genres,
                &taste,
                &seed,
                seed_features.as_ref(),
                candidate_features.get(&track.id),
            );
            let reason = automix_scored_reason(&score);
            AutomixSelection::new(track, reason)
        })
        .collect())
}

fn automix_neighbor_reason(row: &queries::EmbeddingNeighborRow) -> String {
    let reason = row
        .primary_reason
        .as_deref()
        .map(format_reason_key)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "learned similarity".to_string());
    let prefix = format!("automix: {reason}");
    format!(
        "{prefix} | {{\"score\":{:.4},\"behavioral_score\":{:.4},\"audio_score\":{:.4},\"metadata_score\":{:.4},\"confidence\":{:.4}}}",
        row.score, row.behavioral_score, row.audio_score, row.metadata_score, row.confidence
    )
}

fn automix_metadata_reason(seed: &Track, track: &Track) -> String {
    if seed.artist_id != 0 && seed.artist_id == track.artist_id {
        return "automix: same artist fallback".to_string();
    }
    if seed.album_id.is_some() && seed.album_id == track.album_id {
        return "automix: same album fallback".to_string();
    }
    "automix: shared genre fallback".to_string()
}

/// Build the persisted "Why" string for a scored automix pick from the score's
/// own signal breakdown. Boost signals lead the reason; penalties are rendered
/// explicitly as "despite ..." so the explanation can never present a factor that
/// actually counted *against* the track as the reason it was picked.
pub(crate) fn automix_scored_reason(score: &AutomixScore) -> String {
    let boosts: Vec<&str> = score
        .signals
        .iter()
        .filter(|signal| signal.kind == AutomixSignalKind::Boost)
        .map(|signal| signal.label)
        .collect();
    let penalties: Vec<&str> = score
        .signals
        .iter()
        .filter(|signal| signal.kind == AutomixSignalKind::Penalty)
        .map(|signal| signal.label)
        .collect();

    let lead = if boosts.is_empty() {
        "library score".to_string()
    } else {
        boosts.into_iter().take(4).collect::<Vec<_>>().join(", ")
    };
    let prefix = if penalties.is_empty() {
        format!("automix: {lead}")
    } else {
        format!(
            "automix: {lead} despite {}",
            penalties.into_iter().take(3).collect::<Vec<_>>().join(", ")
        )
    };
    format!("{prefix} | {{\"score\":{:.4}}}", score.value)
}

fn format_reason_key(value: &str) -> String {
    value.trim().replace('_', " ")
}

// Metadata-only fallback for seeds with no embedding neighbours and no
// precomputed similarity rows. Cascades: same-artist -> same-album ->
// shared-genre, stopping once `needed` tracks are collected. Excludes the
// seed itself plus anything already in `excluded`. Returns up to `needed`
// tracks; may return fewer or empty if the library has nothing to offer.
fn build_metadata_fallback(
    conn: &Connection,
    seed: &Track,
    excluded: &[i64],
    needed: usize,
) -> Result<Vec<Track>> {
    let mut seen: HashSet<i64> = excluded.iter().copied().collect();
    seen.insert(seed.id);
    let mut result: Vec<Track> = Vec::new();
    let mut stage_hit: Option<&str> = None;

    // Stage 1: same artist.
    if seed.artist_id != 0 {
        let artist_tracks = queries::get_artist_tracks(conn, seed.artist_id)?;
        for t in artist_tracks {
            if seen.insert(t.id) {
                if stage_hit.is_none() {
                    stage_hit = Some("artist");
                }
                result.push(t);
                if result.len() >= needed {
                    break;
                }
            }
        }
    }

    // Stage 2: same album - appends to whatever stage 1 produced.
    if result.len() < needed {
        if let Some(album_id) = seed.album_id {
            let album_tracks = queries::get_album_tracks(conn, album_id)?;
            for t in album_tracks {
                if seen.insert(t.id) {
                    if stage_hit.is_none() {
                        stage_hit = Some("album");
                    }
                    result.push(t);
                    if result.len() >= needed {
                        break;
                    }
                }
            }
        }
    }

    // Stage 3: shared genre - queries each genre_id the seed belongs to.
    if result.len() < needed {
        let mut stmt =
            conn.prepare("SELECT DISTINCT genre_id FROM track_genres WHERE track_id = ?1")?;
        let genre_ids: Vec<i64> = stmt
            .query_map(params![seed.id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        'genre: for genre_id in genre_ids {
            let genre_tracks = queries::get_tracks_by_genre_filtered(
                conn,
                genre_id,
                false,
                crate::genre::filter::GalaxyFilterRule::default_rule(),
            )?;
            for t in genre_tracks {
                if seen.insert(t.id) {
                    if stage_hit.is_none() {
                        stage_hit = Some("genre");
                    }
                    result.push(t);
                    if result.len() >= needed {
                        break 'genre;
                    }
                }
            }
        }
    }

    if let Some(stage) = stage_hit {
        tracing::info!(
            seed_track_id = seed.id,
            stage,
            count = result.len(),
            "automix: metadata fallback hit"
        );
    }

    Ok(result)
}

// Eight inputs is one over clippy's default threshold, but `taste` and `seed`
// represent distinct concepts (per-user preference vs per-query seed track)
// and bundling them just to satisfy the lint would obscure that.
#[allow(clippy::too_many_arguments)]
fn order_automix_candidates(
    mode: ShuffleMode,
    candidates: Vec<Track>,
    candidate_genres: &HashMap<i64, Vec<String>>,
    taste: &TasteVector,
    seed: &SeedContext,
    needed: usize,
    shuffle_seed: Option<i64>,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: &HashMap<i64, AudioDspFeatures>,
) -> Vec<Track> {
    let mut scored = candidates
        .into_iter()
        .map(|track| {
            let score = automix_score(
                &track,
                candidate_genres
                    .get(&track.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                taste,
                seed,
                seed_features,
                candidate_features.get(&track.id),
            )
            .value;
            ScoredTrack { track, score }
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.track.title.cmp(&right.track.title))
    });

    match mode {
        ShuffleMode::Off => scored.into_iter().map(|entry| entry.track).collect(),
        ShuffleMode::True => {
            let pool_size = (needed * TRUE_SHUFFLE_POOL_MULTIPLIER).max(48);
            let pool = scored
                .into_iter()
                .take(pool_size)
                .map(|entry| entry.track)
                .collect::<Vec<_>>();
            if let Some(seed) = shuffle_seed {
                let mut rng = seeded_rng(seed, mode.as_str(), "automix");
                true_shuffle_with_rng(&pool, &mut rng)
            } else {
                true_shuffle(&pool)
            }
        }
        ShuffleMode::Weighted => {
            let pool_size = (needed * TRUE_SHUFFLE_POOL_MULTIPLIER).max(48);
            let pool = scored.into_iter().take(pool_size).collect::<Vec<_>>();
            match shuffle_seed {
                Some(seed) => {
                    let mut rng = seeded_rng(seed, mode.as_str(), "automix");
                    weighted_session_shuffle_with_rng(&pool, &mut rng)
                }
                None => weighted_session_shuffle(&pool),
            }
        }
        ShuffleMode::Genre => {
            let mut preferred = Vec::new();
            let mut fallback = Vec::new();

            for entry in scored {
                let genres = candidate_genres
                    .get(&entry.track.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if matches_preferred_genres(genres, taste, seed) {
                    preferred.push(entry.track);
                } else {
                    fallback.push(entry.track);
                }
            }

            // Interleave preferred and fallback at ~3:1 ratio so the queue
            // never becomes a solid wall of one genre type, but still leans
            // toward the current session's taste.
            let (preferred_shuffled, fallback_shuffled) = if let Some(seed) = shuffle_seed {
                let mut preferred_rng = seeded_rng(seed, mode.as_str(), "automix_preferred");
                let mut fallback_rng = seeded_rng(seed, mode.as_str(), "automix_fallback");
                (
                    genre_shuffle_with_rng(&preferred, candidate_genres, &mut preferred_rng),
                    genre_shuffle_with_rng(&fallback, candidate_genres, &mut fallback_rng),
                )
            } else {
                (
                    genre_shuffle(&preferred, candidate_genres),
                    genre_shuffle(&fallback, candidate_genres),
                )
            };
            let total = preferred_shuffled.len() + fallback_shuffled.len();
            let mut ordered = Vec::with_capacity(total);
            let mut pi = 0usize;
            let mut fi = 0usize;
            let mut streak = 0usize;
            while pi < preferred_shuffled.len() || fi < fallback_shuffled.len() {
                let take_pref =
                    pi < preferred_shuffled.len() && (fi >= fallback_shuffled.len() || streak < 3);
                if take_pref {
                    ordered.push(preferred_shuffled[pi].clone());
                    pi += 1;
                    streak += 1;
                } else {
                    ordered.push(fallback_shuffled[fi].clone());
                    fi += 1;
                    streak = 0;
                }
            }
            ordered
        }
    }
}

/// Spread tracks from the same album apart so they don't run consecutively.
/// Preserves the score ordering as much as possible while ensuring no two
/// adjacent tracks share the same album_id.
/// Uses a visited-set pattern instead of Vec::remove to avoid O(n^2).
fn decluster_by_album(tracks: Vec<Track>) -> Vec<Track> {
    if tracks.len() <= 1 {
        return tracks;
    }
    let mut result = Vec::with_capacity(tracks.len());
    let mut visited = vec![false; tracks.len()];
    let mut last_album: Option<i64> = None;

    // Helper: lowest index whose visited bit is unset, or None when every
    // slot has been emitted. Used as the "nothing to avoid" pick and as the
    // fallback when every remaining candidate happens to share last_album.
    let first_unvisited = |visited: &[bool]| -> Option<usize> { visited.iter().position(|v| !*v) };

    for _ in 0..tracks.len() {
        let pos = if let Some(last_id) = last_album {
            tracks
                .iter()
                .enumerate()
                .position(|(i, t)| !visited[i] && (t.album_id != Some(last_id)))
                .or_else(|| first_unvisited(&visited))
        } else {
            // No last album to avoid (first iter, or previous track's
            // album_id was None). Picking unconditionally from index 0
            // re-emits the same track every iter once it has been visited,
            // because last_album stays None when tracks[0].album_id is None.
            // Always walk to the first *unvisited* index instead.
            first_unvisited(&visited)
        };
        let Some(pos) = pos else {
            // Every track has been emitted - we're done.
            break;
        };
        visited[pos] = true;
        last_album = tracks[pos].album_id;
        result.push(tracks[pos].clone());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_with_album(id: i64, album_id: Option<i64>) -> Track {
        Track {
            id,
            title: format!("T{id}"),
            artist_id: 1,
            artist_name: None,
            album_id,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    /// Regression: decluster_by_album used to fall back to index 0 every
    /// iteration when `last_album` was None, producing N copies of tracks[0]
    /// whenever the first track had `album_id = None`.
    #[test]
    fn decluster_by_album_does_not_duplicate_when_first_album_is_none() {
        let input = vec![
            track_with_album(1, None),
            track_with_album(2, Some(10)),
            track_with_album(3, None),
            track_with_album(4, Some(10)),
        ];
        let out = decluster_by_album(input);

        let ids: Vec<i64> = out.iter().map(|t| t.id).collect();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort();
        sorted_ids.dedup();
        assert_eq!(
            sorted_ids,
            vec![1, 2, 3, 4],
            "every input track must appear exactly once, got {ids:?}",
        );
    }

    /// When perfect declustering is possible (every album appears the same
    /// number of times), no two adjacent tracks should share an album.
    #[test]
    fn decluster_by_album_spreads_same_album_tracks_apart_when_possible() {
        let input = vec![
            track_with_album(1, Some(10)),
            track_with_album(2, Some(10)),
            track_with_album(3, Some(20)),
            track_with_album(4, Some(20)),
        ];
        let out = decluster_by_album(input);
        assert_eq!(out.len(), 4);

        for pair in out.windows(2) {
            if let (Some(a), Some(b)) = (pair[0].album_id, pair[1].album_id) {
                assert_ne!(
                    a,
                    b,
                    "adjacent tracks share album_id {a}: {:?}",
                    out.iter().map(|t| (t.id, t.album_id)).collect::<Vec<_>>(),
                );
            }
        }
    }
}

/// Score a candidate for automix selection *and* emit the signals that
/// produced that score. The reason shown to the user is built from these
/// signals (see `automix_scored_reason`), so the explanation is derived from
/// the same pass that ranked the track - it cannot drift from or contradict
/// the score. `value` is byte-identical to the pre-signal scorer.
pub(crate) fn automix_score(
    track: &Track,
    genres: &[String],
    taste: &TasteVector,
    seed: &SeedContext,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: Option<&AudioDspFeatures>,
) -> AutomixScore {
    let mut score = 1.0;
    let mut signals = Vec::new();

    // Hard suppression for recently skipped tracks
    if taste.skipped_track_ids.contains(&track.id) {
        score *= 0.1;
        signals.push(AutomixSignal::penalty("recently skipped"));
    }

    // Same-artist: gentle familiarity boost, not enough to cause artist runs.
    // Artist spread is handled at the queue level by decluster_by_album.
    if Some(track.artist_id) == seed.artist_id && track.artist_id != 0 {
        score *= 1.1;
        signals.push(AutomixSignal::boost("same artist"));
    }

    if seed.source.as_deref() == Some(track.source.as_str()) {
        score *= 1.05;
        signals.push(AutomixSignal::boost("same source"));
    }

    if track.is_favorite {
        score *= 1.2;
        signals.push(AutomixSignal::boost("favorite"));
    }

    // Unplayed tracks get a meaningful boost so they surface before heavily-played ones.
    if track.play_count == 0 {
        score *= 1.35;
        signals.push(AutomixSignal::boost("unplayed"));
    } else if let Some(last_played) = track.last_played_at.as_deref() {
        // Time-decay penalty: full suppression at <1 day, fades to zero by 14 days.
        let days_since = parse_days_since_last_played(last_played);
        if days_since < 14.0 {
            let penalty = 0.5 + 0.5 * (days_since / 14.0);
            score *= penalty;
            signals.push(AutomixSignal::penalty("recently played"));
        }
    }

    if track.artist_id != 0
        && let Some(affinity) = taste.artist_affinity.get(&track.artist_id)
    {
        score += affinity.pos * 0.5;
        score -= affinity.neg * 0.65;
        // Label by the net effect on the score, not the raw counts.
        let net = affinity.pos * 0.5 - affinity.neg * 0.65;
        if net > 0.0 {
            signals.push(AutomixSignal::boost("artist affinity"));
        } else if net < 0.0 {
            signals.push(AutomixSignal::penalty("recent skip penalty"));
        }
    }

    let mut shares_seed_genre = false;
    let mut genre_affinity_net = 0.0;
    let normalized_genres = genres.iter().map(|genre| normalize_genre_key(genre));
    for genre in normalized_genres {
        if seed.genres.contains(&genre) {
            score += 1.8;
            shares_seed_genre = true;
        }
        if let Some(affinity) = taste.genre_affinity.get(&genre) {
            score += affinity.pos * 0.4;
            score -= affinity.neg * 0.5;
            genre_affinity_net += affinity.pos * 0.4 - affinity.neg * 0.5;
        }
    }
    if shares_seed_genre {
        signals.push(AutomixSignal::boost("shared genres"));
    }
    if genre_affinity_net > 0.0 {
        signals.push(AutomixSignal::boost("genre affinity"));
    } else if genre_affinity_net < 0.0 {
        signals.push(AutomixSignal::penalty("genre mismatch"));
    }

    score += (track.fidelity_score.max(0) as f64) * 0.003;

    // DSP harmonic/BPM/energy scoring - only applied when BOTH tracks have features.
    // Unanalyzed tracks are never penalised; they simply skip this pass.
    if let (Some(seed), Some(cand)) = (seed_features, candidate_features) {
        // Camelot + BPM multiplier (shared with radio post-scoring).
        score *= compute_harmonic_multiplier(
            seed.camelot_key.as_deref(),
            cand.camelot_key.as_deref(),
            seed.bpm,
            cand.bpm,
        );

        // The multiplier folds Camelot *and* BPM together, so it can read >1.0
        // even on a key clash that happens to share a tempo. Derive the
        // harmonic signal from the Camelot relationship directly - via the same
        // `camelot_relation` the multiplier uses - so the "Why" never claims a
        // fit the keys don't have, and the two can't drift apart.
        if let (Some(a), Some(b)) = (seed.camelot_key.as_deref(), cand.camelot_key.as_deref()) {
            signals.push(match camelot_relation(a, b) {
                CamelotRelation::Compatible => AutomixSignal::boost("harmonic match"),
                CamelotRelation::Adjacent => AutomixSignal::boost("adjacent key"),
                CamelotRelation::Clash => AutomixSignal::penalty("key clash"),
            });
        }

        // Energy whiplash penalty.
        if let (Some(seed_energy), Some(cand_energy)) = (seed.energy, cand.energy)
            && (seed_energy - cand_energy).abs() > 0.5
        {
            score *= 0.7;
            signals.push(AutomixSignal::penalty("energy whiplash"));
        }
    }

    AutomixScore {
        value: score.max(0.05),
        signals,
    }
}

/// Parse an ISO-8601 timestamp and return days elapsed since then.
/// Returns `f64::MAX` on failure so malformed timestamps get maximum recency penalty.
pub(crate) fn parse_days_since_last_played(timestamp: &str) -> f64 {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return f64::MAX;
    };
    let elapsed = chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc));
    elapsed.num_seconds().max(0) as f64 / 86_400.0
}

fn matches_preferred_genres(genres: &[String], taste: &TasteVector, seed: &SeedContext) -> bool {
    genres
        .iter()
        .map(|genre| normalize_genre_key(genre))
        .any(|genre| {
            seed.genres.contains(&genre)
                || taste
                    .genre_affinity
                    .get(&genre)
                    .is_some_and(|affinity| affinity.pos > 0.0)
        })
}

fn weighted_session_shuffle(entries: &[ScoredTrack]) -> Vec<Track> {
    let mut rng = rand::thread_rng();
    weighted_session_shuffle_with_rng(entries, &mut rng)
}

fn weighted_session_shuffle_with_rng<R: rand::Rng + ?Sized>(
    entries: &[ScoredTrack],
    rng: &mut R,
) -> Vec<Track> {
    let profile = WeightedShuffleProfile::default();
    let mut weighted = entries
        .iter()
        .map(|entry| {
            let weight = profile.weight_for(&entry.track) * entry.score.max(0.05);
            let uniform = rng.gen_range(f64::EPSILON..1.0);
            let key = -uniform.ln() / weight;
            (key, entry.track.clone())
        })
        .collect::<Vec<_>>();

    weighted.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    weighted.into_iter().map(|(_, track)| track).collect()
}
