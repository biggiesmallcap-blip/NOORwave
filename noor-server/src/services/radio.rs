//! Song Radio orchestrator.
//!
//! Given a seed (track | album | artist), fans out to three sources in parallel:
//!   - Library: embedding neighbors via discovery_learning::radio_from_neighbors
//!   - Last.fm: track.getSimilar resolved to Tidal IDs (Task 3)
//!   - Engine:  external_discovery_engine (slot exists; v1 produces empty)
//!
//! Applies a blend (Familiar/Mixed/Adventurous), ISRC-dedups with library
//! preference, tags each result with provenance, returns a queue.

use crate::db::Database;
use crate::metadata::lastfm::LastFmClient;
use crate::smart::artist_resolver::ArtistResolver;
use crate::smart::taste_vector::TasteVector;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RadioBlend {
    Familiar,
    Mixed,
    Adventurous,
}

impl Default for RadioBlend {
    fn default() -> Self {
        RadioBlend::Mixed
    }
}

impl RadioBlend {
    /// Returns (library_weight, lastfm_weight, engine_weight) summing to 1.0.
    pub fn weights(self) -> (f64, f64, f64) {
        match self {
            RadioBlend::Familiar => (0.60, 0.30, 0.10),
            RadioBlend::Mixed => (0.30, 0.40, 0.30),
            RadioBlend::Adventurous => (0.10, 0.40, 0.50),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RadioSource {
    Library,
    Lastfm,
    Engine,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioCandidate {
    /// Library track id when `is_in_library`; otherwise the resolved Tidal id (best-effort).
    /// Used as a stable canvas/queue identifier.
    pub track_id: i64,
    /// For playback. Always set when known.
    pub tidal_track_id: Option<i64>,
    pub title: String,
    pub artist_name: String,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub isrc: Option<String>,
    pub is_in_library: bool,
    pub source: RadioSource,
    /// Human-readable explanation for the hover-card "Why is this here?" line.
    pub reason: String,
    /// 0..1 source-native score, normalized for cross-source comparison.
    pub similarity_score: f64,
    /// Tier 1 diagnostics carried from the neighbor row. `None` for non-library
    /// candidates (Last.fm, engine table) — they have no neighbor metadata.
    /// Read by Tier 2 flag-gated steps; ignored when flags are off.
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub candidate_in_degree_percentile: Option<f64>,
    #[serde(default)]
    pub support_count: Option<i64>,
    #[serde(default)]
    pub primary_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioSeed {
    pub kind: &'static str, // "track" | "album" | "artist"
    pub track_id: Option<i64>,
    pub album_id: Option<i64>,
    pub artist_id: Option<i64>,
    pub title: String,
    pub artist_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RadioQueue {
    pub session_id: String,
    pub blend_used: RadioBlend,
    pub seed: RadioSeed,
    pub tracks: Vec<RadioCandidate>,
}

// ─── Public orchestrators ────────────────────────────────────────────────────

/// Build a Song Radio queue seeded from a single library track.
pub async fn orchestrate_song(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_track_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    // Load Tier 2 feature flags. The kill-switch short-circuits to a path
    // *equivalent* to the legacy pipeline; in this validation-gate stage there
    // are no behavior changes after it yet, so the kill-switch is currently a
    // no-op semantically. It exists so steps 7-9 can flip behaviors on without
    // touching the entry-point logic, and so an operator can revert to legacy
    // by flipping a single config row.
    let flags = db
        .with_conn(|conn| Ok(crate::services::radio_config::load_radio_flags(conn)))
        .unwrap_or(crate::services::radio_config::RadioFlags::all_off());

    let exclude_set: HashSet<i64> = exclude_track_ids.iter().copied().collect();
    let id = seed_track_id;
    let seed_meta = db
        .with_conn(move |conn| crate::db::queries::load_external_seed_from_track(conn, id))?
        .ok_or_else(|| anyhow::anyhow!("seed track not found: {seed_track_id}"))?;

    let seed_for_session = RadioSeed {
        kind: "track",
        track_id: Some(seed_track_id),
        album_id: None,
        artist_id: None,
        title: seed_meta.title.clone(),
        artist_name: seed_meta.artist_name.clone(),
    };

    // Build per-request taste signals + artist resolver. If the seed
    // track or session profile fails to load, log and fall back to an
    // empty TasteVector so taste-aware adjustments become a no-op
    // rather than failing the whole radio request. Resolver miss is
    // not fatal — it just means no affinity nudges this call.
    let (taste, resolver) = build_taste_inputs(db, seed_track_id);

    let (lib_w, lfm_w, eng_w) = blend.weights();
    let target_per_source = |w: f64| ((limit as f64 * w * 1.5).ceil() as usize).max(1);
    let lib_target = target_per_source(lib_w);
    let lfm_target = target_per_source(lfm_w);
    let eng_target = target_per_source(eng_w);

    // ── Library source ────────────────────────────────────────────────────────
    let library_results: Vec<RadioCandidate> = {
        let mut excl: Vec<i64> = exclude_set.iter().copied().collect();
        excl.push(seed_track_id);
        let creativity = match blend {
            RadioBlend::Familiar => 0.15,
            RadioBlend::Mixed => 0.30,
            RadioBlend::Adventurous => 0.50,
        };
        let lib = crate::services::learning::radio_from_neighbors(db, seed_track_id, &excl, lib_target as i64, creativity);
        if let Err(ref e) = lib {
            tracing::warn!(seed_track_id, error = %e, "orchestrate_song: library/embedding source errored");
        }
        lib.ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let reason = if !n.reason_tags.is_empty() {
                    format!("library · {} (sim {:.2})", n.reason_tags[0], n.similarity_score)
                } else {
                    format!("library · embedding similarity {:.2}", n.similarity_score)
                };
                RadioCandidate {
                    track_id: n.track_id,
                    tidal_track_id: None,
                    title: n.title,
                    artist_name: n.artist_name.unwrap_or_default(),
                    album_title: n.album_title,
                    artwork_url: n.artwork_url,
                    duration_ms: n.duration_ms,
                    isrc: None,
                    is_in_library: true,
                    source: RadioSource::Library,
                    reason,
                    similarity_score: n.similarity_score,
                    confidence: Some(n.confidence),
                    candidate_in_degree_percentile: Some(n.candidate_in_degree_percentile),
                    support_count: Some(n.support_count),
                    primary_reason: n.primary_reason,
                }
            })
            .collect()
    };

    // ── Last.fm source ────────────────────────────────────────────────────────
    if lastfm.is_none() {
        tracing::info!(seed_track_id, "orchestrate_song: no Last.fm client (no API key configured)");
    } else if seed_meta.artist_name.is_none() {
        tracing::info!(seed_track_id, "orchestrate_song: seed has no artist_name; Last.fm source skipped");
    }
    let lastfm_results: Vec<RadioCandidate> =
        if let (Some(client), Some(artist)) = (lastfm, seed_meta.artist_name.as_deref()) {
            let lfm = client
                .track_get_similar_with_artist_fallback(artist, &seed_meta.title, lfm_target.max(20))
                .await;
            if let Err(ref e) = lfm {
                tracing::warn!(seed_track_id, artist, title = %seed_meta.title, error = %e, "orchestrate_song: Last.fm track_get_similar failed");
            }
            lfm.unwrap_or_default()
                .into_iter()
                .take(lfm_target * 2)
                .map(|hit| RadioCandidate {
                    track_id: 0,
                    tidal_track_id: None,
                    title: hit.title,
                    artist_name: hit.artist,
                    album_title: None,
                    artwork_url: None,
                    duration_ms: None,
                    isrc: None,
                    is_in_library: false,
                    source: RadioSource::Lastfm,
                    reason: format!("Last.fm match {:.2}", hit.match_score),
                    similarity_score: hit.match_score.clamp(0.0, 1.0),
                    confidence: None,
                    candidate_in_degree_percentile: None,
                    support_count: None,
                    primary_reason: None,
                })
                .collect()
        } else {
            Vec::new()
        };

    // ── Engine source ─────────────────────────────────────────────────────────
    // Pre-computed track_similarity table (co-album / co-artist /
    // co-listen / genre-proximity / duration / era). Library-only,
    // independent recall path from the embedding model that
    // `radio_from_neighbors` uses. Excludes seed + caller's exclude
    // list so we don't surface tracks the user already has queued.
    let engine_results: Vec<RadioCandidate> = {
        let mut excl: Vec<i64> = exclude_set.iter().copied().collect();
        excl.push(seed_track_id);
        engine_results_from_track_similarity(db, seed_track_id, eng_target, &excl)
            .unwrap_or_default()
    };

    let lib_count = library_results.len();
    let lfm_count = lastfm_results.len();
    let eng_count = engine_results.len();

    // ── Combine + blend ───────────────────────────────────────────────────────
    let mut combined = combine_with_dedup(library_results, lastfm_results, engine_results);
    let combined_count = combined.len();

    // Source-score normalization: only runs when the flag is on. Library cosine
    // scores live in a tighter range than Last.fm match scores, and engine
    // similarity scores have their own scale — direct weighted blending was
    // implicitly favoring whichever source had the widest dynamic range.
    // Normalization is a hybrid of percentile-clipping and rank-norm so neither
    // outliers nor near-degenerate distributions dominate.
    if flags.score_normalization_enabled {
        normalize_source_scores(&mut combined);
    }

    // Candidate-quality penalties (confidence + hub). These shape the score
    // *before* taste/genre/affinity signals fire, treating "is this edge well-
    // supported by the data?" and "is this candidate a hub appearing for every
    // seed?" as more fundamental questions than "does the user prefer this
    // artist?". Both are soft penalties — score multipliers, never hard drops —
    // because the underlying confidence formula is heuristic and a hard floor
    // could quietly remove cold-start library tracks.
    let profile_for_penalties = crate::services::radio_config::RadioProfile::from_blend(blend);
    let mut hub_penalty_total = 0.0_f64;
    if flags.confidence_penalty_enabled {
        apply_confidence_penalty(&mut combined, profile_for_penalties.min_confidence);
    }
    if flags.hub_penalty_enabled {
        hub_penalty_total = apply_hub_penalty(&mut combined, profile_for_penalties.hub_penalty);
    }

    // Snapshot pre-affinity scores so the reason-string suffix can
    // record the affinity multiplier per candidate. Keyed by
    // (source, track_id, normalised dedup key) — same shape the
    // diagnostic harness uses.
    let pre_affinity_scores: HashMap<(RadioSource, i64, String), f64> = combined
        .iter()
        .map(|c| {
            (
                (
                    c.source,
                    c.track_id,
                    normalize_for_dedup(&c.artist_name, &c.title),
                ),
                c.similarity_score,
            )
        })
        .collect();

    // Genre enrichment: load genre paths for every candidate with a
    // real track_id (lastfm hits with track_id=0 are skipped — they
    // have no library row to look up). Build weighted genre sets and
    // a per-candidate Jaccard against the seed. Map keys are stable
    // across both apply_taste_signals and apply_genre_signals
    // candidate drops.
    let jaccard_by_key = compute_genre_jaccard(db, seed_track_id, &combined);

    apply_taste_signals(&mut combined, &taste, &resolver);
    let post_taste_count = combined.len();

    // Snapshot post-affinity / pre-genre scores so the reason suffix
    // can attribute the affinity contribution and the genre
    // contribution to separate fields.
    let post_affinity_scores: HashMap<(RadioSource, i64, String), f64> = combined
        .iter()
        .map(|c| {
            (
                (
                    c.source,
                    c.track_id,
                    normalize_for_dedup(&c.artist_name, &c.title),
                ),
                c.similarity_score,
            )
        })
        .collect();

    // Phase 2b Stage 2: genre coherence scoring + mode-based hard
    // reject. Lastfm candidates pass through untouched (no genre data
    // for tracks outside the library).
    apply_genre_signals(&mut combined, &jaccard_by_key, blend);
    let post_genre_count = combined.len();

    // Reason-string enrichment: append a JSON suffix carrying the
    // structured breakdown that the frontend tooltip parses. Best
    // effort — failure here just keeps the prefix.
    annotate_reasons(
        &mut combined,
        &pre_affinity_scores,
        &post_affinity_scores,
        &jaccard_by_key,
    );

    // Final selection: either the constraint-based diversity re-ranker (when
    // the flag is on) or the legacy weighted-interleave path. Both consume
    // the same `combined` candidate list; only the slot-fill logic differs.
    let mut rerank_counters = RerankCounters::default();
    let ordered = if flags.diversity_rerank_enabled {
        let primary_genres = primary_genres_for_candidates(db, &combined);
        diversity_rerank(
            combined,
            &profile_for_penalties,
            blend,
            limit,
            &primary_genres,
            &taste.recent_track_ids,
            flags.source_quota_bonus_enabled,
            &mut rerank_counters,
        )
    } else {
        blend_interleave(combined, blend, limit)
    };

    tracing::info!(
        seed_track_id,
        blend = ?blend,
        lib_count,
        lfm_count,
        eng_count,
        combined_count,
        post_taste_count,
        post_genre_count,
        final_count = ordered.len(),
        "orchestrate_song: candidate funnel"
    );

    // Diagnostics: record what the new pipeline produced. Skipped when
    // use_legacy_pipeline is true so legacy bypass leaves no trace, matching
    // the plan's "kill-switch produces no row" contract. avg_confidence and
    // avg_candidate_in_degree_pct sample only library candidates that carry
    // those fields; non-library lanes contribute nothing to the average.
    if !flags.use_legacy_pipeline {
        let profile = crate::services::radio_config::RadioProfile::from_blend(blend);
        let mut diag = crate::services::radio_config::RadioDiagnosticsRow {
            seed_track_id: Some(seed_track_id),
            profile_name: profile.name().to_string(),
            creativity: profile.creativity,
            queue_size: ordered.len() as i64,
            target_library_weight: lib_w,
            target_lastfm_weight: lfm_w,
            target_engine_weight: eng_w,
            hub_penalty_total,
            same_artist_penalties: rerank_counters.same_artist_penalties,
            same_album_penalties: rerank_counters.same_album_penalties,
            genre_saturation_penalties: rerank_counters.genre_saturation_penalties,
            repetition_skips: rerank_counters.repetition_skips,
            penalty_relaxations: rerank_counters.penalty_relaxations,
            flags,
            ..Default::default()
        };
        let mut conf_sum = 0.0;
        let mut conf_n = 0;
        let mut hub_sum = 0.0;
        let mut hub_n = 0;
        for cand in &ordered {
            diag.count_source(cand.source);
            if let Some(c) = cand.confidence {
                conf_sum += c;
                conf_n += 1;
            }
            if let Some(h) = cand.candidate_in_degree_percentile {
                hub_sum += h;
                hub_n += 1;
            }
        }
        diag.avg_confidence = if conf_n > 0 {
            Some(conf_sum / conf_n as f64)
        } else {
            None
        };
        diag.avg_candidate_in_degree_pct = if hub_n > 0 {
            Some(hub_sum / hub_n as f64)
        } else {
            None
        };

        if let Err(err) = db.with_conn(|conn| {
            crate::services::radio_config::log_radio_diagnostics(conn, &diag)
        }) {
            // Diagnostics failures should never break a radio request — log and move on.
            tracing::warn!(seed_track_id, error = %err, "failed to log radio diagnostics");
        }
    }

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: seed_for_session,
        tracks: ordered,
    })
}

/// Build a Song Radio queue from an album (multi-seed using album tracks).
pub async fn orchestrate_album(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_album_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    let album_id = seed_album_id;
    let (seed_track_ids, album_title, album_artist) = db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT t.id FROM tracks t WHERE t.album_id = ?1 ORDER BY t.disc_number ASC, t.track_number ASC LIMIT 3",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(rusqlite::params![album_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let meta = conn
            .query_row(
                "SELECT al.title, ar.name FROM albums al LEFT JOIN artists ar ON al.artist_id = ar.id WHERE al.id = ?1",
                rusqlite::params![album_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .ok();
        let title = meta.as_ref().map(|m| m.0.clone());
        let artist = meta.and_then(|m| m.1);
        Ok((ids, title, artist))
    })?;

    if seed_track_ids.is_empty() {
        anyhow::bail!("album has no tracks: {seed_album_id}");
    }

    let per_seed_limit = (limit / seed_track_ids.len()).max(8);
    let mut all_candidates: Vec<RadioCandidate> = Vec::new();
    for tid in &seed_track_ids {
        if let Ok(q) = orchestrate_song(db, lastfm, *tid, blend, per_seed_limit, exclude_track_ids).await {
            all_candidates.extend(q.tracks);
        }
    }
    let combined = combine_with_dedup(all_candidates, Vec::new(), Vec::new());
    let ordered = blend_interleave(combined, blend, limit);

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: RadioSeed {
            kind: "album",
            track_id: None,
            album_id: Some(seed_album_id),
            artist_id: None,
            title: album_title.unwrap_or_else(|| format!("album {seed_album_id}")),
            artist_name: album_artist,
        },
        tracks: ordered,
    })
}

/// Build a Song Radio queue from an artist (multi-seed using artist's top library tracks).
pub async fn orchestrate_artist(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    seed_artist_id: i64,
    blend: RadioBlend,
    limit: usize,
    exclude_track_ids: &[i64],
) -> Result<RadioQueue> {
    let artist_id = seed_artist_id;
    let (seed_track_ids, artist_name) = db.with_conn(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id FROM tracks WHERE artist_id = ?1 ORDER BY play_count DESC, last_played_at DESC LIMIT 3",
        )?;
        let ids: Vec<i64> = stmt
            .query_map(rusqlite::params![artist_id], |row| row.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let name: Option<String> = conn
            .query_row("SELECT name FROM artists WHERE id = ?1", rusqlite::params![artist_id], |row| row.get(0))
            .ok();
        Ok((ids, name))
    })?;

    if seed_track_ids.is_empty() {
        anyhow::bail!("artist has no library tracks: {seed_artist_id}");
    }

    let per_seed_limit = (limit / seed_track_ids.len()).max(8);
    let mut all_candidates: Vec<RadioCandidate> = Vec::new();
    for tid in &seed_track_ids {
        if let Ok(q) = orchestrate_song(db, lastfm, *tid, blend, per_seed_limit, exclude_track_ids).await {
            all_candidates.extend(q.tracks);
        }
    }
    let combined = combine_with_dedup(all_candidates, Vec::new(), Vec::new());
    let ordered = blend_interleave(combined, blend, limit);

    Ok(RadioQueue {
        session_id: new_session_id(),
        blend_used: blend,
        seed: RadioSeed {
            kind: "artist",
            track_id: None,
            album_id: None,
            artist_id: Some(seed_artist_id),
            title: artist_name.clone().unwrap_or_else(|| format!("artist {seed_artist_id}")),
            artist_name,
        },
        tracks: ordered,
    })
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Build per-request taste signals + artist resolver.
///
/// Loads the seed track, builds a `SessionTasteProfile` against the live
/// listen history, converts via `from_session_profile`, and loads an
/// `ArtistResolver` for cross-source artist_id lookups. All three steps
/// share a single connection so the radio request pays for one open, not
/// three.
///
/// On any DB error the function logs a warning and returns empty
/// defaults so taste-aware adjustments become a no-op rather than
/// failing the whole radio request. A skipped artist or a stale seed
/// track id should not take the user's radio offline.
fn build_taste_inputs(db: &Database, seed_track_id: i64) -> (TasteVector, ArtistResolver) {
    let result = db.with_conn(move |conn| -> Result<(TasteVector, ArtistResolver)> {
        let seed_track = crate::playback::queue::get_track_by_id(conn, seed_track_id)?
            .ok_or_else(|| anyhow::anyhow!("seed track not found: {seed_track_id}"))?;
        let profile =
            crate::playback::player::build_session_taste_profile(conn, &seed_track)?;
        let resolver = ArtistResolver::load(conn)?;
        let (taste, _seed_ctx) =
            crate::smart::taste_vector::adapters::from_session_profile(&profile);
        Ok((taste, resolver))
    });

    match result {
        Ok(pair) => pair,
        Err(err) => {
            tracing::warn!(
                seed_track_id,
                "radio: failed to build taste inputs ({err:#}); falling back to no-op taste"
            );
            (TasteVector::default(), ArtistResolver::default())
        }
    }
}

/// Pull engine-source candidates from the precomputed `track_similarity`
/// table. Independent recall path from `radio_from_neighbors` (which uses
/// the embedding model) — both can return library tracks but they score
/// proximity differently, so the union is meaningfully wider than either
/// alone.
///
/// Hands back at most `target` candidates, sorted by `similarity_score`
/// desc. Errors are logged and swallowed: an engine miss should not take
/// the radio request offline, the other two sources can carry it.
///
/// The result `track_id` is a library id, so `is_in_library = true` and
/// hard-suppression in `apply_taste_signals` works against it. The
/// `reason` field carries the component breakdown so the
/// "Why is this here?" hover-card can show co-album / co-artist /
/// co-listen / genre-proximity scores.
fn engine_results_from_track_similarity(
    db: &Database,
    seed_track_id: i64,
    target: usize,
    exclude_ids: &[i64],
) -> Result<Vec<RadioCandidate>> {
    if target == 0 {
        return Ok(Vec::new());
    }
    // Fetch up to 2x target so the dedup downstream has room to drop
    // duplicates without starving the engine slot.
    let fetch_limit = (target * 2).max(8) as i64;
    let exclude_owned: Vec<i64> = exclude_ids.to_vec();
    let rows = db.with_conn(move |conn| {
        crate::db::queries::get_similar_tracks(conn, seed_track_id, fetch_limit, &exclude_owned)
    });

    let rows = match rows {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                seed_track_id,
                "radio: engine source (track_similarity) lookup failed ({err:#}); returning empty"
            );
            return Ok(Vec::new());
        }
    };

    let candidates: Vec<RadioCandidate> = rows
        .into_iter()
        .map(|ts| RadioCandidate {
            track_id: ts.track_id,
            tidal_track_id: None,
            title: ts.title,
            artist_name: ts.artist_name.unwrap_or_default(),
            album_title: ts.album_title,
            artwork_url: ts.artwork_url,
            duration_ms: ts.duration_ms,
            isrc: None,
            is_in_library: true,
            source: RadioSource::Engine,
            reason: format!(
                "library similarity {:.2} (co-album {:.2}, co-artist {:.2}, co-listen {:.2}, genre {:.2})",
                ts.similarity_score,
                ts.co_album_score,
                ts.co_artist_score,
                ts.co_listen_score,
                ts.genre_proximity
            ),
            similarity_score: ts.similarity_score,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        })
        .take(target)
        .collect();

    Ok(candidates)
}

/// Generate a session id like "rad_2a4f...".
pub(crate) fn new_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("rad_{:x}", nanos)
}

/// Normalize an artist+title pair for fuzzy dedup: lowercase, alphanumerics only.
pub(crate) fn normalize_for_dedup(artist: &str, title: &str) -> String {
    let mut s = String::with_capacity(artist.len() + title.len() + 1);
    for ch in artist
        .chars()
        .chain(std::iter::once(' '))
        .chain(title.chars())
    {
        if ch.is_alphanumeric() {
            for c in ch.to_lowercase() {
                s.push(c);
            }
        }
    }
    s
}

/// Group candidates by normalised (artist, title) and pick one winner per
/// group.
///
/// The historical behaviour was "library wins all ties because it iterates
/// first", which silently cannibalised Last.fm's exploration value when the
/// library was dense around a seed. The new rule:
///
/// - If a library candidate is in the group AND its `similarity_score` is
///   within 5% of the best non-library score, library wins. Preserves the
///   "prefer in-library, all else equal" instinct without letting it
///   dominate when an external source is meaningfully more confident.
/// - Otherwise the highest `similarity_score` wins.
/// - Ties (within 1e-9 after the 5% rule) break by source priority
///   Library > Engine > Lastfm so HashMap iteration order doesn't flap
///   between runs.
///
/// Order across groups follows first-seen insertion order across the
/// library/lastfm/engine input slices (in that order). Each input is
/// expected to be roughly score-sorted by its producer, so first-seen
/// approximates a stable, score-ordered output across the deduped set.
fn combine_with_dedup(
    library: Vec<RadioCandidate>,
    lastfm: Vec<RadioCandidate>,
    engine: Vec<RadioCandidate>,
) -> Vec<RadioCandidate> {
    let mut groups: HashMap<String, Vec<RadioCandidate>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for source_list in [library, lastfm, engine] {
        for cand in source_list {
            let norm = normalize_for_dedup(&cand.artist_name, &cand.title);
            if norm.is_empty() {
                continue;
            }
            if !groups.contains_key(&norm) {
                order.push(norm.clone());
            }
            groups.entry(norm).or_default().push(cand);
        }
    }

    let mut out = Vec::with_capacity(order.len());
    for norm in order {
        if let Some(winner) = pick_dedup_winner(groups.remove(&norm).unwrap_or_default()) {
            out.push(winner);
        }
    }
    out
}

// Per-source hybrid normalization. The two halves rein in different failure
// modes: percentile-clipping (p10-p90) drops outliers without compressing the
// bulk; rank-norm guarantees a fair mapping when the score distribution is
// degenerate (all scores nearly equal, e.g. a Last.fm result with everything
// at match=1.0). Half-and-half so neither dominates.
//
// Skipped when N < 5 because both halves get noisy: rank-norm of 4 elements
// produces only 4 distinct values, and p10/p90 quantiles aren't meaningful.
// In that regime, the legacy raw-score behavior is preferable.
fn normalize_source_scores(candidates: &mut [RadioCandidate]) {
    use std::collections::HashMap;
    // Bucket candidates by source. We'll compute the normalization parameters
    // per source then walk the candidates once more applying them.
    let mut by_source: HashMap<RadioSource, Vec<usize>> = HashMap::new();
    for (idx, c) in candidates.iter().enumerate() {
        by_source.entry(c.source).or_default().push(idx);
    }

    for (_source, indices) in by_source.iter() {
        let n = indices.len();
        if n < 5 {
            continue;
        }
        if n == 1 {
            candidates[indices[0]].similarity_score = 1.0;
            continue;
        }
        // Sort indices descending by raw score. Position-in-sorted-order maps
        // to rank: best-ranked = 0 → rank_norm 1.0, worst-ranked = N-1 → 0.0.
        let mut sorted = indices.clone();
        sorted.sort_by(|&a, &b| {
            candidates[b]
                .similarity_score
                .partial_cmp(&candidates[a].similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // p10 and p90 from the same set of raw scores (in any order).
        let mut raw_scores: Vec<f64> =
            indices.iter().map(|&i| candidates[i].similarity_score).collect();
        raw_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p10 = percentile(&raw_scores, 0.10);
        let p90 = percentile(&raw_scores, 0.90);
        let pct_range = p90 - p10;
        let degenerate = pct_range < 1e-6;

        let n_minus_1 = (n as f64) - 1.0;
        for (sorted_idx, &cand_idx) in sorted.iter().enumerate() {
            let rank_norm = 1.0 - (sorted_idx as f64) / n_minus_1;
            let raw = candidates[cand_idx].similarity_score;
            let pct_clipped = if degenerate {
                0.5
            } else {
                ((raw - p10) / pct_range).clamp(0.0, 1.0)
            };
            candidates[cand_idx].similarity_score = 0.5 * pct_clipped + 0.5 * rank_norm;
        }
    }
}

// Soft confidence penalty: library candidates with confidence below the
// threshold get a 0.75 score multiplier. Soft, not a hard drop, because the
// confidence formula is heuristic and we don't want to silently lose cold-
// start tracks (which floor at 0.25). Lastfm + engine candidates pass through
// unchanged — they have no confidence value, and a None should not behave the
// same as "0.0 confidence".
fn apply_confidence_penalty(candidates: &mut [RadioCandidate], min_confidence: f64) {
    const PENALTY_MULTIPLIER: f64 = 0.75;
    for cand in candidates.iter_mut() {
        if let Some(conf) = cand.confidence {
            if conf < min_confidence {
                cand.similarity_score *= PENALTY_MULTIPLIER;
            }
        }
    }
}

// Hub penalty: tracks that appear as a neighbor for many seeds (high in-degree
// percentile) get downweighted. Library candidates only — Lastfm + engine have
// no in-degree data. Returns the cumulative penalty magnitude (sum of (1 -
// multiplier) over all penalized candidates) for diagnostics.
//
// The 1/(1 + k*pct) shape gives a smooth slope: pct=0 → multiplier 1.0 (no
// penalty), pct=1 → 1/(1+k). With hub_penalty=0.35 (Mixed default), top-hub
// gets 0.74×, mid-hub (pct=0.5) gets 0.85×.
fn apply_hub_penalty(candidates: &mut [RadioCandidate], hub_penalty: f64) -> f64 {
    if hub_penalty <= 0.0 {
        return 0.0;
    }
    let mut total = 0.0;
    for cand in candidates.iter_mut() {
        if let Some(pct) = cand.candidate_in_degree_percentile {
            let multiplier = 1.0 / (1.0 + hub_penalty * pct);
            total += 1.0 - multiplier;
            cand.similarity_score *= multiplier;
        }
    }
    total
}

// Linear-interpolated percentile of an already-ascending-sorted slice. q is in
// [0, 1]. Avoids pulling in a full statistics dep just for two values.
fn percentile(sorted_asc: &[f64], q: f64) -> f64 {
    if sorted_asc.is_empty() {
        return 0.0;
    }
    if sorted_asc.len() == 1 {
        return sorted_asc[0];
    }
    let q = q.clamp(0.0, 1.0);
    let pos = q * (sorted_asc.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted_asc[lo]
    } else {
        let frac = pos - lo as f64;
        sorted_asc[lo] * (1.0 - frac) + sorted_asc[hi] * frac
    }
}

/// Returns the higher-priority numeric for tie-breaking. Order is
/// Library, then Engine, then Lastfm — matching the implicit preference
/// from the legacy behaviour (library first), narrowed to only fire on
/// score ties.
fn source_priority(source: RadioSource) -> u8 {
    match source {
        RadioSource::Library => 3,
        RadioSource::Engine => 2,
        RadioSource::Lastfm => 1,
    }
}

const LIBRARY_TIE_BREAK_THRESHOLD: f64 = 0.95;

fn pick_dedup_winner(group: Vec<RadioCandidate>) -> Option<RadioCandidate> {
    if group.is_empty() {
        return None;
    }

    let best_library_score = group
        .iter()
        .filter(|c| c.source == RadioSource::Library)
        .map(|c| c.similarity_score)
        .fold(f64::NEG_INFINITY, f64::max);
    let best_other_score = group
        .iter()
        .filter(|c| c.source != RadioSource::Library)
        .map(|c| c.similarity_score)
        .fold(f64::NEG_INFINITY, f64::max);

    let library_present = best_library_score.is_finite();
    let other_threshold = if best_other_score.is_finite() {
        best_other_score * LIBRARY_TIE_BREAK_THRESHOLD
    } else {
        f64::NEG_INFINITY
    };

    if library_present && best_library_score >= other_threshold {
        // Library wins: pick the highest-scoring library candidate.
        return group
            .into_iter()
            .filter(|c| c.source == RadioSource::Library)
            .max_by(|a, b| {
                a.similarity_score
                    .partial_cmp(&b.similarity_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
    }

    // Highest-score wins. Tie-break by source priority so HashMap
    // iteration order doesn't make the output flap.
    group.into_iter().max_by(|a, b| {
        a.similarity_score
            .partial_cmp(&b.similarity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| source_priority(a.source).cmp(&source_priority(b.source)))
    })
}

/// Saturation constant for affinity compression. At `x = K` the
/// compressed value is 0.5; at `x = 4K` it's 0.8; asymptote is 1.0.
/// `pos` and `neg` are unbounded recency-weighted accumulators that
/// easily reach 20–50 for any artist with recent listen history, so the
/// raw values cannot drive the multiplier directly without saturating.
const AFFINITY_SATURATION: f64 = 10.0;

/// Bounded effect a saturated positive affinity adds to the multiplier.
/// At `pos = ∞` the multiplier is `1.0 + SCALE_POS = 1.20`.
const AFFINITY_SCALE_POS: f64 = 0.20;

/// Bounded effect a saturated negative affinity subtracts. At
/// `neg = ∞` the multiplier is `1.0 - SCALE_NEG = 0.70`. Asymmetric vs
/// `SCALE_POS` to mirror automix's "negatives hurt more than positives
/// help" weighting (0.5 / 0.65 there, 0.20 / 0.30 here).
const AFFINITY_SCALE_NEG: f64 = 0.30;

/// Floor on the multiplier so even heavily-skipped artists still appear
/// in the queue at low rank rather than being eliminated entirely. The
/// formula above never produces a value below `1.0 − SCALE_NEG = 0.70`,
/// so this is a defensive backstop, not the load-bearing clamp the
/// previous implementation relied on.
const AFFINITY_FLOOR: f64 = 0.1;

/// Apply per-user taste signals to a deduped candidate list. Drops tracks
/// the user just skipped; nudges `similarity_score` up for liked artists
/// and down for skipped ones.
///
/// Compression first: the raw `pos` and `neg` accumulators get
/// `x / (x + K)` so 20 vs 50 vs 200 all map to roughly the same
/// region of `[0, 1]`. Then asymmetric scaling: positives are worth at
/// most +20% of the score, negatives at most −30%, mirroring automix's
/// pos:neg ratio at a magnitude that suits radio's bounded
/// `similarity_score`.
///
/// Resolver misses (last.fm hit naming an artist not in the library) leave
/// `similarity_score` unchanged. That is the documented Phase 2a
/// behaviour: unknown artists carry no affinity, full stop.
fn apply_taste_signals(
    candidates: &mut Vec<RadioCandidate>,
    taste: &TasteVector,
    resolver: &ArtistResolver,
) {
    // Hard suppression: drop tracks the user just skipped. Only fires
    // for candidates with a real library track_id; last.fm hits have
    // track_id = 0 and skip the check.
    candidates
        .retain(|cand| cand.track_id == 0 || !taste.skipped_track_ids.contains(&cand.track_id));

    for cand in candidates.iter_mut() {
        if let Some(artist_id) = resolver.lookup(&cand.artist_name)
            && let Some(affinity) = taste.artist_affinity.get(&artist_id)
        {
            let pos_c = affinity.pos / (affinity.pos + AFFINITY_SATURATION);
            let neg_c = affinity.neg / (affinity.neg + AFFINITY_SATURATION);
            let multiplier =
                1.0 + (pos_c * AFFINITY_SCALE_POS) - (neg_c * AFFINITY_SCALE_NEG);
            cand.similarity_score *= multiplier.max(AFFINITY_FLOOR);
        }
    }
}

/// Compute a weighted Jaccard genre similarity between the seed and
/// every candidate that has a real library `track_id`.
///
/// Returns a `HashMap` keyed by `(source, track_id, normalised dedup
/// key)` — the same shape `pre_affinity_scores` uses, deliberately
/// stable across `apply_taste_signals`'s `candidates.retain(...)`
/// drops. Lastfm candidates (track_id == 0) and library candidates
/// with no genre rows are absent from the map; callers should treat
/// absence as "no signal" rather than "zero similarity".
///
/// On any DB error or missing seed genres, returns an empty map and
/// logs. Genre signal failure must not take the radio request offline.
fn compute_genre_jaccard(
    db: &Database,
    seed_track_id: i64,
    candidates: &[RadioCandidate],
) -> HashMap<(RadioSource, i64, String), f64> {
    use crate::genre::jaccard::{weighted_genre_set, weighted_jaccard};

    let cand_ids: Vec<i64> = candidates
        .iter()
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    if cand_ids.is_empty() {
        return HashMap::new();
    }

    let mut all_ids = cand_ids.clone();
    all_ids.push(seed_track_id);
    all_ids.sort_unstable();
    all_ids.dedup();

    let paths_by_track = match db.with_conn(move |conn| {
        crate::db::queries::get_genres_for_tracks(conn, &all_ids)
    }) {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(
                seed_track_id,
                "radio: genre enrichment query failed ({err:#}); skipping genre signal"
            );
            return HashMap::new();
        }
    };

    let seed_set = match paths_by_track.get(&seed_track_id) {
        Some(paths) => weighted_genre_set(paths),
        None => {
            tracing::debug!(
                seed_track_id,
                "radio: seed has no genre rows; genre Jaccard skipped for all candidates"
            );
            return HashMap::new();
        }
    };

    let mut out = HashMap::new();
    for cand in candidates.iter() {
        if cand.track_id <= 0 {
            continue;
        }
        let Some(paths) = paths_by_track.get(&cand.track_id) else {
            continue;
        };
        let cand_set = weighted_genre_set(paths);
        let score = weighted_jaccard(&seed_set, &cand_set);
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        out.insert(key, score);
    }
    out
}

/// Phase 2b Stage 2: genre coherence multiplier and mode-based hard
/// reject.
///
/// For each candidate with a Jaccard value (library/engine candidates;
/// lastfm hits are absent from the map and pass through untouched):
///
/// - **Hard reject** if `jaccard < threshold[blend]`. Familiar drops
///   below 0.10, Mixed drops below 0.05, Adventurous never drops.
///   This filters out candidates that share no genre relationship with
///   the seed at all — the kind of false-positive Phase 2b targets.
///
/// - **Multiplier** `1.0 + (jaccard * 0.30) - ((1.0 - jaccard) * 0.20
///   when jaccard < 0.5 else 0)`. Substantial overlap (jaccard >= 0.5)
///   only ever helps; partial overlap can demote. Floored at 0.1
///   defensively (the formula itself never goes below 0.80).
///
/// Lastfm candidates pass through with no adjustment by design — the
/// system has no genre data for tracks not in the library, and a
/// library-artist proxy was rejected as too lossy. They compete on
/// source-native similarity score and artist-affinity multiplier
/// only.
fn apply_genre_signals(
    candidates: &mut Vec<RadioCandidate>,
    jaccard_by_key: &HashMap<(RadioSource, i64, String), f64>,
    blend: RadioBlend,
) {
    let hard_reject_threshold = match blend {
        RadioBlend::Familiar => Some(0.10),
        RadioBlend::Mixed => Some(0.05),
        RadioBlend::Adventurous => None,
    };

    candidates.retain_mut(|cand| {
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        let Some(jaccard) = jaccard_by_key.get(&key).copied() else {
            // No genre data — pass through unchanged.
            return true;
        };

        // Mode-based hard reject.
        if let Some(threshold) = hard_reject_threshold {
            if jaccard < threshold {
                return false;
            }
        }

        // Multiplier per the locked Phase 2b formula.
        let bonus = jaccard * 0.30;
        let penalty = if jaccard < 0.5 {
            (1.0 - jaccard) * 0.20
        } else {
            0.0
        };
        let multiplier = (1.0 + bonus - penalty).max(0.1);
        cand.similarity_score *= multiplier;
        true
    });
}

/// Append a structured JSON suffix to each candidate's `reason` string,
/// carrying the genre Jaccard, the affinity multiplier, and (in Stage
/// 2) the genre multiplier for the frontend tooltip to display.
///
/// Format: `"<existing prefix> | <json>"`. The frontend parser splits
/// on the rightmost ` | ` and tries `JSON.parse` on the right half;
/// candidates without the suffix keep working as plain strings.
///
/// `pre_affinity` is the snapshot taken *before* both `apply_taste_signals`
/// and `apply_genre_signals`; `post_affinity` is the snapshot taken
/// between the two. The current `cand.similarity_score` is the
/// post-genre value. From these three points we extract:
///
/// - `affinity_mult = post_affinity / pre_affinity`
/// - `genre_mult    = post_genre    / post_affinity`
///
/// Best-effort: serialisation failure is silently swallowed so a
/// reason-formatting bug never fails a radio request.
fn annotate_reasons(
    candidates: &mut [RadioCandidate],
    pre_affinity: &HashMap<(RadioSource, i64, String), f64>,
    post_affinity: &HashMap<(RadioSource, i64, String), f64>,
    jaccard_by_key: &HashMap<(RadioSource, i64, String), f64>,
) {
    for cand in candidates.iter_mut() {
        let key = (
            cand.source,
            cand.track_id,
            normalize_for_dedup(&cand.artist_name, &cand.title),
        );
        let pre_aff = pre_affinity.get(&key).copied();
        let post_aff = post_affinity.get(&key).copied();
        let affinity_mult = match (pre_aff, post_aff) {
            (Some(p), Some(a)) if p > 0.0 => Some(a / p),
            _ => None,
        };
        let genre_mult = match post_aff {
            Some(a) if a > 0.0 => Some(cand.similarity_score / a),
            _ => None,
        };
        let jaccard = jaccard_by_key.get(&key).copied();

        let mut parts: Vec<String> = Vec::new();
        if let Some(j) = jaccard {
            parts.push(format!("\"genre_jaccard\":{j:.4}"));
        }
        if let Some(m) = affinity_mult {
            parts.push(format!("\"affinity_mult\":{m:.4}"));
        }
        if let Some(m) = genre_mult {
            parts.push(format!("\"genre_mult\":{m:.4}"));
        }
        if parts.is_empty() {
            continue;
        }
        let suffix = format!("{{{}}}", parts.join(","));
        cand.reason = format!("{} | {}", cand.reason, suffix);
    }
}

fn blend_interleave(candidates: Vec<RadioCandidate>, blend: RadioBlend, limit: usize) -> Vec<RadioCandidate> {
    let (lib_w, lfm_w, eng_w) = blend.weights();
    let mut by_source: std::collections::HashMap<RadioSource, Vec<RadioCandidate>> =
        std::collections::HashMap::new();
    for c in candidates {
        by_source.entry(c.source).or_default().push(c);
    }
    for v in by_source.values_mut() {
        v.sort_by(|a, b| b.similarity_score.partial_cmp(&a.similarity_score).unwrap_or(std::cmp::Ordering::Equal));
    }

    let lib_avail = by_source.get(&RadioSource::Library).map_or(0, |v| v.len());
    let lfm_avail = by_source.get(&RadioSource::Lastfm).map_or(0, |v| v.len());
    let eng_avail = by_source.get(&RadioSource::Engine).map_or(0, |v| v.len());
    let lib_take = ((limit as f64 * lib_w).round() as usize).min(lib_avail);
    let lfm_take = ((limit as f64 * lfm_w).round() as usize).min(lfm_avail);
    let eng_take = ((limit as f64 * eng_w).round() as usize).min(eng_avail);

    let mut lib_iter = by_source.remove(&RadioSource::Library).unwrap_or_default().into_iter().take(lib_take);
    let mut lfm_iter = by_source.remove(&RadioSource::Lastfm).unwrap_or_default().into_iter().take(lfm_take);
    let mut eng_iter = by_source.remove(&RadioSource::Engine).unwrap_or_default().into_iter().take(eng_take);

    let mut out = Vec::with_capacity(limit);
    let mut lib_done = 0usize;
    let mut lfm_done = 0usize;
    let mut eng_done = 0usize;

    while out.len() < limit {
        let lib_behind = (lib_take as f64 - lib_done as f64) / lib_w.max(0.01);
        let lfm_behind = (lfm_take as f64 - lfm_done as f64) / lfm_w.max(0.01);
        let eng_behind = (eng_take as f64 - eng_done as f64) / eng_w.max(0.01);

        let pick = if lib_behind >= lfm_behind && lib_behind >= eng_behind {
            lib_iter.next().map(|c| { lib_done += 1; c })
        } else if lfm_behind >= eng_behind {
            lfm_iter.next().map(|c| { lfm_done += 1; c })
        } else {
            eng_iter.next().map(|c| { eng_done += 1; c })
        };

        match pick {
            Some(c) => out.push(c),
            None => {
                if let Some(c) = lib_iter.next() { lib_done += 1; out.push(c); }
                else if let Some(c) = lfm_iter.next() { lfm_done += 1; out.push(c); }
                else if let Some(c) = eng_iter.next() { eng_done += 1; out.push(c); }
                else { break; }
            }
        }
    }
    out
}

// Counters tallied by diversity_rerank for the radio_diagnostics row. Each
// counts how many *picks* triggered the corresponding penalty, not how many
// candidates were considered — so a value of 3 means 3 of the final queue's
// slots had to push through that penalty to be placed.
#[derive(Debug, Clone, Default)]
struct RerankCounters {
    same_artist_penalties: i64,
    same_album_penalties: i64,
    genre_saturation_penalties: i64,
    repetition_skips: i64,
    penalty_relaxations: i64,
}

// Pulls one primary genre token per candidate track. "Primary" = the root of
// the most-frequently-occurring genre path — `Electronic > House > Deep House`
// becomes "electronic". Lowercased so saturation-counting is case-insensitive.
fn primary_genres_for_candidates(
    db: &Database,
    candidates: &[RadioCandidate],
) -> HashMap<i64, String> {
    let ids: Vec<i64> = candidates
        .iter()
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    let paths_by_track = match db
        .with_conn(move |conn| crate::db::queries::get_genres_for_tracks(conn, &ids))
    {
        Ok(m) => m,
        Err(err) => {
            tracing::debug!(
                "radio: primary-genre lookup failed ({err:#}); diversity rerank will skip genre saturation"
            );
            return HashMap::new();
        }
    };
    let mut out = HashMap::with_capacity(paths_by_track.len());
    for (track_id, paths) in paths_by_track {
        let mut counts: HashMap<String, i32> = HashMap::new();
        for path in &paths {
            let root = path
                .split(" > ")
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !root.is_empty() {
                *counts.entry(root).or_default() += 1;
            }
        }
        if let Some((root, _)) = counts.into_iter().max_by_key(|(_, c)| *c) {
            out.insert(track_id, root);
        }
    }
    out
}

// Tracks which penalties fired during a single (candidate, queue) scoring.
// Used by diversity_rerank to increment the diagnostic counters once per
// final pick (not per scoring-attempt — we don't want relaxation passes to
// double-count).
#[derive(Debug, Default, Clone, Copy)]
struct PenaltyHits {
    artist: bool,
    album: bool,
    genre_saturation: bool,
}

// Score a single candidate against the running queue with optional penalty
// dimensions disabled (for the relaxation pass). Returns (penalized_score,
// hit_flags). Lower is worse; a negative score means every penalty bit landed.
fn score_candidate_for_slot(
    cand: &RadioCandidate,
    queue: &[RadioCandidate],
    profile: &crate::services::radio_config::RadioProfile,
    primary_genres: &HashMap<i64, String>,
    drop_artist: bool,
    drop_album: bool,
    drop_genre: bool,
) -> (f64, PenaltyHits) {
    let mut score = cand.similarity_score;
    let mut hits = PenaltyHits::default();
    let weight = profile.diversity_weight;

    if !drop_artist && profile.same_artist_penalty > 0.0 && weight > 0.0 {
        let lo = queue.len().saturating_sub(5);
        if queue[lo..]
            .iter()
            .any(|q| q.artist_name.eq_ignore_ascii_case(&cand.artist_name))
        {
            score -= profile.same_artist_penalty * weight;
            hits.artist = true;
        }
    }

    if !drop_album && profile.same_album_penalty > 0.0 && weight > 0.0 {
        if let Some(my_album) = cand.album_title.as_deref() {
            let lo = queue.len().saturating_sub(8);
            if queue[lo..].iter().any(|q| {
                q.album_title
                    .as_deref()
                    .map(|a| a.eq_ignore_ascii_case(my_album))
                    .unwrap_or(false)
            }) {
                score -= profile.same_album_penalty * weight;
                hits.album = true;
            }
        }
    }

    if !drop_genre && profile.genre_saturation_penalty > 0.0 && weight > 0.0 {
        if let Some(my_genre) = primary_genres.get(&cand.track_id) {
            let lo = queue.len().saturating_sub(10);
            let count = queue[lo..]
                .iter()
                .filter(|q| {
                    primary_genres
                        .get(&q.track_id)
                        .map(|g| g == my_genre)
                        .unwrap_or(false)
                })
                .count();
            // Threshold: penalty fires only above 3 in the last 10. Saturating
            // sub avoids underflow for counts < 3.
            let excess = count.saturating_sub(3) as f64;
            if excess > 0.0 {
                score -= profile.genre_saturation_penalty * weight * excess;
                hits.genre_saturation = true;
            }
        }
    }

    (score, hits)
}

// Constraint-based greedy slot-fill replacing blend_interleave. At each slot,
// scores every eligible candidate against the queue-so-far, applies penalties,
// optionally biases toward under-quota sources, and picks the argmax. If the
// best score after penalties is non-positive, relaxes one dimension at a time
// (genre → album → artist) until something positive emerges.
//
// Hard skips: tracks in `recent_track_ids` (skipped or recently-played) are
// dropped from the eligible pool entirely. If the pool empties before the
// queue fills, the function returns short — the caller logs `repetition_skips`
// in diagnostics. No fallback to a wider candidate pool here; the orchestrator
// upstream can decide whether that's acceptable.
fn diversity_rerank(
    candidates: Vec<RadioCandidate>,
    profile: &crate::services::radio_config::RadioProfile,
    blend: RadioBlend,
    limit: usize,
    primary_genres: &HashMap<i64, String>,
    recent_track_ids: &HashSet<i64>,
    apply_source_quota: bool,
    counters: &mut RerankCounters,
) -> Vec<RadioCandidate> {
    let target_size = limit.min(candidates.len());
    let mut available = candidates;
    let mut queue: Vec<RadioCandidate> = Vec::with_capacity(target_size);
    let mut source_counts: HashMap<RadioSource, i64> = HashMap::new();
    let (lib_w, lfm_w, eng_w) = blend.weights();
    const SOURCE_QUOTA_BONUS: f64 = 1.05;

    while queue.len() < target_size && !available.is_empty() {
        // Filter to eligible: not in recent_track_ids. Track-id 0 (lastfm rows)
        // pass the filter regardless since they're not in the library set.
        let eligible_indices: Vec<usize> = (0..available.len())
            .filter(|&i| {
                let c = &available[i];
                c.track_id == 0 || !recent_track_ids.contains(&c.track_id)
            })
            .collect();
        if eligible_indices.is_empty() {
            counters.repetition_skips += (target_size - queue.len()) as i64;
            break;
        }

        // Try with full penalties first; relax progressively if every score
        // comes out non-positive.
        let mut chosen: Option<(usize, PenaltyHits)> = None;
        let mut relaxation_used = false;
        let relaxation_steps = [
            (false, false, false), // full penalties
            (false, false, true),  // drop genre_saturation
            (false, true, true),   // drop album too
            (true, true, true),    // drop artist too — no penalties left
        ];

        for (step_idx, &(drop_artist, drop_album, drop_genre)) in
            relaxation_steps.iter().enumerate()
        {
            let mut best: Option<(usize, f64, PenaltyHits)> = None;

            for &idx in &eligible_indices {
                let cand = &available[idx];
                let (mut score, hits) = score_candidate_for_slot(
                    cand,
                    &queue,
                    profile,
                    primary_genres,
                    drop_artist,
                    drop_album,
                    drop_genre,
                );
                if apply_source_quota {
                    let target_for_source = match cand.source {
                        RadioSource::Library => lib_w,
                        RadioSource::Lastfm => lfm_w,
                        RadioSource::Engine => eng_w,
                    } * (queue.len() as f64);
                    let actual = *source_counts.get(&cand.source).unwrap_or(&0) as f64;
                    if actual < target_for_source {
                        score *= SOURCE_QUOTA_BONUS;
                    }
                }
                if best
                    .map(|(_, b_score, _)| score > b_score)
                    .unwrap_or(true)
                {
                    best = Some((idx, score, hits));
                }
            }

            match best {
                Some((idx, score, hits)) if score > 0.0 || step_idx == relaxation_steps.len() - 1 => {
                    chosen = Some((idx, hits));
                    if step_idx > 0 {
                        relaxation_used = true;
                    }
                    break;
                }
                Some(_) => {
                    // Score still <= 0; advance to next relaxation step.
                    continue;
                }
                None => break,
            }
        }

        let Some((pick_idx, hits)) = chosen else {
            counters.repetition_skips += (target_size - queue.len()) as i64;
            break;
        };
        if relaxation_used {
            counters.penalty_relaxations += 1;
        }
        if hits.artist {
            counters.same_artist_penalties += 1;
        }
        if hits.album {
            counters.same_album_penalties += 1;
        }
        if hits.genre_saturation {
            counters.genre_saturation_penalties += 1;
        }
        let cand = available.swap_remove(pick_idx);
        *source_counts.entry(cand.source).or_default() += 1;
        queue.push(cand);
    }

    queue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_sum_to_one() {
        for blend in [RadioBlend::Familiar, RadioBlend::Mixed, RadioBlend::Adventurous] {
            let (a, b, c) = blend.weights();
            assert!(
                (a + b + c - 1.0).abs() < 1e-9,
                "weights for {blend:?}: {a}+{b}+{c}"
            );
        }
    }

    #[test]
    fn dedup_normalizes_punctuation_and_case() {
        let a = normalize_for_dedup("*NSYNC", "Bye Bye Bye");
        let b = normalize_for_dedup("nsync!!!", "byeByeBye");
        assert_eq!(a, b);
    }

    #[test]
    fn dedup_normalizes_unicode_whitespace() {
        let a = normalize_for_dedup("Sigur  Rós", "Hoppípolla");
        let b = normalize_for_dedup("sigurrós", "hoppípolla");
        assert_eq!(a, b);
    }

    fn make_cand(source: RadioSource, idx: i64, score: f64) -> RadioCandidate {
        RadioCandidate {
            track_id: idx,
            tidal_track_id: None,
            title: format!("t{idx}"),
            artist_name: format!("a{idx}"),
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            isrc: None,
            is_in_library: source == RadioSource::Library,
            source,
            reason: String::new(),
            similarity_score: score,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        }
    }

    #[test]
    fn normalize_skips_when_source_has_fewer_than_five() {
        // 3-candidate source: function should leave scores untouched.
        let mut cands: Vec<RadioCandidate> = (0..3)
            .map(|i| make_cand(RadioSource::Library, i, 0.10 + 0.20 * i as f64))
            .collect();
        let originals: Vec<f64> = cands.iter().map(|c| c.similarity_score).collect();
        normalize_source_scores(&mut cands);
        let after: Vec<f64> = cands.iter().map(|c| c.similarity_score).collect();
        assert_eq!(originals, after);
    }

    #[test]
    fn normalize_top_candidate_lands_at_one_after_blend() {
        // Library has 5 distinct, well-spread scores. The top one's
        // rank_norm = 1.0; with non-degenerate p10/p90 spread the
        // top's score sits between (0.5*1.0 + 0.5*1.0) = 1.0.
        let scores = [0.10, 0.30, 0.50, 0.70, 0.90];
        let mut cands: Vec<RadioCandidate> = scores
            .iter()
            .enumerate()
            .map(|(i, &s)| make_cand(RadioSource::Library, i as i64, s))
            .collect();
        normalize_source_scores(&mut cands);
        let top = cands.iter().fold(0.0_f64, |max, c| max.max(c.similarity_score));
        assert!((top - 1.0).abs() < 1e-6, "top candidate normalized to {}", top);
        let bottom = cands
            .iter()
            .fold(f64::INFINITY, |min, c| min.min(c.similarity_score));
        assert!(bottom < 0.05, "bottom candidate normalized to {}", bottom);
    }

    #[test]
    fn normalize_handles_degenerate_spread_via_neutral_pct() {
        // All five Last.fm scores equal — p90 == p10, hits the guard.
        // Resulting normalized scores should all be 0.5*0.5 + 0.5*rank_norm,
        // which still produces distinct values from the rank component.
        let mut cands: Vec<RadioCandidate> = (0..5)
            .map(|i| make_cand(RadioSource::Lastfm, i, 1.0))
            .collect();
        normalize_source_scores(&mut cands);
        // Top = 0.5*0.5 + 0.5*1.0 = 0.75. Bottom = 0.5*0.5 + 0.5*0.0 = 0.25.
        let top = cands
            .iter()
            .fold(0.0_f64, |max, c| max.max(c.similarity_score));
        let bottom = cands
            .iter()
            .fold(f64::INFINITY, |min, c| min.min(c.similarity_score));
        assert!((top - 0.75).abs() < 1e-6, "top = {}", top);
        assert!((bottom - 0.25).abs() < 1e-6, "bottom = {}", bottom);
    }

    #[test]
    fn confidence_penalty_only_hits_library_below_threshold() {
        let mut cands = vec![
            // Below threshold — gets penalty
            {
                let mut c = make_cand(RadioSource::Library, 1, 1.0);
                c.confidence = Some(0.30);
                c
            },
            // At threshold — passes through (strict <, not <=)
            {
                let mut c = make_cand(RadioSource::Library, 2, 1.0);
                c.confidence = Some(0.40);
                c
            },
            // No confidence (lastfm) — passes through regardless
            make_cand(RadioSource::Lastfm, 3, 1.0),
        ];
        apply_confidence_penalty(&mut cands, 0.40);
        assert!((cands[0].similarity_score - 0.75).abs() < 1e-9);
        assert!((cands[1].similarity_score - 1.0).abs() < 1e-9);
        assert!((cands[2].similarity_score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn hub_penalty_scales_with_percentile() {
        let mut cands = vec![
            // pct=0 → multiplier=1.0 (no penalty)
            {
                let mut c = make_cand(RadioSource::Library, 1, 1.0);
                c.candidate_in_degree_percentile = Some(0.0);
                c
            },
            // pct=1.0 → multiplier=1/(1+0.5) = 0.667
            {
                let mut c = make_cand(RadioSource::Library, 2, 1.0);
                c.candidate_in_degree_percentile = Some(1.0);
                c
            },
            // No percentile data → passes through
            make_cand(RadioSource::Lastfm, 3, 1.0),
        ];
        let total = apply_hub_penalty(&mut cands, 0.5);
        assert!((cands[0].similarity_score - 1.0).abs() < 1e-9);
        let expected_top = 1.0 / 1.5;
        assert!((cands[1].similarity_score - expected_top).abs() < 1e-9);
        assert!((cands[2].similarity_score - 1.0).abs() < 1e-9);
        let expected_total = (1.0 - 1.0) + (1.0 - expected_top);
        assert!((total - expected_total).abs() < 1e-9);
    }

    #[test]
    fn hub_penalty_zero_is_noop() {
        let mut cands = vec![{
            let mut c = make_cand(RadioSource::Library, 1, 0.7);
            c.candidate_in_degree_percentile = Some(0.9);
            c
        }];
        let total = apply_hub_penalty(&mut cands, 0.0);
        assert!((cands[0].similarity_score - 0.7).abs() < 1e-9);
        assert_eq!(total, 0.0);
    }

    fn make_cand_full(
        source: RadioSource,
        track_id: i64,
        artist: &str,
        title: &str,
        album: Option<&str>,
        score: f64,
    ) -> RadioCandidate {
        RadioCandidate {
            track_id,
            tidal_track_id: None,
            title: title.to_string(),
            artist_name: artist.to_string(),
            album_title: album.map(|s| s.to_string()),
            artwork_url: None,
            duration_ms: None,
            isrc: None,
            is_in_library: source == RadioSource::Library,
            source,
            reason: String::new(),
            similarity_score: score,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        }
    }

    #[test]
    fn diversity_rerank_spaces_same_artist() {
        // Five candidates by "A" with high scores, then one each by B and C
        // with lower scores. With same_artist_penalty active, the rerank
        // should not place A back-to-back even though A has the best raw scores.
        let cands = vec![
            make_cand_full(RadioSource::Library, 1, "A", "t1", None, 0.95),
            make_cand_full(RadioSource::Library, 2, "A", "t2", None, 0.94),
            make_cand_full(RadioSource::Library, 3, "B", "t3", None, 0.50),
            make_cand_full(RadioSource::Library, 4, "C", "t4", None, 0.40),
        ];
        let profile = crate::services::radio_config::RadioProfile {
            same_artist_penalty: 0.5,
            same_album_penalty: 0.0,
            genre_saturation_penalty: 0.0,
            diversity_weight: 1.0,
            ..crate::services::radio_config::RadioProfile::mixed()
        };
        let mut counters = RerankCounters::default();
        let queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Mixed,
            4,
            &HashMap::new(),
            &HashSet::new(),
            false,
            &mut counters,
        );
        // First slot is a top-A. Second slot should be B (score 0.50) instead
        // of A2 (0.94 - 0.5 = 0.44).
        assert_eq!(queue[0].artist_name, "A");
        assert_eq!(queue[1].artist_name, "B", "second slot should not be same artist");
    }

    #[test]
    fn diversity_rerank_hard_skips_recent_tracks() {
        let cands = vec![
            make_cand_full(RadioSource::Library, 1, "A", "t1", None, 0.95),
            make_cand_full(RadioSource::Library, 2, "B", "t2", None, 0.40),
        ];
        let mut recent = HashSet::new();
        recent.insert(1);
        let profile = crate::services::radio_config::RadioProfile::mixed();
        let mut counters = RerankCounters::default();
        let queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Mixed,
            2,
            &HashMap::new(),
            &recent,
            false,
            &mut counters,
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].track_id, 2);
        assert_eq!(counters.repetition_skips, 1);
    }

    #[test]
    fn diversity_rerank_relaxes_when_all_scores_negative() {
        // Only one candidate, an artist match against the queue, with a
        // crushing penalty. Without relaxation it'd score below zero and never
        // be picked. With relaxation, the penalty drops and it gets picked.
        let mut counters = RerankCounters::default();
        let queue_already = vec![make_cand_full(
            RadioSource::Library,
            10,
            "Solo",
            "first",
            None,
            0.5,
        )];
        let mut cands = vec![make_cand_full(
            RadioSource::Library,
            11,
            "Solo",
            "second",
            None,
            0.05,
        )];
        let profile = crate::services::radio_config::RadioProfile {
            same_artist_penalty: 0.5,
            diversity_weight: 1.0,
            ..crate::services::radio_config::RadioProfile::mixed()
        };
        // Manually invoke the inner machinery: pre-populate queue, then call
        // the rerank with a one-candidate pool. (Easier than coaxing the full
        // function into emitting the same setup.)
        cands.extend(queue_already.clone().into_iter());
        let queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Mixed,
            2,
            &HashMap::new(),
            &HashSet::new(),
            false,
            &mut counters,
        );
        // Both got placed; the second slot relaxed past the artist penalty.
        assert_eq!(queue.len(), 2);
        assert!(counters.penalty_relaxations >= 1);
    }

    #[test]
    fn diversity_rerank_genre_saturation_steers_toward_jazz_when_electronic_floods() {
        // 5 electronic candidates + 1 jazz. With genre threshold of 3 in last
        // 10, the first 4 electronic slots fill freely (the 4th is the slot
        // where excess just becomes 1). When the 5th slot evaluates, the
        // remaining electronic would score below jazz, so jazz gets picked.
        let cands: Vec<RadioCandidate> = (1..=6)
            .map(|i| {
                make_cand_full(
                    RadioSource::Library,
                    i,
                    &format!("artist{i}"),
                    &format!("t{i}"),
                    None,
                    0.5,
                )
            })
            .collect();
        let mut primary_genres = HashMap::new();
        for i in 1..=5 {
            primary_genres.insert(i, "electronic".to_string());
        }
        primary_genres.insert(6, "jazz".to_string());
        let profile = crate::services::radio_config::RadioProfile {
            genre_saturation_penalty: 1.0,
            same_artist_penalty: 0.0,
            same_album_penalty: 0.0,
            diversity_weight: 1.0,
            ..crate::services::radio_config::RadioProfile::mixed()
        };
        let mut counters = RerankCounters::default();
        let queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Mixed,
            5,
            &primary_genres,
            &HashSet::new(),
            false,
            &mut counters,
        );
        let jazz_index = queue.iter().position(|c| c.track_id == 6);
        assert!(
            jazz_index.is_some(),
            "jazz candidate should be selected when electronic saturates"
        );
    }

    #[test]
    fn diversity_rerank_counter_fires_when_penalty_applies_to_chosen() {
        // No alternative genre — every candidate is electronic. Once the queue
        // has 4+ electronic, every remaining pick triggers the saturation
        // penalty, so the counter increments on subsequent slots.
        let cands: Vec<RadioCandidate> = (1..=6)
            .map(|i| {
                make_cand_full(
                    RadioSource::Library,
                    i,
                    &format!("artist{i}"),
                    &format!("t{i}"),
                    None,
                    0.5,
                )
            })
            .collect();
        let primary_genres: HashMap<i64, String> =
            (1..=6).map(|i| (i, "electronic".to_string())).collect();
        let profile = crate::services::radio_config::RadioProfile {
            genre_saturation_penalty: 0.1,
            same_artist_penalty: 0.0,
            same_album_penalty: 0.0,
            diversity_weight: 1.0,
            ..crate::services::radio_config::RadioProfile::mixed()
        };
        let mut counters = RerankCounters::default();
        let _queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Mixed,
            6,
            &primary_genres,
            &HashSet::new(),
            false,
            &mut counters,
        );
        assert!(
            counters.genre_saturation_penalties >= 2,
            "expected ≥2 penalty applications by slot 6 with all-electronic, got {}",
            counters.genre_saturation_penalties,
        );
    }

    #[test]
    fn diversity_rerank_source_quota_bonus_promotes_underrepresented_source() {
        // 3 library candidates, 3 lastfm candidates, identical raw scores.
        // With Familiar blend (lib=0.60, lfm=0.30, eng=0.10) and the bonus
        // enabled, library should get 60% target → seed picks tilt library
        // until the quota balances. Specifically: slot 2 has lib_count=1,
        // target=0.60×1=0.60, actual=1 ≥ target → no bonus. lfm count=0,
        // target=0.30×1=0.30, actual=0 < target → +5%. So slot 2 should be lfm.
        let mut cands = Vec::new();
        for i in 1..=3 {
            cands.push(make_cand_full(
                RadioSource::Library,
                i,
                &format!("la{i}"),
                "t",
                None,
                0.5,
            ));
        }
        for i in 100..=102 {
            cands.push(make_cand_full(
                RadioSource::Lastfm,
                0,
                &format!("fa{i}"),
                "t",
                None,
                0.5,
            ));
        }
        let profile = crate::services::radio_config::RadioProfile {
            same_artist_penalty: 0.0,
            same_album_penalty: 0.0,
            genre_saturation_penalty: 0.0,
            diversity_weight: 1.0,
            ..crate::services::radio_config::RadioProfile::familiar()
        };
        let mut counters = RerankCounters::default();
        let queue = diversity_rerank(
            cands,
            &profile,
            RadioBlend::Familiar,
            6,
            &HashMap::new(),
            &HashSet::new(),
            true, // quota bonus on
            &mut counters,
        );
        // Final mix should reflect blend weights: with 6 slots and
        // (0.60, 0.30, 0.10), library ≈ 3-4 slots, lastfm ≈ 2 slots.
        let lib_count = queue.iter().filter(|c| c.source == RadioSource::Library).count();
        let lfm_count = queue.iter().filter(|c| c.source == RadioSource::Lastfm).count();
        assert!(
            (3..=4).contains(&lib_count),
            "expected ~3-4 library slots with source quota, got {lib_count}",
        );
        assert!(
            lfm_count >= 1,
            "lastfm should be present in queue under quota bonus, got {lfm_count}",
        );
    }

    #[test]
    fn normalize_isolates_per_source() {
        // Library scores are tightly clustered around 0.7, lastfm around 0.3.
        // Without normalization, library's narrow band would lose to lastfm's
        // wider one in any rank-mixing step. After normalization, each source's
        // top should map to ~1.0 independent of the other.
        let mut cands: Vec<RadioCandidate> = Vec::new();
        let lib_scores = [0.65, 0.68, 0.70, 0.72, 0.75];
        for (i, &s) in lib_scores.iter().enumerate() {
            cands.push(make_cand(RadioSource::Library, 100 + i as i64, s));
        }
        let lfm_scores = [0.10, 0.20, 0.30, 0.40, 0.50];
        for (i, &s) in lfm_scores.iter().enumerate() {
            cands.push(make_cand(RadioSource::Lastfm, 200 + i as i64, s));
        }

        normalize_source_scores(&mut cands);

        let lib_top = cands
            .iter()
            .filter(|c| c.source == RadioSource::Library)
            .map(|c| c.similarity_score)
            .fold(0.0_f64, f64::max);
        let lfm_top = cands
            .iter()
            .filter(|c| c.source == RadioSource::Lastfm)
            .map(|c| c.similarity_score)
            .fold(0.0_f64, f64::max);
        assert!((lib_top - 1.0).abs() < 1e-6);
        assert!((lfm_top - 1.0).abs() < 1e-6);
    }

    #[test]
    fn session_id_starts_with_rad_and_is_unique() {
        let a = new_session_id();
        // Force a tick so the nanos count differs.
        std::thread::sleep(std::time::Duration::from_nanos(1));
        let b = new_session_id();
        assert!(a.starts_with("rad_"));
        assert!(b.starts_with("rad_"));
        // Note: rare race could fail this — but `Duration::from_nanos(1)` plus the
        // syscall round-trip makes collision astronomically unlikely.
        assert_ne!(a, b, "session ids should differ across calls");
    }

    #[test]
    fn radio_blend_default_is_mixed() {
        assert_eq!(RadioBlend::default(), RadioBlend::Mixed);
    }

    #[test]
    fn radio_blend_serde_roundtrip() {
        for blend in [RadioBlend::Familiar, RadioBlend::Mixed, RadioBlend::Adventurous] {
            let s = serde_json::to_string(&blend).unwrap();
            let back: RadioBlend = serde_json::from_str(&s).unwrap();
            assert_eq!(blend, back);
        }
    }
}

#[cfg(test)]
mod radio_phase2_tests {
    //! Phase 2a Stage 1 component tests for the dedup tie-break change
    //! and the `apply_taste_signals` pass.
    //!
    //! A full orchestrator-level snapshot test would require seeding an
    //! embedding model (for `radio_from_neighbors` to return library
    //! results) plus a Last.fm stub; both add scope without strengthening
    //! the gate beyond what these component tests already cover. The
    //! orchestrator wiring is exercised end-to-end by the existing radio
    //! API endpoints in production.
    use super::*;
    use crate::smart::taste_vector::{AffinitySignal, TasteVector};

    fn cand(
        track_id: i64,
        source: RadioSource,
        artist_name: &str,
        title: &str,
        score: f64,
    ) -> RadioCandidate {
        RadioCandidate {
            track_id,
            tidal_track_id: None,
            title: title.to_string(),
            artist_name: artist_name.to_string(),
            album_title: None,
            artwork_url: None,
            duration_ms: None,
            isrc: None,
            is_in_library: source == RadioSource::Library,
            source,
            reason: format!("test {source:?}"),
            similarity_score: score,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        }
    }

    // ─── combine_with_dedup: 5% library tie-break rule ────────────────────────

    #[test]
    fn dedup_library_wins_when_within_five_percent() {
        // Library 0.96 vs Lastfm 1.00 — library is 96% of lastfm, inside the
        // 0.95 threshold, so library still wins despite lower raw score.
        let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.96)];
        let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 1.0)];
        let out = combine_with_dedup(lib, lfm, Vec::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Library);
        assert_eq!(out[0].track_id, 1);
    }

    #[test]
    fn dedup_lastfm_wins_when_library_below_threshold() {
        // Library 0.80 vs Lastfm 1.00 — library is 80%, below 0.95
        // threshold, so the higher non-library score wins.
        let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.80)];
        let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 1.0)];
        let out = combine_with_dedup(lib, lfm, Vec::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Lastfm);
    }

    #[test]
    fn dedup_library_wins_alone_when_only_source_present() {
        // No competing source: library wins by default (0.95 of nothing
        // is satisfied).
        let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.20)];
        let out = combine_with_dedup(lib, Vec::new(), Vec::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Library);
    }

    #[test]
    fn dedup_picks_highest_when_no_library_in_group() {
        // Lastfm 0.60 vs Engine 0.85 — no library, highest score wins.
        let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.60)];
        let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.85)];
        let out = combine_with_dedup(Vec::new(), lfm, eng);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Engine);
    }

    #[test]
    fn dedup_tie_break_prefers_library_then_engine_then_lastfm() {
        // All three sources at identical score 0.5. Library wins (and
        // would win regardless via the 5% rule), but the tie-break
        // ordering also matters when library is absent.
        let lib = vec![cand(1, RadioSource::Library, "A", "Song", 0.5)];
        let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
        let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.5)];
        let out = combine_with_dedup(lib, lfm, eng);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Library);

        // Without library, engine should beat lastfm at the same score.
        let lfm = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
        let eng = vec![cand(2, RadioSource::Engine, "A", "Song", 0.5)];
        let out = combine_with_dedup(Vec::new(), lfm, eng);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, RadioSource::Engine);
    }

    #[test]
    fn dedup_keeps_unique_candidates_in_first_seen_order() {
        let lib = vec![
            cand(1, RadioSource::Library, "A", "First", 0.9),
            cand(2, RadioSource::Library, "B", "Second", 0.8),
        ];
        let lfm = vec![cand(0, RadioSource::Lastfm, "C", "Third", 0.7)];
        let out = combine_with_dedup(lib, lfm, Vec::new());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].title, "First");
        assert_eq!(out[1].title, "Second");
        assert_eq!(out[2].title, "Third");
    }

    #[test]
    fn dedup_drops_candidates_with_empty_normalised_key() {
        // Empty artist + title produces an empty norm key and is dropped.
        let lib = vec![
            cand(1, RadioSource::Library, "", "", 0.9),
            cand(2, RadioSource::Library, "Real", "Song", 0.8),
        ];
        let out = combine_with_dedup(lib, Vec::new(), Vec::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].track_id, 2);
    }

    // ─── apply_taste_signals: hard suppression + artist affinity ──────────────

    fn make_taste(
        skipped: &[i64],
        artist_signals: &[(i64, f64, f64)], // (artist_id, pos, neg)
    ) -> TasteVector {
        let mut t = TasteVector::default();
        for id in skipped {
            t.skipped_track_ids.insert(*id);
        }
        for (artist_id, pos, neg) in artist_signals {
            t.artist_affinity
                .insert(*artist_id, AffinitySignal { pos: *pos, neg: *neg });
        }
        t
    }

    #[test]
    fn apply_taste_signals_is_noop_on_empty_taste() {
        let taste = TasteVector::default();
        let resolver = ArtistResolver::default();
        let mut candidates = vec![
            cand(1, RadioSource::Library, "A", "Song1", 0.8),
            cand(2, RadioSource::Library, "B", "Song2", 0.6),
        ];
        let before = candidates.clone();
        apply_taste_signals(&mut candidates, &taste, &resolver);

        assert_eq!(candidates.len(), before.len());
        for (a, b) in candidates.iter().zip(before.iter()) {
            assert_eq!(a.track_id, b.track_id);
            assert!((a.similarity_score - b.similarity_score).abs() < 1e-12);
        }
    }

    #[test]
    fn apply_taste_signals_drops_skipped_library_candidates() {
        let taste = make_taste(&[2], &[]);
        let resolver = ArtistResolver::default();
        let mut candidates = vec![
            cand(1, RadioSource::Library, "A", "Keep", 0.8),
            cand(2, RadioSource::Library, "B", "Drop", 0.9),
            cand(3, RadioSource::Engine, "C", "Keep", 0.7),
        ];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|c| c.track_id != 2));
    }

    #[test]
    fn apply_taste_signals_does_not_drop_lastfm_with_zero_track_id() {
        // Lastfm hits have track_id = 0 even when track_id 0 is in
        // skipped_track_ids; they should never be hard-suppressed by this
        // path.
        let taste = make_taste(&[0], &[]);
        let resolver = ArtistResolver::default();
        let mut candidates = vec![cand(0, RadioSource::Lastfm, "A", "Song", 0.5)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn apply_taste_signals_nudges_score_for_known_artist() {
        // Artist 1: pos=10, neg=0. Saturation K=10:
        //   pos_c = 10/20 = 0.5, neg_c = 0
        //   multiplier = 1.0 + 0.5*0.20 - 0*0.30 = 1.10
        //   final = 0.5 * 1.10 = 0.55
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 10.0, 0.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert!((candidates[0].similarity_score - 0.55).abs() < 1e-12);
    }

    #[test]
    fn apply_taste_signals_penalises_negative_artist() {
        // Artist 1: pos=0, neg=10. Saturation K=10:
        //   pos_c = 0, neg_c = 10/20 = 0.5
        //   multiplier = 1.0 + 0 - 0.5*0.30 = 0.85
        //   final = 0.5 * 0.85 = 0.425
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 0.0, 10.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert!((candidates[0].similarity_score - 0.425).abs() < 1e-12);
    }

    #[test]
    fn apply_taste_signals_skips_unknown_artist_silently() {
        // Resolver has no entry for "Unknown"; affinity adjustment doesn't
        // fire and the score stays as-is.
        let resolver = ArtistResolver::default();
        let taste = make_taste(&[], &[(1, 10.0, 0.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "Unknown", "Song", 0.5)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert!((candidates[0].similarity_score - 0.5).abs() < 1e-12);
    }

    #[test]
    fn apply_taste_signals_never_zeroes_under_high_negative_signal() {
        // Regression guard for the Doja Cat case. Old formula at neg=20:
        //   multiplier = 1.0 - 20*0.07 = -0.4, clamped to 0.0,
        //   destroying every library candidate from a recently-skipped
        //   artist.
        // New saturating formula at neg=20:
        //   neg_c = 20/30 = 0.667, multiplier = 1.0 - 0.667*0.30 = 0.80
        //   final = 0.5 * 0.80 = 0.40
        // Library candidates from skipped artists are demoted, not
        // eliminated.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 0.0, 20.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 0.5)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        assert!(
            candidates[0].similarity_score > 0.3,
            "expected score > 0.3, got {}",
            candidates[0].similarity_score
        );
        assert!((candidates[0].similarity_score - 0.40).abs() < 1e-12);
    }

    #[test]
    fn apply_taste_signals_neg_50_does_not_zero_score() {
        // High-magnitude neg: even a heavily-skipped artist should keep
        // a meaningful score. With the old 0.05/0.07 formula, neg = 50
        // gave multiplier = -2.5 → clamped to 0. With saturating
        // compression:
        //   neg_c = 50/60 = 0.833, multiplier = 1.0 - 0.833*0.30 = 0.75
        //   final = 0.5 * 0.75 = 0.375
        // The asymptote is 1.0 - 0.30 = 0.70 regardless of how large
        // neg gets, which is the point of the saturation curve.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 0.0, 50.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.0)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        let multiplier = candidates[0].similarity_score;
        assert!(
            multiplier > 0.5,
            "neg=50 should leave multiplier > 0.5, got {multiplier}"
        );
        assert!((multiplier - 0.75).abs() < 1e-9);
    }

    #[test]
    fn apply_taste_signals_pos_50_does_not_overshoot() {
        // High-magnitude pos: a beloved artist still gets a bounded
        // boost, not unbounded growth that would swamp source-native
        // similarity. With the old 0.05 formula, pos = 50 gave
        // multiplier = 3.5 → score 5x boosted, which would dwarf
        // last.fm match scores entirely. With saturating compression:
        //   pos_c = 50/60 = 0.833, multiplier = 1.0 + 0.833*0.20 = 1.167
        // Asymptote is 1.0 + 0.20 = 1.20.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 50.0, 0.0)]);
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.0)];
        apply_taste_signals(&mut candidates, &taste, &resolver);
        let multiplier = candidates[0].similarity_score;
        assert!(
            (1.10..=1.30).contains(&multiplier),
            "pos=50 multiplier should sit in [1.10, 1.30], got {multiplier}"
        );
    }

    // ─── Stage 2: engine slot fills with track_similarity ─────────────────────

    /// Build an in-memory Database with the full schema and seed:
    ///   - artist 1 "A"
    ///   - 4 tracks (100..103) — 100 is the radio seed
    ///   - 3 track_similarity rows pointing seed→{101, 102, 103} with
    ///     different score components (co-album, co-artist, genre proximity)
    ///
    /// The seeded rows represent three legitimate library-similarity
    /// signals that the engine slot should surface:
    ///   - track 101 (sim 0.85, co_album=1.0): same album as seed.
    ///   - track 102 (sim 0.65, co_artist=1.0): same artist, different album.
    ///   - track 103 (sim 0.30, genre=0.5):    shared genre branch only.
    ///
    /// These are the three new tracks the engine slot brings to a radio
    /// queue that previously (Stage 1) would have returned only library
    /// (embedding) and lastfm results. None of them require an embedding
    /// model to surface; track_similarity is the second recall path.
    fn seed_engine_test_db() -> Database {
        let db = Database::open(":memory:").expect("in-memory db");
        db.run_migrations().expect("run migrations");
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            for id in 100..=103 {
                conn.execute(
                    "INSERT INTO tracks (id, title, artist_id, album_id, source) \
                     VALUES (?1, ?2, 1, NULL, 'tidal')",
                    rusqlite::params![id, format!("Track {id}")],
                )?;
            }
            // track_similarity has CHECK (track_a < track_b), so seed=100
            // is always track_a. similarity_score is the rolled-up field
            // that engine_results_from_track_similarity sorts by.
            for (b, sim, co_album, co_artist, genre) in [
                (101_i64, 0.85_f64, 1.0_f64, 0.0_f64, 0.0_f64),
                (102_i64, 0.65_f64, 0.0_f64, 1.0_f64, 0.0_f64),
                (103_i64, 0.30_f64, 0.0_f64, 0.0_f64, 0.5_f64),
            ] {
                conn.execute(
                    "INSERT INTO track_similarity \
                     (track_a, track_b, similarity_score, co_album_score, co_artist_score, genre_proximity) \
                     VALUES (100, ?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![b, sim, co_album, co_artist, genre],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn engine_returns_seeded_track_similarity_in_score_order() {
        let db = seed_engine_test_db();
        let results =
            engine_results_from_track_similarity(&db, 100, 10, &[]).expect("engine results");

        assert_eq!(results.len(), 3, "expected 3 seeded similarity rows");

        // Ordering: 101 (0.85) > 102 (0.65) > 103 (0.30).
        let ids: Vec<i64> = results.iter().map(|c| c.track_id).collect();
        assert_eq!(ids, vec![101, 102, 103]);

        // Provenance: every result is engine-source, in-library, with a
        // reason string carrying the score breakdown so the
        // "Why is this here?" UI can show it.
        for cand in &results {
            assert_eq!(cand.source, RadioSource::Engine);
            assert!(cand.is_in_library);
            assert!(cand.reason.starts_with("library similarity"));
            assert!(cand.reason.contains("co-album"));
            assert!(cand.reason.contains("co-artist"));
            assert!(cand.reason.contains("genre"));
        }

        // Specific component check: track 101 was seeded with co_album=1.0,
        // so its reason should mention that magnitude.
        let r101 = results.iter().find(|c| c.track_id == 101).unwrap();
        assert!(r101.reason.contains("co-album 1.00"));
    }

    #[test]
    fn engine_respects_target_limit() {
        let db = seed_engine_test_db();
        let results = engine_results_from_track_similarity(&db, 100, 2, &[])
            .expect("engine results truncated to 2");
        assert_eq!(results.len(), 2);
        // Top 2 by similarity_score: 101 (0.85), 102 (0.65).
        assert_eq!(results[0].track_id, 101);
        assert_eq!(results[1].track_id, 102);
    }

    #[test]
    fn engine_excludes_listed_track_ids() {
        let db = seed_engine_test_db();
        // Exclude track 101 — should drop the top result.
        let results = engine_results_from_track_similarity(&db, 100, 10, &[101])
            .expect("engine results with exclusion");
        let ids: Vec<i64> = results.iter().map(|c| c.track_id).collect();
        assert_eq!(ids, vec![102, 103]);
    }

    #[test]
    fn engine_returns_empty_when_target_is_zero() {
        let db = seed_engine_test_db();
        let results = engine_results_from_track_similarity(&db, 100, 0, &[]).expect("zero target");
        assert!(results.is_empty());
    }

    /// Stage 2 before/after diff demonstration with affinity logging.
    ///
    /// Before (Stage 1): library + lastfm + empty engine = `empty_input`
    /// candidate set after combine_with_dedup. With empty taste and an
    /// empty resolver, apply_taste_signals is a no-op. Result is the
    /// `empty_input` count.
    ///
    /// After (Stage 2): library + lastfm + non-empty engine = three new
    /// engine candidates surface. Each is a real library track with
    /// documented similarity provenance. None override existing dedup
    /// winners (they cover artist names not present in the library/
    /// lastfm slots in this fixture).
    ///
    /// Per-candidate affinity multiplier is logged for visibility — soft
    /// signal, not a gate. With the small artist-affinity values seeded
    /// here the multipliers stay close to 1.0; if a future change made
    /// the formula aggressive enough to swamp the source-native score,
    /// these logs would make it obvious.
    #[test]
    fn stage_2_engine_diff_with_affinity_logging() {
        let db = seed_engine_test_db();

        // Empty engine baseline (Stage 1 shape): combine library +
        // lastfm + empty engine. Use empty source slots since this test
        // exercises only the diff brought by the engine slot itself.
        let mut empty_path =
            combine_with_dedup(Vec::new(), Vec::new(), Vec::new());
        assert!(empty_path.is_empty(), "no candidates without engine slot");

        // Engine-on (Stage 2 shape): same combine, with engine populated.
        let engine = engine_results_from_track_similarity(&db, 100, 10, &[])
            .expect("engine results");
        let engine_count = engine.len();
        let mut engine_path = combine_with_dedup(Vec::new(), Vec::new(), engine);

        assert_eq!(
            engine_path.len(),
            engine_count,
            "every engine candidate survives dedup when no other source competes"
        );

        // Apply a non-empty taste to exercise the affinity path. Artist
        // 1 (which all engine candidates belong to) is mildly liked.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO artists VALUES (1, 'A')", []).unwrap();
        let resolver = ArtistResolver::load(&conn).unwrap();
        let taste = make_taste(&[], &[(1, 4.0, 1.0)]); // pos=4, neg=1

        // Snapshot scores before affinity to compute the multiplier
        // empirically per candidate.
        let pre: Vec<(i64, f64)> = engine_path
            .iter()
            .map(|c| (c.track_id, c.similarity_score))
            .collect();

        apply_taste_signals(&mut engine_path, &taste, &resolver);

        // Stage 2 visibility: print per-candidate affinity multiplier
        // so any future formula change shows up here. Soft signal,
        // not a hard gate.
        eprintln!(
            "stage 2 affinity multipliers (artist=A, pos=4, neg=1):"
        );
        let post: Vec<(i64, f64)> = engine_path
            .iter()
            .map(|c| (c.track_id, c.similarity_score))
            .collect();
        for (track_id, post_score) in &post {
            let pre_score = pre.iter().find(|(id, _)| id == track_id).unwrap().1;
            let multiplier = post_score / pre_score;
            eprintln!(
                "  track {track_id}: pre={pre_score:.4} post={post_score:.4} multiplier={multiplier:.4}"
            );
        }

        // Expected multiplier from the saturating formula:
        //   pos_c = 4/(4+10) = 0.2857..
        //   neg_c = 1/(1+10) = 0.0909..
        //   mult  = 1.0 + 0.2857*0.20 - 0.0909*0.30 ≈ 1.02987
        // Same artist on every candidate, so every multiplier is the
        // same. Compared to the old 0.05/0.07 formula's 1.13, the
        // saturated version produces a smaller nudge for low-magnitude
        // pos/neg — which is correct: "barely any signal" should
        // barely move the score.
        let expected_mult = 1.0
            + (4.0_f64 / 14.0) * 0.20
            - (1.0_f64 / 11.0) * 0.30;
        for (track_id, post_score) in &post {
            let pre_score = pre.iter().find(|(id, _)| id == track_id).unwrap().1;
            let actual_mult = post_score / pre_score;
            assert!(
                (actual_mult - expected_mult).abs() < 1e-9,
                "expected multiplier {expected_mult:.6} for track {track_id}, got {actual_mult:.6}"
            );
        }

        // Justification (for the commit and for future readers):
        // - Track 101 surfaces because it shares an album with the seed
        //   (co_album=1.0). Same-album tracks are correctly classified as
        //   similar by the precomputed table.
        // - Track 102 surfaces because it shares the artist (co_artist=1.0)
        //   without sharing the album — exactly the kind of "more by this
        //   artist" expansion radio should offer.
        // - Track 103 surfaces from genre-proximity alone (genre=0.5) at a
        //   correspondingly lower score, reflecting weaker library
        //   evidence for similarity.
        // None of these would have surfaced in Stage 1 because the engine
        // slot was empty; the embedding model (the other library recall
        // path) is independent and may or may not also surface them.
        let ids: Vec<i64> = engine_path.iter().map(|c| c.track_id).collect();
        assert_eq!(ids, vec![101, 102, 103]);
    }

    // ─── Phase 2b Stage 1: reason-string JSON suffix ──────────────────────────

    #[test]
    fn annotate_reasons_appends_json_suffix_for_library_candidate() {
        // Library candidate with all three signals populated. Suffix
        // carries genre_jaccard, affinity_mult, and genre_mult.
        let mut candidates = vec![cand(100, RadioSource::Library, "A", "Song", 1.32)];
        let key = (
            RadioSource::Library,
            100,
            normalize_for_dedup("A", "Song"),
        );
        let pre_affinity =
            std::collections::HashMap::from([(key.clone(), 1.0_f64)]);
        // Post-affinity = 1.10 means apply_taste_signals applied a +10% nudge.
        // The candidate's current similarity_score (1.32) divided by post_affinity
        // gives genre_mult = 1.20.
        let post_affinity =
            std::collections::HashMap::from([(key.clone(), 1.10_f64)]);
        let jaccard_by_key =
            std::collections::HashMap::from([(key, 0.67_f64)]);
        annotate_reasons(
            &mut candidates,
            &pre_affinity,
            &post_affinity,
            &jaccard_by_key,
        );
        let reason = &candidates[0].reason;
        assert!(reason.contains(" | "), "expected JSON suffix separator, got: {reason}");
        assert!(reason.contains("\"genre_jaccard\":0.6700"), "got: {reason}");
        assert!(reason.contains("\"affinity_mult\":1.1000"), "got: {reason}");
        assert!(reason.contains("\"genre_mult\":1.2000"), "got: {reason}");
    }

    #[test]
    fn annotate_reasons_emits_partial_suffix_when_only_affinity_available() {
        // Last.fm candidate (track_id=0): no Jaccard, no genre_mult
        // (lastfm passes through apply_genre_signals untouched).
        let mut candidates = vec![cand(0, RadioSource::Lastfm, "B", "Tune", 0.20)];
        let key = (
            RadioSource::Lastfm,
            0,
            normalize_for_dedup("B", "Tune"),
        );
        let pre_affinity =
            std::collections::HashMap::from([(key.clone(), 0.20_f64)]);
        // Post-affinity equals the live score (no genre pass touched it).
        let post_affinity =
            std::collections::HashMap::from([(key, 0.20_f64)]);
        let jaccard_by_key: std::collections::HashMap<(RadioSource, i64, String), f64> =
            std::collections::HashMap::new();
        annotate_reasons(
            &mut candidates,
            &pre_affinity,
            &post_affinity,
            &jaccard_by_key,
        );
        let reason = &candidates[0].reason;
        assert!(reason.contains(" | "), "got: {reason}");
        assert!(!reason.contains("genre_jaccard"), "got: {reason}");
        assert!(reason.contains("\"affinity_mult\":1.0000"), "got: {reason}");
        // genre_mult is post / post = 1.0 — emitted, but indistinguishable
        // from no-op. That's fine; the tooltip filters near-1.0 values.
    }

    #[test]
    fn annotate_reasons_skips_when_no_signals_present() {
        // No pre_affinity / post_affinity / Jaccard entries. Reason
        // string is left untouched.
        let mut candidates = vec![cand(0, RadioSource::Lastfm, "C", "Song", 0.5)];
        let original_reason = candidates[0].reason.clone();
        let pre_affinity: std::collections::HashMap<(RadioSource, i64, String), f64> =
            std::collections::HashMap::new();
        let post_affinity: std::collections::HashMap<(RadioSource, i64, String), f64> =
            std::collections::HashMap::new();
        let jaccard_by_key: std::collections::HashMap<(RadioSource, i64, String), f64> =
            std::collections::HashMap::new();
        annotate_reasons(
            &mut candidates,
            &pre_affinity,
            &post_affinity,
            &jaccard_by_key,
        );
        assert_eq!(candidates[0].reason, original_reason);
    }

    // ─── Phase 2b Stage 2: genre coherence scoring ────────────────────────────

    fn run_genre_signals(
        cand_pairs: &[(RadioSource, i64, &str, &str, f64)],
        jaccards: &[(RadioSource, i64, &str, &str, f64)],
        blend: RadioBlend,
    ) -> Vec<RadioCandidate> {
        let mut candidates: Vec<RadioCandidate> = cand_pairs
            .iter()
            .map(|(s, tid, an, t, score)| cand(*tid, *s, an, t, *score))
            .collect();
        let map: HashMap<(RadioSource, i64, String), f64> = jaccards
            .iter()
            .map(|(s, tid, an, t, j)| {
                ((*s, *tid, normalize_for_dedup(an, t)), *j)
            })
            .collect();
        apply_genre_signals(&mut candidates, &map, blend);
        candidates
    }

    #[test]
    fn genre_score_multiplier_full_overlap() {
        // jaccard 1.0: bonus 0.30, no penalty (>= 0.5). Multiplier 1.30.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            RadioBlend::Mixed,
        );
        assert_eq!(result.len(), 1);
        assert!(
            (result[0].similarity_score - 1.30).abs() < 1e-9,
            "got {}",
            result[0].similarity_score
        );
    }

    #[test]
    fn genre_score_multiplier_partial_no_penalty_at_threshold() {
        // jaccard 0.5: bonus 0.15, no penalty (penalty branch fires only
        // for jaccard < 0.5). Multiplier 1.15.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.5)],
            RadioBlend::Mixed,
        );
        assert!((result[0].similarity_score - 1.15).abs() < 1e-9);
    }

    #[test]
    fn genre_score_multiplier_zero_overlap() {
        // jaccard 0.0 under Adventurous (no hard reject):
        // bonus 0.0, penalty 1.0*0.20 = 0.20. Multiplier 0.80.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.0)],
            RadioBlend::Adventurous,
        );
        assert!((result[0].similarity_score - 0.80).abs() < 1e-9);
    }

    #[test]
    fn genre_score_multiplier_floor_clamp() {
        // Pathological negative multiplier (shouldn't happen with the
        // current formula but defensive floor must hold).
        // The formula floors at 0.1 — even with the bonus and penalty
        // values, jaccard=0 produces 0.80, not below the floor.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.0)],
            RadioBlend::Adventurous,
        );
        assert!(result[0].similarity_score >= 0.1);
    }

    #[test]
    fn genre_hard_reject_familiar_drops_disjoint() {
        // jaccard 0.05 under Familiar (threshold 0.10): drop.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.05)],
            RadioBlend::Familiar,
        );
        assert_eq!(result.len(), 0, "Familiar should hard-reject jaccard 0.05");
    }

    #[test]
    fn genre_hard_reject_mixed_borderline() {
        // jaccard 0.04 under Mixed (threshold 0.05): drop.
        // jaccard 0.06 under Mixed: keep.
        let dropped = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.04)],
            RadioBlend::Mixed,
        );
        assert_eq!(dropped.len(), 0);
        let kept = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.06)],
            RadioBlend::Mixed,
        );
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn genre_hard_reject_adventurous_keeps_disjoint() {
        // jaccard 0.0 under Adventurous: keep (no hard reject), score
        // demoted by penalty.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 1.0)],
            &[(RadioSource::Library, 100, "A", "Song", 0.0)],
            RadioBlend::Adventurous,
        );
        assert_eq!(result.len(), 1);
        assert!(result[0].similarity_score < 1.0);
    }

    #[test]
    fn genre_lastfm_skipped_when_no_genre_data() {
        // Lastfm candidate with track_id=0 has no entry in jaccard_by_key.
        // It must pass through apply_genre_signals with similarity_score
        // unchanged AND not be hard-rejected even under Familiar.
        let result = run_genre_signals(
            &[(RadioSource::Lastfm, 0, "C", "Song", 0.20)],
            &[],
            RadioBlend::Familiar,
        );
        assert_eq!(result.len(), 1);
        assert!((result[0].similarity_score - 0.20).abs() < 1e-9);
    }

    #[test]
    fn genre_library_with_no_jaccard_passes_through() {
        // Library candidate without a jaccard entry (e.g. seed had no
        // genres, or this candidate had no genre rows). Pass through.
        let result = run_genre_signals(
            &[(RadioSource::Library, 100, "A", "Song", 0.50)],
            &[],
            RadioBlend::Familiar,
        );
        assert_eq!(result.len(), 1);
        assert!((result[0].similarity_score - 0.50).abs() < 1e-9);
    }
}

#[cfg(test)]
mod radio_diagnostic_harness {
    //! # Radio quality diagnostic harness
    //!
    //! Permanent debug tool, not dead code. Run against the live
    //! database to inspect what `orchestrate_song` actually produces for
    //! a given seed track and why.
    //!
    //! ## Usage
    //!
    //! ```bash
    //! NOOR_SEED=1634 NOOR_DB=/e/NOORwave/noor.db \
    //!     cargo test -p noor-server radio_diagnostic_for_seed \
    //!     -- --ignored --nocapture
    //! ```
    //!
    //! Defaults: `NOOR_SEED=1634` (Doja Cat — "Go To Town" — Amala, the
    //! seed that surfaced the Phase 2a affinity-zeroing regression),
    //! `NOOR_DB=e:/NOORwave/noor.db`. Override either with the env var.
    //!
    //! ## What it prints
    //!
    //! - The seed's title, artist, and genres.
    //! - TasteVector fixture sizes (artist/genre affinity entries,
    //!   skipped/recent track counts).
    //! - Per-source candidate counts before dedup.
    //! - Survivor count after `combine_with_dedup` and after
    //!   `apply_taste_signals`.
    //! - A per-candidate table for the final queue: source, track_id,
    //!   pre-affinity score, post-affinity score, multiplier, and genre
    //!   overlap with the seed.
    //! - The top 30 `track_similarity` neighbours with their score
    //!   components and genre overlap.
    //! - Hypothesis-discriminator summary lines so a regression has an
    //!   immediately-readable verdict.
    //!
    //! ## Why `#[ignore]`
    //!
    //! Touches the live database and (when configured) live Last.fm.
    //! Out of scope for `cargo test` defaults; only run on demand. The
    //! `#[ignore]` gate is the cleanest way to opt the harness out of
    //! CI while keeping it inside the same crate as the private radio
    //! helpers it inspects.
    //!
    //! ## Adding a new diagnostic seed
    //!
    //! Just pass `NOOR_SEED=<id>`. No code change needed. Pick a seed
    //! whose listen-history pattern matches the regression class you're
    //! investigating: heavy negative signal seeds (recently-skipped
    //! artist) are likely to have demoted library candidates;
    //! positive-history seeds should keep library on top.
    use super::*;
    use crate::metadata::lastfm::LastFmClient;
    use std::collections::{HashMap, HashSet};

    fn track_genres(db: &Database, track_id: i64) -> HashSet<i64> {
        if track_id <= 0 {
            return HashSet::new();
        }
        db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT genre_id FROM track_genres WHERE track_id = ?1")?;
            let ids = stmt
                .query_map(rusqlite::params![track_id], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids.into_iter().collect())
        })
        .unwrap_or_default()
    }

    fn truncate(s: &str, n: usize) -> String {
        s.chars().take(n).collect()
    }

    #[tokio::test]
    #[ignore]
    async fn radio_diagnostic_for_seed() {
        let db_path =
            std::env::var("NOOR_DB").unwrap_or_else(|_| "e:/NOORwave/noor.db".to_string());
        let seed_id: i64 = std::env::var("NOOR_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1634);
        let blend = RadioBlend::Mixed;
        let limit: usize = 30;

        let db = Database::open(&db_path).expect("open live db");

        // ─── Seed metadata + genres ───────────────────────────────────────
        let (seed_title, seed_artist) = db
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT t.title, COALESCE(a.name, '?') FROM tracks t \
                     LEFT JOIN artists a ON a.id = t.artist_id WHERE t.id = ?1",
                    rusqlite::params![seed_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(anyhow::Error::from)
            })
            .expect("seed lookup");
        let seed_genres = track_genres(&db, seed_id);
        let seed_genre_names: Vec<String> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT g.name FROM track_genres tg JOIN genres g ON g.id = tg.genre_id WHERE tg.track_id = ?1",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![seed_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap_or_default();
        eprintln!(
            "\n=== SEED: track {seed_id} '{seed_title}' by '{seed_artist}' ===\n  Genres: {:?}\n",
            seed_genre_names
        );

        // ─── TasteVector + ArtistResolver ─────────────────────────────────
        let (taste, resolver) = build_taste_inputs(&db, seed_id);
        eprintln!(
            "TasteVector: artist_affinity={}, genre_affinity={}, skipped_track_ids={}, recent_track_ids={}",
            taste.artist_affinity.len(),
            taste.genre_affinity.len(),
            taste.skipped_track_ids.len(),
            taste.recent_track_ids.len()
        );

        // ─── Replicate orchestrate_song's source generation ───────────────
        let exclude_set: HashSet<i64> = HashSet::new();
        let (lib_w, lfm_w, eng_w) = blend.weights();
        let target_per_source = |w: f64| ((limit as f64 * w * 1.5).ceil() as usize).max(1);
        let lib_target = target_per_source(lib_w);
        let lfm_target = target_per_source(lfm_w);
        let eng_target = target_per_source(eng_w);

        // Library
        let library_results: Vec<RadioCandidate> = {
            let mut excl: Vec<i64> = exclude_set.iter().copied().collect();
            excl.push(seed_id);
            let creativity = match blend {
                RadioBlend::Familiar => 0.15,
                RadioBlend::Mixed => 0.30,
                RadioBlend::Adventurous => 0.50,
            };
            crate::services::learning::radio_from_neighbors(
                &db,
                seed_id,
                &excl,
                lib_target as i64,
                creativity,
            )
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|n| RadioCandidate {
                track_id: n.track_id,
                tidal_track_id: None,
                title: n.title,
                artist_name: n.artist_name.unwrap_or_default(),
                album_title: n.album_title,
                artwork_url: n.artwork_url,
                duration_ms: n.duration_ms,
                isrc: None,
                is_in_library: true,
                source: RadioSource::Library,
                reason: format!("library similarity {:.2}", n.similarity_score),
                similarity_score: n.similarity_score,
                confidence: Some(n.confidence),
                candidate_in_degree_percentile: Some(n.candidate_in_degree_percentile),
                support_count: Some(n.support_count),
                primary_reason: n.primary_reason,
            })
            .collect()
        };
        eprintln!("\nLibrary source: {} candidates (target {})", library_results.len(), lib_target);

        // Last.fm
        let http_client = reqwest::Client::new();
        let lastfm = LastFmClient::load(http_client, &db);
        let lastfm_results: Vec<RadioCandidate> = if let Some(client) = lastfm.as_ref() {
            client
                .track_get_similar(&seed_artist, &seed_title, lfm_target.max(20))
                .await
                .unwrap_or_default()
                .into_iter()
                .take(lfm_target * 2)
                .map(|hit| RadioCandidate {
                    track_id: 0,
                    tidal_track_id: None,
                    title: hit.title,
                    artist_name: hit.artist,
                    album_title: None,
                    artwork_url: None,
                    duration_ms: None,
                    isrc: None,
                    is_in_library: false,
                    source: RadioSource::Lastfm,
                    reason: format!("Last.fm match {:.2}", hit.match_score),
                    similarity_score: hit.match_score.clamp(0.0, 1.0),
                    confidence: None,
                    candidate_in_degree_percentile: None,
                    support_count: None,
                    primary_reason: None,
                })
                .collect()
        } else {
            Vec::new()
        };
        eprintln!(
            "Last.fm source: {} candidates (target {}, client={})",
            lastfm_results.len(),
            lfm_target,
            lastfm.is_some()
        );

        // Engine
        let mut engine_excl: Vec<i64> = exclude_set.iter().copied().collect();
        engine_excl.push(seed_id);
        let engine_results =
            engine_results_from_track_similarity(&db, seed_id, eng_target, &engine_excl)
                .unwrap_or_default();
        eprintln!("Engine source: {} candidates (target {})", engine_results.len(), eng_target);

        // ─── Combine + dedup → snapshot pre-affinity ──────────────────────
        let combined = combine_with_dedup(
            library_results.clone(),
            lastfm_results.clone(),
            engine_results.clone(),
        );
        eprintln!("\nAfter combine_with_dedup: {} candidates", combined.len());

        // Map (source, track_id, normalised key) -> pre-affinity score
        let pre_affinity: HashMap<(RadioSource, i64, String), f64> = combined
            .iter()
            .map(|c| {
                (
                    (
                        c.source,
                        c.track_id,
                        normalize_for_dedup(&c.artist_name, &c.title),
                    ),
                    c.similarity_score,
                )
            })
            .collect();

        // ─── Apply taste signals ──────────────────────────────────────────
        let mut after_affinity = combined.clone();
        apply_taste_signals(&mut after_affinity, &taste, &resolver);
        let dropped_by_suppression = combined.len() - after_affinity.len();
        eprintln!(
            "After apply_taste_signals: {} candidates (dropped {} via skipped_track_ids)",
            after_affinity.len(),
            dropped_by_suppression
        );

        // ─── Compute genre Jaccard + Stage 2 genre signal ─────────────────
        let jaccard_by_key = compute_genre_jaccard(&db, seed_id, &after_affinity);
        let pre_genre = after_affinity.clone();
        let pre_genre_count = pre_genre.len();
        let mut after_genre = after_affinity.clone();
        apply_genre_signals(&mut after_genre, &jaccard_by_key, blend);
        eprintln!(
            "After apply_genre_signals: {} candidates (dropped {} via genre hard reject in {:?})",
            after_genre.len(),
            pre_genre_count - after_genre.len(),
            blend
        );

        // ─── Blend interleave → final queue ───────────────────────────────
        let final_queue = blend_interleave(after_genre.clone(), blend, limit);
        eprintln!("\nFinal queue length: {}\n", final_queue.len());

        // ─── Per-candidate breakdown ──────────────────────────────────────
        // Columns: pre_aff (combine_with_dedup output), post_aff
        // (after apply_taste_signals), final (after apply_genre_signals
        // — the score blend_interleave actually sorts on), aff_mult
        // (post_aff / pre_aff), gen_mult (final / post_aff), jacc (the
        // Jaccard value from compute_genre_jaccard), g_olp (raw genre
        // overlap count between candidate and seed for sanity).
        eprintln!("==== FINAL QUEUE PER-CANDIDATE BREAKDOWN ====");
        eprintln!(
            "{:>3} {:>7} {:>7} {:>8} {:>8} {:>8} {:>6} {:>6} {:>5} {:>5} {:<26} {:<26}",
            "#", "src", "tid", "pre_aff", "post_aff", "final",
            "aff_m", "gen_m", "jacc", "g_olp", "artist", "title"
        );
        let mut zero_overlap_count = 0;
        let mut unknown_overlap_count = 0;
        let mut overlap_present_count = 0;
        let post_affinity_lookup: HashMap<(RadioSource, i64, String), f64> = pre_genre
            .iter()
            .map(|c| {
                (
                    (
                        c.source,
                        c.track_id,
                        normalize_for_dedup(&c.artist_name, &c.title),
                    ),
                    c.similarity_score,
                )
            })
            .collect();
        for (i, cand) in final_queue.iter().enumerate() {
            let key = (
                cand.source,
                cand.track_id,
                normalize_for_dedup(&cand.artist_name, &cand.title),
            );
            let pre = pre_affinity.get(&key).copied().unwrap_or(f64::NAN);
            let post_aff = post_affinity_lookup
                .get(&key)
                .copied()
                .unwrap_or(f64::NAN);
            let aff_m = if pre > 0.0 {
                post_aff / pre
            } else {
                f64::NAN
            };
            let gen_m = if post_aff > 0.0 {
                cand.similarity_score / post_aff
            } else {
                f64::NAN
            };
            let jacc = jaccard_by_key
                .get(&key)
                .copied()
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".to_string());
            let cand_genres = track_genres(&db, cand.track_id);
            let overlap_label = if cand.track_id <= 0 {
                unknown_overlap_count += 1;
                "?".to_string()
            } else {
                let n = cand_genres.intersection(&seed_genres).count();
                if n == 0 {
                    zero_overlap_count += 1;
                } else {
                    overlap_present_count += 1;
                }
                n.to_string()
            };
            let src = match cand.source {
                RadioSource::Library => "library",
                RadioSource::Lastfm => "lastfm",
                RadioSource::Engine => "engine",
            };
            eprintln!(
                "{:>3} {:>7} {:>7} {:>8.4} {:>8.4} {:>8.4} {:>6.3} {:>6.3} {:>5} {:>5} {:<26} {:<26}",
                i + 1,
                src,
                cand.track_id,
                pre,
                post_aff,
                cand.similarity_score,
                aff_m,
                gen_m,
                jacc,
                overlap_label,
                truncate(&cand.artist_name, 24),
                truncate(&cand.title, 24)
            );
        }

        // ─── Top 30 track_similarity neighbours, with genre overlap ───────
        eprintln!("\n==== TOP 30 track_similarity NEIGHBOURS for seed ====");
        let neighbours: Vec<(i64, String, String, f64, f64, f64, f64)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT t.id, t.title, COALESCE(a.name, '?'), \
                            ts.similarity_score, ts.co_album_score, ts.co_artist_score, ts.genre_proximity \
                     FROM track_similarity ts \
                     JOIN tracks t ON t.id = CASE WHEN ts.track_a = ?1 THEN ts.track_b ELSE ts.track_a END \
                     LEFT JOIN artists a ON a.id = t.artist_id \
                     WHERE (ts.track_a = ?1 OR ts.track_b = ?1) AND t.id != ?1 \
                     ORDER BY ts.similarity_score DESC LIMIT 30",
                )?;
                let rows = stmt
                    .query_map(rusqlite::params![seed_id], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, f64>(3)?,
                            row.get::<_, f64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, f64>(6)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(rows)
            })
            .unwrap_or_default();

        eprintln!(
            "{:>3} {:>7} {:>5} {:>5} {:>5} {:>5} {:>5} {:<28} {:<28}",
            "#", "tid", "sim", "alb", "art", "gen", "g_olp", "artist", "title"
        );
        let mut neighbour_overlap = 0;
        for (i, (tid, title, artist, sim, ca, cr, gp)) in neighbours.iter().enumerate() {
            let cand_genres = track_genres(&db, *tid);
            let overlap = cand_genres.intersection(&seed_genres).count();
            if overlap > 0 {
                neighbour_overlap += 1;
            }
            eprintln!(
                "{:>3} {:>7} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5} {:<28} {:<28}",
                i + 1,
                tid,
                sim,
                ca,
                cr,
                gp,
                overlap,
                truncate(artist, 26),
                truncate(title, 26)
            );
        }
        eprintln!(
            "\n{} of top-{} track_similarity neighbours share at least one genre with seed.",
            neighbour_overlap,
            neighbours.len()
        );

        // ─── Summary ──────────────────────────────────────────────────────
        let by_source: HashMap<RadioSource, usize> =
            final_queue.iter().fold(HashMap::new(), |mut m, c| {
                *m.entry(c.source).or_insert(0) += 1;
                m
            });
        eprintln!("\n==== SUMMARY ====");
        eprintln!("Final queue source breakdown: {:?}", by_source);
        eprintln!(
            "Genre overlap in final queue: {} share, {} zero, {} unknown (lastfm)",
            overlap_present_count, zero_overlap_count, unknown_overlap_count
        );

        // ─── Hypothesis discriminator ─────────────────────────────────────
        eprintln!("\n==== HYPOTHESIS DISCRIMINATOR ====");

        // H1: weak source candidates across the board
        let h1_signal = library_results.is_empty() || engine_results.is_empty();
        eprintln!(
            "H1 (weak sources): library={} lastfm={} engine={}{}",
            library_results.len(),
            lastfm_results.len(),
            engine_results.len(),
            if h1_signal { "  [SIGNAL]" } else { "" }
        );

        // H2: apply_taste_signals promoting unrelated artists
        let mut h2_promoted = 0;
        for cand in &after_affinity {
            let key = (
                cand.source,
                cand.track_id,
                normalize_for_dedup(&cand.artist_name, &cand.title),
            );
            if let Some(pre) = pre_affinity.get(&key).copied() {
                if cand.similarity_score > pre * 1.05 && cand.track_id > 0 {
                    let cg = track_genres(&db, cand.track_id);
                    if !cg.is_empty() && cg.intersection(&seed_genres).count() == 0 {
                        h2_promoted += 1;
                    }
                }
            }
        }
        eprintln!(
            "H2 (affinity promotes off-genre): {} candidates promoted >5% with zero genre overlap{}",
            h2_promoted,
            if h2_promoted > 3 { "  [SIGNAL]" } else { "" }
        );

        // H3: dedup letting unrelated cross-source candidates through
        // Specifically: lastfm winners (track_id=0) where the same key had a library candidate that lost.
        let mut h3_lastfm_wins = 0;
        for c in &combined {
            if c.source == RadioSource::Lastfm {
                let norm = normalize_for_dedup(&c.artist_name, &c.title);
                let library_loser_at_same_key = library_results
                    .iter()
                    .any(|lib| normalize_for_dedup(&lib.artist_name, &lib.title) == norm);
                if library_loser_at_same_key {
                    h3_lastfm_wins += 1;
                }
            }
        }
        eprintln!(
            "H3 (cross-source dedup): {} lastfm winners where a library candidate existed at the same key{}",
            h3_lastfm_wins,
            if h3_lastfm_wins > 5 { "  [SIGNAL]" } else { "" }
        );

        // H4: stale track_similarity
        let h4_signal = neighbour_overlap < (neighbours.len() / 2).max(1);
        eprintln!(
            "H4 (stale track_similarity): {}/{} top-30 neighbours share a genre with seed{}",
            neighbour_overlap,
            neighbours.len(),
            if h4_signal { "  [SIGNAL]" } else { "" }
        );
    }
}
