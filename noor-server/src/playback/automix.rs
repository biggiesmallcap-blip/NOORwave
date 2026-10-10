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

#[cfg(test)]
use crate::db::models::AudioDjProfileKey;
use crate::db::{
    models::{AudioDspFeatures, QueueItem, Track},
    queries,
};
use crate::playback::candidate_gate::CandidateGate;
use crate::playback::dj_queue_ranker::{
    GeneratedCandidate, GeneratedCandidatePolicy, append_dj_reason, dj_fit_multiplier,
    mixing_active, rank_generated_candidates, rank_generated_candidates_chain,
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
#[cfg(test)]
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

pub const AUTOMIX_MIN_UPCOMING: usize = 8;
const AUTOMIX_BATCH_SIZE: usize = 12;
const TRUE_SHUFFLE_POOL_MULTIPLIER: usize = 12;

// Hub-track penalty strength for learned neighbours. The trainer already records
// each candidate's in-degree percentile (how many seeds list it as a neighbour);
// the radio re-ranker discounts high-in-degree "everyone's neighbour" hubs via
// `apply_hub_penalty` (services/radio.rs), but automix historically ignored that
// signal, so a handful of hubs (the XXXTentacion / Blur effect) bled into every
// genre's queue regardless of fit. Mirror radio's `1/(1 + k*pct)` shape here.
// Strength of the discount once a candidate is past the hub threshold. Stronger
// than radio's per-blend values because automix applies it only to the worst
// offenders (see threshold below) rather than across the whole distribution.
const AUTOMIX_HUB_PENALTY: f64 = 1.5;

// Only the top of the in-degree distribution counts as a "hub". The percentile is
// rank-based and library-relative, so half of any library sits above 0.5 —
// penalising everything with some in-degree just compresses scores uniformly and
// changes no ordering. Gating at 0.85 leaves ordinary tracks untouched and lets
// the ramp act only on the small tail of genuine everyone's-neighbour artists,
// whatever those happen to be in a given library.
const AUTOMIX_HUB_THRESHOLD: f64 = 0.85;

// Radio-style `1/(1 + k*x)` hub discount, shared by the learned-neighbour policy
// and the scored-fallback path. `percentile` is a candidate's global in-degree
// percentile in [0, 1]; below the threshold (or with no neighbour rows) it returns
// 1.0, and above it the penalty ramps with how far past the threshold it sits, so a
// 0.99 hub is hit far harder than a 0.86 one.
fn hub_multiplier(percentile: f64) -> f64 {
    if percentile <= AUTOMIX_HUB_THRESHOLD {
        1.0
    } else {
        let excess = (percentile - AUTOMIX_HUB_THRESHOLD) / (1.0 - AUTOMIX_HUB_THRESHOLD);
        1.0 / (1.0 + AUTOMIX_HUB_PENALTY * excess)
    }
}

// Rarity (IDF, normalised to [0, 1]) for each of the seed's genre keys. A genre
// covering most of the library carries little signal; agreement on a niche genre
// is a stronger match. Mirrors the IDF weighting in compute_track_similarity so the
// scorer and the similarity table agree on what a genre match is worth. Returns
// empty on any error, which leaves the scorer on its original flat weighting.
fn seed_genre_rarity(conn: &Connection, seed_genres: &HashSet<String>) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    if seed_genres.is_empty() {
        return out;
    }
    let Ok(total) = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| {
        row.get::<_, i64>(0)
    }) else {
        return out;
    };
    if total <= 0 {
        return out;
    }
    let ln_total = (total as f64).ln().max(1.0);
    let rows = {
        let Ok(mut stmt) = conn.prepare(
            "SELECT g.name, COUNT(DISTINCT tg.track_id)
             FROM track_genres tg JOIN genres g ON g.id = tg.genre_id
             GROUP BY g.id",
        ) else {
            return out;
        };
        let mapped = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        });
        match mapped.and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>()) {
            Ok(rows) => rows,
            Err(_) => return out,
        }
    };
    for (name, members) in rows {
        let key = normalize_genre_key(&name);
        if members > 0 && seed_genres.contains(&key) {
            let rarity = ((total as f64 / members as f64).ln() / ln_total).clamp(0.0, 1.0);
            // A normalised key can come from more than one genre name; keep the rarest.
            out.entry(key)
                .and_modify(|existing: &mut f64| *existing = existing.max(rarity))
                .or_insert(rarity);
        }
    }
    out
}

#[derive(Debug, Clone)]
struct ScoredTrack {
    track: Track,
    score: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct AutomixSelection {
    pub(crate) track: Track,
    reason: Option<String>,
    ranking_policy: GeneratedCandidatePolicy,
}

impl AutomixSelection {
    fn new(track: Track, reason: impl Into<String>) -> Self {
        Self {
            track,
            reason: Some(reason.into()),
            ranking_policy: GeneratedCandidatePolicy::default(),
        }
    }

    fn with_ranking_policy(mut self, ranking_policy: GeneratedCandidatePolicy) -> Self {
        self.ranking_policy = ranking_policy;
        self
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
    if !extension.is_empty() {
        let extension = extension
            .into_iter()
            .map(AutomixSelection::into_queue_pair)
            .collect::<Vec<_>>();
        queue::append_tracks_with_reasons(conn, &extension, "automix")?;
        appended = true;
    }

    // External picks ("Allow external" or "Include New"): tracks Last.fm
    // linked to this seed or its learned neighbors, gated like every other
    // source, filling up to a quarter of the batch when good ones exist.
    if state.automix_allow_external || state.automix_discover_new {
        let model_id = queries::get_selected_discovery_embedding_model(conn)
            .ok()
            .flatten()
            .map(|model| model.id);
        let external_slots = (needed / 4).max(1);
        // External picks are optional; a failure here must never fail the
        // refill that next_track and peek_next_track depend on.
        match append_automix_external_candidates(conn, model_id, current_track.id, external_slots) {
            Ok(appended_external) => appended |= appended_external > 0,
            Err(error) => {
                tracing::warn!(target: "noor.automix", %error, "external automix refill failed")
            }
        }
    }

    if !appended {
        return Ok(queue_items);
    }

    queue::load_queue(conn)
}

/// Where a refill draws from: the session anchor (the station seed or the
/// last track the listener chose, never an automix pick), the playing track
/// and the queue tail, weighted in that order. Anchoring keeps a long session
/// from drifting one noisy hop at a time away from where it started.
fn session_seeds(current_track: &Track, queue_items: &[QueueItem]) -> Vec<(i64, f64)> {
    fn push(seeds: &mut Vec<(i64, f64)>, id: i64, weight: f64) {
        if id > 0 && !seeds.iter().any(|(seed, _)| *seed == id) {
            seeds.push((id, weight));
        }
    }
    let playable = |item: &&QueueItem| !item.is_pending && item.track.id > 0;
    let current_index = queue_items
        .iter()
        .position(|item| item.track.id == current_track.id);
    let anchor = current_index.and_then(|index| {
        queue_items[..=index]
            .iter()
            .rev()
            .filter(playable)
            .find(|item| !item.source.starts_with("automix"))
            .map(|item| item.track.id)
    });
    let tail = queue_items
        .iter()
        .rev()
        .find(playable)
        .map(|item| item.track.id);

    let mut seeds = Vec::new();
    if let Some(anchor) = anchor {
        push(&mut seeds, anchor, SESSION_ANCHOR_WEIGHT);
    }
    push(&mut seeds, current_track.id, SESSION_CURRENT_WEIGHT);
    if let Some(tail) = tail {
        push(&mut seeds, tail, SESSION_TAIL_WEIGHT);
    }
    seeds
}

const SESSION_ANCHOR_WEIGHT: f64 = 1.0;
const SESSION_CURRENT_WEIGHT: f64 = 0.8;
const SESSION_TAIL_WEIGHT: f64 = 0.6;

/// Learned neighbors of several weighted seeds, merged: a candidate scores the
/// weighted sum over the seeds that list it (agreement between anchor and
/// current track counts), and keeps the row of its strongest seed for reasons.
fn anchored_neighbors(
    conn: &Connection,
    model_id: i64,
    seeds: &[(i64, f64)],
    limit: i64,
    excluded: &[i64],
) -> Result<Vec<queries::EmbeddingNeighborRow>> {
    let mut excluded = excluded.to_vec();
    excluded.extend(seeds.iter().map(|(seed, _)| *seed));
    let mut merged: HashMap<i64, (f64, f64, queries::EmbeddingNeighborRow)> = HashMap::new();
    for (seed, weight) in seeds {
        for row in queries::get_track_neighbors(conn, model_id, *seed, limit, &excluded)? {
            let contribution = weight * row.score;
            match merged.get_mut(&row.track_id) {
                Some(entry) => {
                    entry.0 += contribution;
                    if contribution > entry.1 {
                        entry.1 = contribution;
                        entry.2 = row;
                    }
                }
                None => {
                    merged.insert(row.track_id, (contribution, contribution, row));
                }
            }
        }
    }
    let mut rows = merged
        .into_values()
        .map(|(sum, _, mut row)| {
            row.score = sum;
            row
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.track_id.cmp(&right.track_id))
    });
    rows.truncate(limit.max(0) as usize);
    Ok(rows)
}

/// Last.fm match (after neighbor weighting) an external pick needs.
const EXTERNAL_MIN_SCORE: f64 = 0.15;
/// Learned neighbors whose Last.fm links also count, at this weight.
const EXTERNAL_NEIGHBOR_SEEDS: i64 = 5;
const EXTERNAL_NEIGHBOR_WEIGHT: f64 = 0.65;

fn append_automix_external_candidates(
    conn: &Connection,
    model_id: Option<i64>,
    seed_track_id: i64,
    limit: usize,
) -> Result<usize> {
    // Seeds: the playing track, plus its strongest learned neighbors at a
    // discount, so a seed Last.fm never looked up still reaches its scene.
    let mut weighted_seeds = vec![(seed_track_id, 1.0)];
    if let Some(model_id) = model_id {
        // Best effort: without neighbors the seed's own links still count.
        let neighbors = queries::get_track_neighbors(
            conn,
            model_id,
            seed_track_id,
            EXTERNAL_NEIGHBOR_SEEDS,
            &[],
        )
        .unwrap_or_default();
        for row in neighbors {
            weighted_seeds.push((
                row.track_id,
                EXTERNAL_NEIGHBOR_WEIGHT * row.score.clamp(0.0, 1.0),
            ));
        }
    }
    let rows = queries::get_sighted_external_candidates(
        conn,
        &weighted_seeds,
        EXTERNAL_MIN_SCORE,
        (limit.max(1) * 4).max(12) as i64,
    )?;
    // Hidden content, Not for me, recent plays and anything already queued
    // (any version) stay out; hidden rows never reach the insert, where they
    // would bail.
    let queue_items = queue::load_queue(conn)?;
    let mut gate = CandidateGate::load(conn, &queue_items, None);
    let candidates = rows
        .into_iter()
        .filter(|row| gate.admit_candidate(None, row.tidal_id, None, &row.artist_name, &row.title))
        .collect::<Vec<_>>();

    let generated = candidates
        .into_iter()
        .map(|row| GeneratedCandidate {
            track_id: None,
            tidal_id: row.tidal_id,
            policy: Default::default(),
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
    let ranked = rank_generated_candidates(conn, seed_track_id, generated, mixing_active(conn))
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
        let reason = append_dj_reason("Last.fm similar", ranked.score, &ranked.reasons);
        if let Err(error) = queue::append_external_track(
            conn,
            &queue::ExternalTrackInsert {
                artist: &row.artist_name,
                title: &row.title,
                source: "automix-new",
                reason: Some(&reason),
                tidal_id_hint: row.tidal_id,
                ..Default::default()
            },
        ) {
            tracing::warn!(
                target: "noor.automix",
                %error,
                artist = %row.artist_name,
                title = %row.title,
                "skipping external automix pick"
            );
            continue;
        }
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
            policy: selection.ranking_policy.clone(),
            item: selection,
        })
        .collect::<Vec<_>>();
    let fallback = generated
        .iter()
        .map(|candidate| candidate.item.clone())
        .collect::<Vec<_>>();
    rank_generated_candidates_chain(conn, seed_track_id, generated, mixing_active(conn))
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
        let seeds = session_seeds(current_track, queue_items);
        let neighbors = anchored_neighbors(
            conn,
            model.id,
            &seeds,
            (needed * 4).max(24) as i64,
            &excluded,
        )?;
        if !neighbors.is_empty() {
            let neighbor_reasons = neighbors
                .iter()
                .map(|row| (row.track_id, automix_neighbor_reason(row)))
                .collect::<HashMap<_, _>>();
            let neighbor_policies = neighbors
                .iter()
                .map(|row| (row.track_id, automix_neighbor_policy(row)))
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
                        let policy = neighbor_policies
                            .get(&track_id)
                            .cloned()
                            .unwrap_or_default();
                        AutomixSelection::new(
                            track,
                            neighbor_reasons
                                .get(&track_id)
                                .cloned()
                                .unwrap_or_else(|| "automix: learned similarity".to_string()),
                        )
                        .with_ranking_policy(policy)
                    })
                })
                .collect::<Vec<_>>();
            // One gate for every source: recent plays, early skips, Not for me,
            // hidden content and versions of queued tracks stay out.
            let mut gate = CandidateGate::load(conn, queue_items, None);
            ordered.retain(|selection| gate.admit_track(&selection.track));
            // Taste reaches the learned path too: favorites and artist
            // affinity scale the lane policy the ranker multiplies in.
            let (taste, _) =
                from_session_profile(&build_session_taste_profile(conn, current_track)?);
            for selection in &mut ordered {
                selection.ranking_policy.score_multiplier *=
                    learned_taste_multiplier(&selection.track, &taste);
            }
            // Neighbors arrive in relevance order; the ranker keeps that order
            // and, while mixing, chains fit from the last track already queued
            // so the first appended track follows the queue tail, not the
            // track playing now.
            let chain_from = queue_items
                .iter()
                .rev()
                .find(|item| !item.is_pending && item.track.id > 0)
                .map(|item| item.track.id)
                .unwrap_or(current_track.id);
            ordered = rank_automix_selections(conn, chain_from, ordered);
            ordered = cap_per_artist(
                ordered,
                |selection| selection.track.artist_id,
                (needed / 4).max(2),
                needed,
            );
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
    let (taste, mut seed) = from_session_profile(&session_profile);
    seed.genre_rarity = seed_genre_rarity(conn, &seed.genres);
    if let Some(genres) =
        queue::get_track_genre_evidence(conn, std::slice::from_ref(current_track))?
            .get(&current_track.id)
    {
        for genre in genres {
            let key = normalize_genre_key(&genre.path);
            let confidence = genre.confidence.clamp(0.0, 1.0);
            seed.genre_confidence
                .entry(key)
                .and_modify(|existing| *existing = existing.max(confidence))
                .or_insert(confidence);
        }
    }
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
        let mut gate = CandidateGate::load(conn, queue_items, None);
        let mut fallback =
            build_metadata_fallback(conn, current_track, &excluded_track_ids, needed)?;
        fallback.retain(|track| gate.admit_track(track));
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

    // Widen recall when the pool is artist-thin. A precomputed similar-pool often
    // collapses to a couple of artists (the seed's own catalogue plus one
    // over-connected neighbour), and no per-artist cap can manufacture diversity
    // the pool doesn't hold. Inject an artist-diverse, genre-matched sample (one
    // track per artist sharing the seed's genres) so the scorer and the cap have
    // real variety to choose from. Only fires when diversity is genuinely low, so
    // healthy pools are untouched; the shared-genre boost keeps the additions on
    // vibe, and the hub penalty + cap still apply to everything downstream.
    let distinct_artists = candidates
        .iter()
        .map(|track| track.artist_id)
        .collect::<HashSet<_>>()
        .len();
    if distinct_artists < needed {
        let mut seen = candidates
            .iter()
            .map(|track| track.id)
            .collect::<HashSet<_>>();
        seen.extend(excluded_track_ids.iter().copied());
        seen.insert(current_track.id);
        let diverse = queries::get_genre_diverse_candidates(conn, current_track.id, MAX_CANDIDATES)
            .unwrap_or_default();
        for track in diverse {
            if candidates.len() >= MAX_CANDIDATES {
                break;
            }
            if seen.insert(track.id) {
                candidates.push(track);
            }
        }
    }

    // Same gate as every other source; the first version of a recording in
    // similarity order wins.
    let mut gate = CandidateGate::load(conn, queue_items, None);
    candidates.retain(|track| gate.admit_track(track));
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_genres = queue::get_track_genre_evidence(conn, &candidates)?;

    // Artist-level hub-ness for the candidate pool. The scored fallback draws
    // candidates from track_similarity, where an over-connected artist appears in
    // most seeds' pools; without this discount its tracks tie at the 0.05 floor and
    // the deterministic tie-break surfaces them for any weak-signal seed. Keyed on
    // artist (not track) so an over-represented artist's low-in-degree deep cuts are
    // caught too. Discount them so non-hub genre matches win the tie. Map is
    // artist_id -> max percentile; look up via track.artist_id.
    let mut artist_ids = candidates
        .iter()
        .map(|track| track.artist_id)
        .filter(|id| *id != 0)
        .collect::<Vec<_>>();
    artist_ids.sort_unstable();
    artist_ids.dedup();
    let artist_hub = queries::get_artist_hub_percentiles(conn, &artist_ids).unwrap_or_default();

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

    // Key and tempo fit only shapes the order while transitions are mixed.
    let mixing = mixing_active(conn);
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
        &artist_hub,
        mixing,
    );
    let ordered = decluster_by_album(ordered);
    let ordered = cap_per_artist(
        ordered,
        |track| track.artist_id,
        (needed / 4).max(2),
        needed,
    );
    Ok(ordered
        .into_iter()
        .take(needed)
        .map(|track| {
            let genres = candidate_genres
                .get(&track.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let mut score = automix_score_with_genre_confidence(
                &track,
                genres,
                &taste,
                &seed,
                seed_features.as_ref(),
                candidate_features.get(&track.id),
                mixing,
            );
            // Same hub discount the ordering used, reflected in the score and its
            // "Why" so a hub that got buried can't still claim a clean reason.
            let hub_pct = artist_hub.get(&track.artist_id).copied().unwrap_or(0.0);
            if hub_pct > AUTOMIX_HUB_THRESHOLD {
                score.value *= hub_multiplier(hub_pct);
                score.signals.push(AutomixSignal::penalty("hub"));
            }
            let reason = automix_scored_reason(&score);
            AutomixSelection::new(track, reason)
        })
        .collect())
}

#[cfg(test)]
#[derive(Debug, Serialize)]
pub(crate) struct AutomixEvaluationReport {
    pub(crate) seed_track_id: i64,
    pub(crate) queue_len_before: i64,
    pub(crate) queue_len_after: i64,
    pub(crate) before: Vec<AutomixEvaluationRow>,
    pub(crate) after: Vec<AutomixEvaluationRow>,
}

#[cfg(test)]
#[derive(Debug, Serialize)]
pub(crate) struct AutomixEvaluationRow {
    pub(crate) order: usize,
    pub(crate) track_id: i64,
    pub(crate) title: String,
    pub(crate) artist_name: Option<String>,
    pub(crate) bpm: Option<f64>,
    pub(crate) camelot_key: Option<String>,
    pub(crate) profile_confidence: Option<f64>,
    pub(crate) safe_crossfade_only: bool,
    pub(crate) learned_score: Option<f64>,
    pub(crate) primary_reason: Option<String>,
    pub(crate) reason_tags: Vec<String>,
    pub(crate) chain_score: Option<f64>,
    pub(crate) dj_reasons: Vec<String>,
    pub(crate) final_score: Option<f64>,
    pub(crate) final_reason: Option<String>,
}

#[cfg(test)]
pub(crate) fn evaluate_automix_for_seed(
    conn: &Connection,
    seed_track_id: i64,
    limit: usize,
) -> Result<AutomixEvaluationReport> {
    let queue_len_before = queue_len(conn)?;
    let seed = queue::get_track_by_id(conn, seed_track_id)?
        .ok_or_else(|| anyhow::anyhow!("seed track {seed_track_id} not found"))?;
    let model = queries::get_selected_discovery_embedding_model(conn)
        .ok()
        .flatten();
    let learned_rows = if let Some(model) = model {
        queries::get_track_neighbors(
            conn,
            model.id,
            seed_track_id,
            (limit * 4).max(24) as i64,
            &[],
        )
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    let learned_by_track = learned_rows
        .iter()
        .map(|row| (row.track_id, row.clone()))
        .collect::<HashMap<_, _>>();
    let before_ids = learned_rows
        .iter()
        .take(limit)
        .map(|row| row.track_id)
        .collect::<Vec<_>>();
    let before_tracks = queue::get_tracks_by_ids(conn, &before_ids)?;
    let before_track_map = before_tracks
        .into_iter()
        .map(|track| (track.id, track))
        .collect::<HashMap<_, _>>();
    let before = before_ids
        .into_iter()
        .enumerate()
        .filter_map(|(idx, track_id)| {
            before_track_map.get(&track_id).map(|track| {
                evaluation_row(conn, idx + 1, track, learned_by_track.get(&track.id), None)
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let selected = build_automix_extension_with_reasons(
        conn,
        &seed,
        &[],
        ShuffleMode::Off,
        None,
        limit,
        true,
    )?;
    let after = selected
        .iter()
        .enumerate()
        .map(|(idx, selection)| {
            evaluation_row(
                conn,
                idx + 1,
                &selection.track,
                learned_by_track.get(&selection.track.id),
                selection.reason.as_deref(),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let queue_len_after = queue_len(conn)?;

    Ok(AutomixEvaluationReport {
        seed_track_id,
        queue_len_before,
        queue_len_after,
        before,
        after,
    })
}

#[cfg(test)]
fn evaluation_row(
    conn: &Connection,
    order: usize,
    track: &Track,
    learned: Option<&queries::EmbeddingNeighborRow>,
    final_reason: Option<&str>,
) -> Result<AutomixEvaluationRow> {
    let dsp = queries::get_audio_dsp_features(conn, track.id)
        .ok()
        .flatten();
    let profile = queries::get_audio_dj_profile_for_track(conn, track.id)
        .ok()
        .flatten();
    let safe_crossfade_only = queries::get_audio_dj_profile_correction(
        conn,
        &AudioDjProfileKey {
            media_ref_kind: "library_track".to_string(),
            media_ref_id: track.id.to_string(),
        },
    )
    .ok()
    .flatten()
    .is_some_and(|correction| correction.safe_crossfade_only);
    let tags = learned
        .and_then(|row| row.reason_json.as_deref())
        .map(|reason_json| automix_reason_tags(Some(reason_json)))
        .unwrap_or_default();
    let dj_reasons = final_reason
        .and_then(|reason| reason.split(" | dj: ").nth(1))
        .map(|suffix| suffix.split(" | {").next().unwrap_or(suffix))
        .map(|reasons| {
            reasons
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let chain_score = final_reason.and_then(|reason| reason_json_number(reason, "dj_score"));
    let final_score = chain_score.or_else(|| learned.map(|row| row.score));

    Ok(AutomixEvaluationRow {
        order,
        track_id: track.id,
        title: track.title.clone(),
        artist_name: track.artist_name.clone(),
        bpm: dsp.as_ref().and_then(|features| features.bpm),
        camelot_key: dsp
            .as_ref()
            .and_then(|features| features.camelot_key.clone()),
        profile_confidence: profile.as_ref().map(|profile| profile.profile_confidence),
        safe_crossfade_only,
        learned_score: learned.map(|row| row.score),
        primary_reason: learned.and_then(|row| row.primary_reason.clone()),
        reason_tags: tags,
        chain_score,
        dj_reasons,
        final_score,
        final_reason: final_reason.map(str::to_string),
    })
}

#[cfg(test)]
fn reason_json_number(reason: &str, key: &str) -> Option<f64> {
    let start = reason.rfind('{')?;
    serde_json::from_str::<serde_json::Value>(&reason[start..])
        .ok()?
        .get(key)?
        .as_f64()
}

#[cfg(test)]
fn queue_len(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM queue", [], |row| row.get(0))
        .map_err(Into::into)
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

fn automix_neighbor_policy(row: &queries::EmbeddingNeighborRow) -> GeneratedCandidatePolicy {
    let mut multiplier = 1.0;
    let mut policy_reasons = Vec::new();
    let reason_tags = automix_reason_tags(row.reason_json.as_deref());
    let has_tag = |tag: &str| reason_tags.iter().any(|value| value == tag);

    let has_lastfm_direct = has_tag("lastfm_direct");
    let has_lastfm_branch = has_tag("lastfm_branch");
    if has_lastfm_direct {
        multiplier *= 1.06;
        policy_reasons.push("lastfm direct");
    }
    if has_lastfm_branch {
        multiplier *= 0.90;
        policy_reasons.push("lastfm branch");
    }

    // Models before trainer v3 call the metadata proxy "audio_texture".
    let is_texture = |tag: &str| matches!(tag, "metadata_similarity" | "audio_texture");
    let primary_is_texture = row.primary_reason.as_deref().is_some_and(is_texture);
    let has_texture = primary_is_texture || reason_tags.iter().any(|tag| is_texture(tag));
    let has_supporting_reason = reason_tags.iter().any(|tag| {
        matches!(
            tag.as_str(),
            "artist_affinity"
                | "genre_branch"
                | "album_context"
                | "bpm_match"
                | "harmonic_match"
                | "energy_match"
                | "behavioral"
                | "direct_transition"
                | "lastfm_direct"
                | "lastfm_branch"
        )
    }) || row.behavioral_score > 0.35
        || row.metadata_score > 0.0
        || row.support_transition > 0.0
        || row.support_structure > 0.0
        || row.support_colisten > 0.0;

    if has_texture && !has_supporting_reason {
        multiplier *= 0.82;
        policy_reasons.push("texture-only learned signal");
    }

    // Hub penalty: a candidate that's a top neighbour for a large share of seeds
    // is a graph hub, not a genre match. Discount it the same way radio does so
    // popular hubs don't dominate every automix queue. `candidate_in_degree_percentile`
    // is 0 for tracks the trainer never saw as a neighbour, leaving them untouched.
    if row.candidate_in_degree_percentile > AUTOMIX_HUB_THRESHOLD {
        multiplier *= hub_multiplier(row.candidate_in_degree_percentile);
        policy_reasons.push("hub penalty");
    }

    GeneratedCandidatePolicy {
        score_multiplier: multiplier,
        reasons: policy_reasons,
    }
}

fn automix_reason_tags(reason_json: Option<&str>) -> Vec<String> {
    reason_json
        .and_then(|raw| serde_json::from_str::<Vec<serde_json::Value>>(raw).ok())
        .map(|values| {
            values
                .into_iter()
                .filter_map(|value| {
                    value
                        .get("key")
                        .and_then(|key| key.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
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
    if result.len() < needed
        && let Some(album_id) = seed.album_id
    {
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
    candidate_genres: &HashMap<i64, Vec<queue::TrackGenreEvidence>>,
    taste: &TasteVector,
    seed: &SeedContext,
    needed: usize,
    shuffle_seed: Option<i64>,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: &HashMap<i64, AudioDspFeatures>,
    artist_hub: &HashMap<i64, f64>,
    mixing: bool,
) -> Vec<Track> {
    let mut scored = candidates
        .into_iter()
        .map(|track| {
            let mut score = automix_score_with_genre_confidence(
                &track,
                candidate_genres
                    .get(&track.id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                taste,
                seed,
                seed_features,
                candidate_features.get(&track.id),
                mixing,
            )
            .value;
            // Discount hub artists so they sink below non-hub matches at the 0.05
            // floor. Applied after automix_score's floor, so a hub can score below
            // 0.05 and lose the otherwise-alphabetical title tie-break.
            score *= hub_multiplier(artist_hub.get(&track.artist_id).copied().unwrap_or(0.0));
            ScoredTrack { track, score }
        })
        .collect::<Vec<_>>();

    scored.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            // Score ties are common at the 0.05 floor. Break them on quality and
            // freshness (higher fidelity, then less-played) rather than alphabetically
            // by title, which systematically favoured early-titled tracks for no
            // recommendation reason. id last only as a stable final key.
            .then_with(|| right.track.fidelity_score.cmp(&left.track.fidelity_score))
            .then_with(|| left.track.play_count.cmp(&right.track.play_count))
            .then_with(|| left.track.id.cmp(&right.track.id))
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
            let genre_paths = candidate_genres
                .iter()
                .map(|(&track_id, genres)| {
                    (
                        track_id,
                        genres.iter().map(|genre| genre.path.clone()).collect(),
                    )
                })
                .collect::<HashMap<i64, Vec<String>>>();
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
                    genre_shuffle_with_rng(&preferred, &genre_paths, &mut preferred_rng),
                    genre_shuffle_with_rng(&fallback, &genre_paths, &mut fallback_rng),
                )
            } else {
                (
                    genre_shuffle(&preferred, &genre_paths),
                    genre_shuffle(&fallback, &genre_paths),
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

// Cap any single artist's share of one extension batch. The hub penalty demotes
// artists that are over-connected in the similarity graph, but an artist can
// dominate a weak-signal pool for reasons hub-ness never sees - a large catalogue,
// the seed's own artist, co-listen bias - and the extension is meant to be diverse
// (same-artist gets only a gentle boost precisely to avoid runs). This is the
// general backstop: keep the highest-scored `max_per_artist` from each artist,
// then, only if that left the batch short, backfill the dropped ones round-robin
// by artist so a low-diversity library still gets a full queue that stays as
// spread as the pool allows, rather than a starved one or a front-loaded run of
// the dominant artist. Items must already be score-ordered; relative order within
// an artist is preserved. Generic over the item so both the learned
// (AutomixSelection) and scored (Track) paths share it.
fn cap_per_artist<T>(
    items: Vec<T>,
    artist_id: impl Fn(&T) -> i64,
    max_per_artist: usize,
    needed: usize,
) -> Vec<T> {
    let mut counts: HashMap<i64, usize> = HashMap::new();
    let mut kept = Vec::with_capacity(items.len());
    // Overflow bucketed by artist, first-seen order preserved, so backfill can
    // round-robin instead of draining the highest-scored artist first.
    let mut bucket_order: Vec<i64> = Vec::new();
    let mut buckets: HashMap<i64, VecDeque<T>> = HashMap::new();
    for item in items {
        let id = artist_id(&item);
        // artist_id 0 means "unknown artist"; never collapse those together.
        let count = counts.entry(id).or_insert(0);
        if id == 0 || *count < max_per_artist {
            *count += 1;
            kept.push(item);
        } else {
            if !buckets.contains_key(&id) {
                bucket_order.push(id);
            }
            buckets.entry(id).or_default().push_back(item);
        }
    }

    while kept.len() < needed {
        let mut progressed = false;
        for id in &bucket_order {
            if let Some(item) = buckets.get_mut(id).and_then(VecDeque::pop_front) {
                kept.push(item);
                progressed = true;
                if kept.len() >= needed {
                    break;
                }
            }
        }
        // Every bucket is empty - the pool simply has nothing more to offer.
        if !progressed {
            break;
        }
    }
    kept
}

/// Score a candidate for automix selection *and* emit the signals that
/// produced that score. The reason shown to the user is built from these
/// signals (see `automix_scored_reason`), so the explanation is derived from
/// the same pass that ranked the track - it cannot drift from or contradict
/// the score. `value` is byte-identical to the pre-signal scorer.
#[cfg(test)]
pub(crate) fn automix_score(
    track: &Track,
    genres: &[String],
    taste: &TasteVector,
    seed: &SeedContext,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: Option<&AudioDspFeatures>,
) -> AutomixScore {
    let genres = genres
        .iter()
        .map(|path| queue::TrackGenreEvidence {
            path: path.clone(),
            confidence: 1.0,
        })
        .collect::<Vec<_>>();
    automix_score_with_genre_confidence(
        track,
        &genres,
        taste,
        seed,
        seed_features,
        candidate_features,
        true,
    )
}

/// Taste on the learned path: favorites and artist affinity (the same net the
/// fallback scorer uses) scale the lane policy, bounded to 0.6..1.4.
fn learned_taste_multiplier(track: &Track, taste: &TasteVector) -> f64 {
    let mut multiplier = if track.is_favorite { 1.2 } else { 1.0 };
    if track.artist_id != 0
        && let Some(affinity) = taste.artist_affinity.get(&track.artist_id)
    {
        let net = affinity.pos * 0.5 - affinity.neg * 0.65;
        multiplier *= (1.0 + 0.1 * net).clamp(0.6, 1.4);
    }
    multiplier
}

/// Full-overlap genre match is worth this much relevance on top of the 1.0 base.
const AUTOMIX_GENRE_WEIGHT: f64 = 2.5;

/// Relevance first, penalties last: genre overlap (weighted Jaccard, so extra
/// tags dilute instead of stacking) and taste affinities build relevance;
/// familiarity boosts scale it; the tamed harmonic fit nudges it while
/// transitions are mixed; skip, recency and energy-whiplash penalties multiply
/// the result so a shared genre can no longer cancel a skip.
fn automix_score_with_genre_confidence(
    track: &Track,
    genres: &[queue::TrackGenreEvidence],
    taste: &TasteVector,
    seed: &SeedContext,
    seed_features: Option<&AudioDspFeatures>,
    candidate_features: Option<&AudioDspFeatures>,
    mixing: bool,
) -> AutomixScore {
    let mut signals = Vec::new();

    // -- Relevance -----------------------------------------------------------
    let mut relevance = 1.0;

    if track.artist_id != 0
        && let Some(affinity) = taste.artist_affinity.get(&track.artist_id)
    {
        relevance += affinity.pos * 0.5;
        relevance -= affinity.neg * 0.65;
        // Label by the net effect on the score, not the raw counts.
        let net = affinity.pos * 0.5 - affinity.neg * 0.65;
        if net > 0.0 {
            signals.push(AutomixSignal::boost("artist affinity"));
        } else if net < 0.0 {
            signals.push(AutomixSignal::penalty("recent skip penalty"));
        }
    }

    // Weighted Jaccard between the seed's genres and the candidate's: each
    // genre weighs by rarity (0.5, the absent-data default, weighs 1.0), and a
    // shared genre counts only as strongly as the weaker side believes it.
    let genre_weight = |genre: &str| {
        let rarity = seed.genre_rarity.get(genre).copied().unwrap_or(0.5);
        0.7 + 0.6 * rarity
    };
    let mut shared_weight = 0.0;
    let mut candidate_only_weight = 0.0;
    let mut genre_affinity_net = 0.0;
    let mut seen = HashSet::new();
    for genre in genres {
        let key = normalize_genre_key(&genre.path);
        if !seen.insert(key.clone()) {
            continue;
        }
        let candidate_confidence = genre.confidence.clamp(0.0, 1.0);
        let in_seed = seed.genres.contains(&key);
        let match_confidence = if in_seed {
            let seed_confidence = seed
                .genre_confidence
                .get(&key)
                .copied()
                .unwrap_or(1.0)
                .clamp(0.0, 1.0);
            candidate_confidence.min(seed_confidence)
        } else {
            0.0
        };
        if in_seed {
            shared_weight += genre_weight(&key) * match_confidence;
        } else {
            candidate_only_weight += genre_weight(&key);
        }
        if let Some(affinity) = taste.genre_affinity.get(&key) {
            let net = affinity.pos * 0.4 - affinity.neg * 0.5;
            let affinity_confidence = if in_seed {
                match_confidence
            } else {
                candidate_confidence
            };
            relevance += net * affinity_confidence;
            genre_affinity_net += net * affinity_confidence;
        }
    }
    let seed_weight: f64 = seed.genres.iter().map(|genre| genre_weight(genre)).sum();
    let union_weight = seed_weight + candidate_only_weight;
    if shared_weight > 0.0 && union_weight > 0.0 {
        relevance += AUTOMIX_GENRE_WEIGHT * shared_weight / union_weight;
        signals.push(AutomixSignal::boost("shared genres"));
    }
    if genre_affinity_net > 0.0 {
        signals.push(AutomixSignal::boost("genre affinity"));
    } else if genre_affinity_net < 0.0 {
        signals.push(AutomixSignal::penalty("genre mismatch"));
    }

    relevance += (track.fidelity_score.max(0) as f64) * 0.003;
    let mut score = relevance.max(0.05);

    // -- Familiarity boosts --------------------------------------------------
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
    }

    // -- Harmonic fit, tamed -------------------------------------------------
    // Only while mixing, and only when BOTH tracks have features; unanalyzed
    // tracks are never penalised. The raw multiplier swings x0.39..x3.96, so it
    // is tamed with the shared DJ ranker's bounds.
    let mut whiplash = false;
    if let (Some(seed), Some(cand)) = (seed_features, candidate_features) {
        if mixing {
            score *= dj_fit_multiplier(compute_harmonic_multiplier(
                seed.camelot_key.as_deref(),
                cand.camelot_key.as_deref(),
                seed.bpm,
                cand.bpm,
            ));
        }

        // The multiplier folds Camelot *and* BPM together, so it can read >1.0
        // even on a key clash that happens to share a tempo. Derive the
        // harmonic signal from the Camelot relationship directly - via the same
        // `camelot_relation` the multiplier uses - so the "Why" never claims a
        // fit the keys don't have, and the two can't drift apart.
        if mixing
            && let (Some(a), Some(b)) = (seed.camelot_key.as_deref(), cand.camelot_key.as_deref())
        {
            signals.push(match camelot_relation(a, b) {
                CamelotRelation::Compatible => AutomixSignal::boost("harmonic match"),
                CamelotRelation::Adjacent => AutomixSignal::boost("adjacent key"),
                CamelotRelation::Clash => AutomixSignal::penalty("key clash"),
            });
        }
        whiplash = matches!(
            (seed.energy, cand.energy),
            (Some(seed_energy), Some(cand_energy)) if (seed_energy - cand_energy).abs() > 0.5
        );
    }

    // -- Penalties, applied last ---------------------------------------------
    if taste.skipped_track_ids.contains(&track.id) {
        score *= 0.1;
        signals.push(AutomixSignal::penalty("recently skipped"));
    }
    if track.play_count > 0
        && let Some(last_played) = track.last_played_at.as_deref()
    {
        // Time-decay penalty: half weight at <1 day, fading to none by 14 days.
        let days_since = parse_days_since_last_played(last_played);
        if days_since < 14.0 {
            score *= 0.5 + 0.5 * (days_since / 14.0);
            signals.push(AutomixSignal::penalty("recently played"));
        }
    }
    if whiplash {
        score *= 0.7;
        signals.push(AutomixSignal::penalty("energy whiplash"));
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

fn matches_preferred_genres(
    genres: &[queue::TrackGenreEvidence],
    taste: &TasteVector,
    seed: &SeedContext,
) -> bool {
    genres.iter().any(|genre| {
        let key = normalize_genre_key(&genre.path);
        let candidate_confidence = genre.confidence.clamp(0.0, 1.0);
        let seed_confidence = seed
            .genre_confidence
            .get(&key)
            .copied()
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        (seed.genres.contains(&key) && candidate_confidence.min(seed_confidence) >= 0.5)
            || (candidate_confidence >= 0.5
                && taste
                    .genre_affinity
                    .get(&key)
                    .is_some_and(|affinity| affinity.pos > 0.0))
    })
}

fn weighted_session_shuffle(entries: &[ScoredTrack]) -> Vec<Track> {
    let mut rng = rand::rng();
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
            let uniform = rng.random_range(f64::EPSILON..1.0);
            let key = -uniform.ln() / weight;
            (key, entry.track.clone())
        })
        .collect::<Vec<_>>();

    weighted.sort_by(|left, right| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal));
    weighted.into_iter().map(|(_, track)| track).collect()
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
            artist_tidal_id: None,
            album_tidal_id: None,
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

    fn create_dsp_schema(conn: &Connection) {
        conn.execute_batch(
            "
            CREATE TABLE audio_dsp_features (
                track_id INTEGER PRIMARY KEY,
                bpm REAL,
                key_signature TEXT,
                camelot_key TEXT,
                loudness_lufs REAL,
                energy REAL,
                danceability REAL,
                beat_strength REAL,
                spectral_centroid REAL,
                stereo_width REAL,
                is_instrumental INTEGER NOT NULL DEFAULT 0,
                analysis_source TEXT NOT NULL DEFAULT 'test',
                analysis_offset_ms INTEGER NOT NULL DEFAULT 0,
                samples_analyzed INTEGER,
                analyzed_at TEXT NOT NULL DEFAULT '2026-01-01T00:00:00Z',
                analysis_version TEXT NOT NULL DEFAULT 'test'
            );
            ",
        )
        .expect("dsp schema");
    }

    fn insert_dsp(conn: &Connection, track_id: i64, bpm: f64, camelot_key: &str) {
        conn.execute(
            "INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES (?1, ?2, ?3)",
            params![track_id, bpm, camelot_key],
        )
        .expect("insert dsp");
    }

    fn policy(multiplier: f64, reason: &'static str) -> GeneratedCandidatePolicy {
        GeneratedCandidatePolicy {
            score_multiplier: multiplier,
            reasons: vec![reason],
        }
    }

    // Minimal neighbour row carrying only the fields the policy logic reads;
    // everything else is zeroed so a test can isolate one signal at a time.
    fn neighbor_row(track_id: i64, in_degree_pct: f64) -> queries::EmbeddingNeighborRow {
        queries::EmbeddingNeighborRow {
            track_id,
            title: format!("track {track_id}"),
            artist_name: None,
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            best_quality: None,
            score: 1.0,
            behavioral_score: 0.5,
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
            candidate_in_degree_percentile: in_degree_pct,
            play_count_seed: 0,
            play_count_candidate: 0,
            primary_reason: Some("behavioral".to_string()),
        }
    }

    #[test]
    fn hub_candidates_are_penalized_relative_to_non_hubs() {
        // Below-threshold tracks (incl. percentile 0 and a mid 0.5) are untouched
        // at 1.0; only the top of the distribution is treated as a hub, and a 0.99
        // hub is hit strictly harder than a 0.90 one.
        let non_hub = automix_neighbor_policy(&neighbor_row(1, 0.0));
        let mid = automix_neighbor_policy(&neighbor_row(2, 0.5));
        let near_threshold = automix_neighbor_policy(&neighbor_row(3, 0.90));
        let strong_hub = automix_neighbor_policy(&neighbor_row(4, 0.99));

        assert!((non_hub.score_multiplier - 1.0).abs() < 1e-9);
        assert!((mid.score_multiplier - 1.0).abs() < 1e-9);
        assert!(near_threshold.score_multiplier < 1.0);
        assert!(strong_hub.score_multiplier < near_threshold.score_multiplier);
        assert!(strong_hub.reasons.contains(&"hub penalty"));
        assert!(!mid.reasons.contains(&"hub penalty"));
    }

    #[test]
    fn scorer_weights_rare_seed_genre_above_broad_one() {
        use crate::smart::taste_vector::{SeedContext, TasteVector};
        let taste = TasteVector::default();
        let seed = SeedContext {
            genres: ["rare".to_string(), "broad".to_string()]
                .into_iter()
                .collect(),
            genre_rarity: [("rare".to_string(), 0.95), ("broad".to_string(), 0.10)]
                .into_iter()
                .collect(),
            ..SeedContext::default()
        };

        let track = track_with_album(2, None);
        let rare = automix_score(&track, &["rare".to_string()], &taste, &seed, None, None).value;
        let broad = automix_score(&track, &["broad".to_string()], &taste, &seed, None, None).value;
        assert!(
            rare > broad,
            "a rare shared genre ({rare}) should beat a broad one ({broad})"
        );

        // Absent rarity data (other consumers, older fixtures) keeps flat weighting:
        // the two genres score identically.
        let flat_seed = SeedContext {
            genres: seed.genres.clone(),
            ..SeedContext::default()
        };
        let rare_flat = automix_score(
            &track,
            &["rare".to_string()],
            &taste,
            &flat_seed,
            None,
            None,
        )
        .value;
        let broad_flat = automix_score(
            &track,
            &["broad".to_string()],
            &taste,
            &flat_seed,
            None,
            None,
        )
        .value;
        assert!((rare_flat - broad_flat).abs() < 1e-9);
    }

    fn dsp(bpm: f64, key: &str) -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 0,
            bpm: Some(bpm),
            key_signature: None,
            camelot_key: Some(key.to_string()),
            loudness_lufs: None,
            energy: None,
            danceability: None,
            beat_strength: None,
            spectral_centroid: None,
            stereo_width: None,
            is_instrumental: false,
            analysis_source: "test".to_string(),
            analysis_offset_ms: 0,
            samples_analyzed: None,
            analyzed_at: "2026-01-01T00:00:00Z".to_string(),
            analysis_version: "test".to_string(),
        }
    }

    fn two_genre_seed() -> crate::smart::taste_vector::SeedContext {
        crate::smart::taste_vector::SeedContext {
            genres: ["house".to_string(), "deep house".to_string()]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn scorer_skip_penalty_survives_shared_genres() {
        use crate::smart::taste_vector::TasteVector;
        let seed = two_genre_seed();
        let genres = ["house".to_string(), "deep house".to_string()];
        let track = track_with_album(7, None);
        let fresh =
            automix_score(&track, &genres, &TasteVector::default(), &seed, None, None).value;
        let mut skipped_taste = TasteVector::default();
        skipped_taste.skipped_track_ids.insert(7);
        let skipped = automix_score(&track, &genres, &skipped_taste, &seed, None, None).value;
        assert!(
            skipped <= fresh * 0.15,
            "a skip must stay a strong penalty: {skipped} vs {fresh}"
        );
    }

    #[test]
    fn scorer_prefers_a_closer_genre_match_over_more_tags() {
        use crate::smart::taste_vector::TasteVector;
        let seed = two_genre_seed();
        let track = track_with_album(8, None);
        let exact = automix_score(
            &track,
            &["house".to_string(), "deep house".to_string()],
            &TasteVector::default(),
            &seed,
            None,
            None,
        )
        .value;
        let sprawling = automix_score(
            &track,
            &[
                "house".to_string(),
                "deep house".to_string(),
                "techno".to_string(),
                "trance".to_string(),
                "ambient".to_string(),
            ],
            &TasteVector::default(),
            &seed,
            None,
            None,
        )
        .value;
        assert!(exact > sprawling, "{exact} vs {sprawling}");
    }

    #[test]
    fn fallback_order_ignores_key_and_tempo_while_mixing_is_off() {
        let db = crate::db::Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, name) VALUES (1, 'Seed'), (2, 'Two'), (3, 'Three');
                 INSERT INTO tracks (id, title, artist_id, duration_ms, source, is_library, is_favorite)
                 VALUES (1, 'Seed', 1, 200000, 'tidal', 1, 0),
                        (2, 'Preferred', 2, 200000, 'tidal', 1, 1),
                        (3, 'Other', 3, 200000, 'tidal', 1, 0);
                 UPDATE playback_state SET crossfade_ms = 0 WHERE id = 1;
                 INSERT INTO queue (id, track_id, position, source) VALUES (1, 1, 0, 'user');
                 INSERT INTO track_similarity (track_a, track_b, similarity_score)
                 VALUES (1, 2, 0.9), (1, 3, 0.8);
                 INSERT INTO audio_dsp_features (track_id, bpm, camelot_key)
                 VALUES (1, 124.0, '8A'), (2, 145.0, '3B'), (3, 124.0, '8A');",
            )?;
            assert!(!mixing_active(conn));
            let current = queue::get_track_by_id(conn, 1)?.unwrap();
            let items = queue::load_queue(conn)?;
            let ranked = build_automix_extension_with_reasons(
                conn,
                &current,
                &items,
                ShuffleMode::Off,
                None,
                1,
                false,
            )?;
            assert_eq!(ranked[0].track.id, 2, "key and tempo reordered automix with mixing off");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn scorer_keeps_harmonic_swing_within_bounds() {
        use crate::smart::taste_vector::{SeedContext, TasteVector};
        let taste = TasteVector::default();
        let seed = SeedContext::default();
        let track = track_with_album(9, None);
        let seed_dsp = dsp(124.0, "8A");
        let neutral = automix_score(&track, &[], &taste, &seed, None, None).value;
        let fit = automix_score(
            &track,
            &[],
            &taste,
            &seed,
            Some(&seed_dsp),
            Some(&dsp(124.0, "8A")),
        )
        .value;
        let clash = automix_score(
            &track,
            &[],
            &taste,
            &seed,
            Some(&seed_dsp),
            Some(&dsp(160.0, "2B")),
        )
        .value;
        assert!(fit <= neutral * 1.25 + 1e-9, "{fit} vs {neutral}");
        assert!(clash >= neutral * 0.8 - 1e-9, "{clash} vs {neutral}");
        assert!(fit > neutral && clash < neutral);
    }

    #[test]
    fn scorer_caps_shared_genre_signal_at_both_tracks_confidence() {
        use crate::playback::queue::TrackGenreEvidence;
        use crate::smart::taste_vector::{AffinitySignal, SeedContext, TasteVector};

        let mut taste = TasteVector::default();
        let track = track_with_album(2, None);
        let strong_genre = [TrackGenreEvidence {
            path: "Genres > Electronic > House".to_string(),
            confidence: 1.0,
        }];
        let weak_genre = [TrackGenreEvidence {
            path: "Genres > Electronic > House".to_string(),
            confidence: 0.1,
        }];
        let mut seed = SeedContext::default();
        seed.genres
            .insert("genres > electronic > house".to_string());
        taste.genre_affinity.insert(
            "genres > electronic > house".to_string(),
            AffinitySignal { pos: 2.2, neg: 0.0 },
        );

        let strong = automix_score_with_genre_confidence(
            &track,
            &strong_genre,
            &taste,
            &seed,
            None,
            None,
            false,
        )
        .value;
        let weak_candidate = automix_score_with_genre_confidence(
            &track,
            &weak_genre,
            &taste,
            &seed,
            None,
            None,
            false,
        )
        .value;
        assert!(strong > weak_candidate);

        seed.genre_confidence
            .insert("genres > electronic > house".to_string(), 0.1);
        let weak_seed = automix_score_with_genre_confidence(
            &track,
            &strong_genre,
            &taste,
            &seed,
            None,
            None,
            false,
        )
        .value;
        assert!((weak_seed - weak_candidate).abs() < 1e-9);
    }

    #[test]
    fn hub_multiplier_is_flat_below_threshold_and_ramps_above() {
        assert_eq!(hub_multiplier(0.0), 1.0);
        assert_eq!(hub_multiplier(AUTOMIX_HUB_THRESHOLD), 1.0);
        assert!(hub_multiplier(0.99) < hub_multiplier(0.90));
        assert!(hub_multiplier(0.99) < 0.5);
    }

    fn track_by_artist(id: i64, artist_id: i64) -> Track {
        let mut track = track_with_album(id, Some(id));
        track.artist_id = artist_id;
        track
    }

    #[test]
    fn cap_per_artist_limits_one_artist_but_keeps_others() {
        // Five tracks by artist 1, then artists 2 and 3. Cap of 2 keeps the first
        // two of artist 1 plus the others; no artist exceeds the cap when supply
        // allows. Order among the kept items is preserved.
        let input = vec![
            track_by_artist(1, 1),
            track_by_artist(2, 1),
            track_by_artist(3, 1),
            track_by_artist(4, 1),
            track_by_artist(5, 1),
            track_by_artist(6, 2),
            track_by_artist(7, 3),
        ];
        let out = cap_per_artist(input, |t| t.artist_id, 2, 4);
        let artist1 = out.iter().filter(|t| t.artist_id == 1).count();
        assert_eq!(artist1, 2);
        assert!(out.iter().any(|t| t.artist_id == 2));
        assert!(out.iter().any(|t| t.artist_id == 3));
        assert_eq!(out[0].id, 1);
    }

    #[test]
    fn cap_per_artist_backfills_rather_than_starving_a_thin_library() {
        // Everything is one artist and there is nothing else to reach for: the cap
        // must not shrink the batch below `needed` - it backfills the dropped ones.
        let input = (1..=6).map(|id| track_by_artist(id, 1)).collect::<Vec<_>>();
        let out = cap_per_artist(input, |t| t.artist_id, 2, 5);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn learned_policy_penalizes_texture_only_when_dj_fit_disagrees() {
        let conn = Connection::open_in_memory().expect("db");
        create_dsp_schema(&conn);
        insert_dsp(&conn, 1, 120.0, "1A");
        insert_dsp(&conn, 2, 145.0, "6B");
        insert_dsp(&conn, 3, 121.0, "2A");

        let ranked = rank_automix_selections(
            &conn,
            1,
            vec![
                AutomixSelection::new(track_with_album(2, None), "automix: audio texture")
                    .with_ranking_policy(policy(0.82, "texture-only learned signal")),
                AutomixSelection::new(track_with_album(3, None), "automix: learned similarity"),
            ],
        );

        assert_eq!(ranked[0].track.id, 3);
        assert_eq!(ranked[1].track.id, 2);
        assert!(
            ranked[1]
                .reason
                .as_deref()
                .unwrap_or_default()
                .contains("texture-only learned signal")
        );
    }

    #[test]
    fn lastfm_direct_is_confidence_not_override() {
        let conn = Connection::open_in_memory().expect("db");
        create_dsp_schema(&conn);
        insert_dsp(&conn, 1, 120.0, "1A");
        insert_dsp(&conn, 2, 120.5, "1A");
        insert_dsp(&conn, 3, 120.5, "1A");

        let tied_facts = rank_automix_selections(
            &conn,
            1,
            vec![
                AutomixSelection::new(track_with_album(3, None), "automix: lastfm branch")
                    .with_ranking_policy(policy(0.90, "lastfm branch")),
                AutomixSelection::new(track_with_album(2, None), "automix: lastfm direct")
                    .with_ranking_policy(policy(1.06, "lastfm direct")),
            ],
        );
        assert_eq!(tied_facts[0].track.id, 2);

        // While mixing, a clearly better transition outweighs a slightly
        // more trusted lane.
        let conn = Connection::open_in_memory().expect("db");
        create_dsp_schema(&conn);
        conn.execute_batch(
            "CREATE TABLE playback_state (id INTEGER PRIMARY KEY, crossfade_ms INTEGER);
             INSERT INTO playback_state (id, crossfade_ms) VALUES (1, 4000);",
        )
        .expect("mixing on");
        insert_dsp(&conn, 1, 120.0, "1A");
        insert_dsp(&conn, 2, 145.0, "6B");
        insert_dsp(&conn, 3, 120.5, "1A");

        let better_fit_branch = rank_automix_selections(
            &conn,
            1,
            vec![
                AutomixSelection::new(track_with_album(2, None), "automix: lastfm direct")
                    .with_ranking_policy(policy(1.06, "lastfm direct")),
                AutomixSelection::new(track_with_album(3, None), "automix: lastfm branch")
                    .with_ranking_policy(policy(0.90, "lastfm branch")),
            ],
        );
        assert_eq!(better_fit_branch[0].track.id, 3);
    }

    #[test]
    #[ignore]
    fn automix_diagnostic_for_seed() {
        let db_path = crate::paths::resolve_db_path_from_env();
        let limit = std::env::var("NOOR_AUTOMIX_LIMIT")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(8);
        let seeds = std::env::var("NOOR_AUTOMIX_SEEDS")
            .ok()
            .or_else(|| std::env::var("NOOR_SEED").ok())
            .unwrap_or_else(|| "1".to_string())
            .split(',')
            .filter_map(|value| value.trim().parse::<i64>().ok())
            .collect::<Vec<_>>();
        let db = crate::db::Database::open(&db_path).expect("open db");

        for seed in seeds {
            let report = db
                .with_conn(|conn| evaluate_automix_for_seed(conn, seed, limit))
                .expect("evaluate automix");
            println!("{}", serde_json::to_string_pretty(&report).expect("json"));
            println!("seed {}", report.seed_track_id);
            println!("before:");
            for row in &report.before {
                println!(
                    "#{:<2} {:<6} {:<32} bpm={:?} key={:?} learned={:?} primary={:?}",
                    row.order,
                    row.track_id,
                    row.title.chars().take(32).collect::<String>(),
                    row.bpm,
                    row.camelot_key,
                    row.learned_score,
                    row.primary_reason
                );
            }
            println!("after:");
            for row in &report.after {
                println!(
                    "#{:<2} {:<6} {:<32} bpm={:?} key={:?} final={:?} reason={:?}",
                    row.order,
                    row.track_id,
                    row.title.chars().take(32).collect::<String>(),
                    row.bpm,
                    row.camelot_key,
                    row.final_score,
                    row.final_reason
                );
            }
            assert_eq!(report.queue_len_before, report.queue_len_after);
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
