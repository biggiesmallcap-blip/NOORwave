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
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod rerank;
pub(crate) use rerank::*;

mod scoring;
use scoring::*;

pub type LastFmSimilarCache = Arc<Mutex<HashMap<LastFmSimilarCacheKey, LastFmSimilarCacheEntry>>>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LastFmSimilarCacheKey {
    artist: String,
    title: String,
}

#[derive(Debug, Clone)]
pub struct LastFmSimilarCacheEntry {
    fetched_at: Instant,
    requested_limit: usize,
    tracks: Vec<crate::metadata::lastfm::LastFmSimilarTrack>,
}

const LASTFM_SIMILAR_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum RadioBlend {
    Familiar,
    #[default]
    Mixed,
    Adventurous,
}

impl RadioBlend {
    /// Returns (library_weight, lastfm_weight, engine_weight) summing to 1.0.
    ///
    /// Lanes ordered by distance from the seed: engine (album, artist and
    /// co-listen siblings) is closest, learned neighbors next (their reach set
    /// by `creativity`), Last.fm furthest. More adventurous blends move weight
    /// from the closest lane to the furthest; the old Adventurous gave siblings
    /// half the queue.
    pub fn weights(self) -> (f64, f64, f64) {
        match self {
            RadioBlend::Familiar => (0.55, 0.15, 0.30),
            RadioBlend::Mixed => (0.40, 0.40, 0.20),
            RadioBlend::Adventurous => (0.40, 0.50, 0.10),
        }
    }

    /// How far down the learned neighbor list the library lane reaches
    /// (see `learning::radio_from_neighbors`).
    pub fn creativity(self) -> f64 {
        match self {
            RadioBlend::Familiar => 0.0,
            RadioBlend::Mixed => 0.25,
            RadioBlend::Adventurous => 0.50,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RadioSource {
    Library,
    Lastfm,
    Engine,
    /// TIDAL's own track or artist mix, used when the seed has too little
    /// evidence for a radio of its own.
    Tidal,
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

/// Fewer picks than this means the seed has too little evidence of its own (a
/// cold track: no plays, genres or audio analysis, and no Last.fm match), so
/// TIDAL's mix for the seed fills the radio instead of noise.
pub const TIDAL_MIX_FALLBACK_MIN_PICKS: usize = 10;

/// The mix id TIDAL attaches to a track or artist payload: `mixes.TRACK_MIX`
/// or `mixes.ARTIST_MIX`.
pub fn tidal_mix_id(extra: &HashMap<String, serde_json::Value>, kind: &str) -> Option<String> {
    extra
        .get("mixes")?
        .get(kind)?
        .as_str()
        // It goes into a URL path: the same shape rule as the mix routes.
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 96
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
        .map(str::to_string)
}

/// TIDAL mix tracks as radio candidates, in mix order. A track already in the
/// library comes in as that library row; the seed and anything already picked
/// are skipped.
pub fn tidal_mix_candidates(
    conn: &rusqlite::Connection,
    mix: Vec<crate::services::tidal::client::TidalTrack>,
    seed_tidal_id: Option<i64>,
    existing: &[RadioCandidate],
    limit: usize,
) -> Vec<RadioCandidate> {
    let mut seen: HashSet<String> = existing
        .iter()
        .map(|c| normalize_for_dedup(&c.artist_name, &c.title))
        .collect();
    let total = mix.len().max(1) as f64;
    let mut out = Vec::new();
    for (index, track) in mix.into_iter().enumerate() {
        if out.len() >= limit {
            break;
        }
        if Some(track.id) == seed_tidal_id
            || !seen.insert(normalize_for_dedup(&track.artist.name, &track.title))
        {
            continue;
        }
        let local_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM tracks WHERE tidal_id = ?1",
                [track.id],
                |row| row.get(0),
            )
            .ok();
        out.push(RadioCandidate {
            track_id: local_id.unwrap_or(0),
            tidal_track_id: Some(track.id),
            title: track.title,
            artist_name: track.artist.name,
            album_title: track.album.as_ref().map(|album| album.title.clone()),
            artwork_url: track.album.as_ref().and_then(|album| {
                crate::services::tidal::client::TidalClient::get_artwork_url(&album.cover, 640)
            }),
            duration_ms: Some(track.duration * 1000),
            isrc: track.isrc,
            is_in_library: local_id.is_some(),
            source: RadioSource::Tidal,
            reason: "TIDAL radio for this seed".to_string(),
            // Mix order is TIDAL's ranking.
            similarity_score: 1.0 - index as f64 / total * 0.5,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        });
    }
    out
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

pub fn new_lastfm_similar_cache() -> LastFmSimilarCache {
    Arc::new(Mutex::new(HashMap::new()))
}

fn lastfm_similar_cache_key(artist: &str, title: &str) -> LastFmSimilarCacheKey {
    LastFmSimilarCacheKey {
        artist: artist.trim().to_lowercase(),
        title: title.trim().to_lowercase(),
    }
}

async fn lastfm_similar_with_cache<F, Fut>(
    cache: Option<&LastFmSimilarCache>,
    artist: &str,
    title: &str,
    limit: usize,
    fetch: F,
) -> Result<Vec<crate::metadata::lastfm::LastFmSimilarTrack>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<crate::metadata::lastfm::LastFmSimilarTrack>>>,
{
    let Some(cache) = cache else {
        return fetch().await;
    };
    let key = lastfm_similar_cache_key(artist, title);
    let now = Instant::now();

    if let Ok(mut guard) = cache.lock()
        && let Some(entry) = guard.get(&key)
    {
        if now.duration_since(entry.fetched_at) <= LASTFM_SIMILAR_CACHE_TTL
            && entry.requested_limit >= limit
        {
            tracing::debug!(artist, title, limit, "lastfm similar cache hit");
            return Ok(entry.tracks.iter().take(limit).cloned().collect());
        }
        if now.duration_since(entry.fetched_at) > LASTFM_SIMILAR_CACHE_TTL {
            guard.remove(&key);
        }
    }

    let tracks: Vec<_> = fetch().await?.into_iter().take(limit).collect();
    if let Ok(mut guard) = cache.lock() {
        guard.insert(
            key,
            LastFmSimilarCacheEntry {
                fetched_at: now,
                requested_limit: limit,
                tracks: tracks.clone(),
            },
        );
    }
    Ok(tracks)
}

// ─── Public orchestrators ────────────────────────────────────────────────────

/// Build a Song Radio queue seeded from a single library track.
pub async fn orchestrate_song(
    db: &Database,
    lastfm: Option<&LastFmClient>,
    lastfm_similar_cache: Option<&LastFmSimilarCache>,
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
        let creativity = blend.creativity();
        let lib = crate::services::learning::radio_from_neighbors(
            db,
            seed_track_id,
            &excl,
            lib_target as i64,
            creativity,
        );
        if let Err(ref e) = lib {
            tracing::warn!(seed_track_id, error = %e, "orchestrate_song: library/embedding source errored");
        }
        lib.ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|n| {
                let reason = if !n.reason_tags.is_empty() {
                    format!(
                        "library · {} (sim {:.2})",
                        n.reason_tags[0], n.similarity_score
                    )
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
    //
    // A `None` client is not logged here. It used to be, at INFO, as "no API key
    // configured" - which this function has no way of knowing. `None` is just as
    // often a caller switching the source off on purpose: `home_suggestions`
    // does exactly that for every seed it fans out, so a single Home load wrote
    // eight INFO lines blaming a key that was configured and working. A
    // diagnostic that states a cause it did not check is worse than no
    // diagnostic. What actually happened is already on the funnel line below as
    // `lfm_count`, and whether a key exists is what `/api/lastfm/status`
    // answers.
    if lastfm.is_some() && seed_meta.artist_name.is_none() {
        tracing::info!(
            seed_track_id,
            "orchestrate_song: seed has no artist_name; Last.fm source skipped"
        );
    }
    let mut lastfm_results: Vec<RadioCandidate> = if let (Some(client), Some(artist)) =
        (lastfm, seed_meta.artist_name.as_deref())
    {
        let fetch_limit = lfm_target.max(20);
        let lfm = lastfm_similar_with_cache(
            lastfm_similar_cache,
            artist,
            &seed_meta.title,
            fetch_limit,
            || client.track_get_similar_with_artist_fallback(artist, &seed_meta.title, fetch_limit),
        )
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

    // Adventurous reaches one Last.fm hop further: stored Last.fm links of the
    // seed's learned neighbors, discounted. Familiar and Mixed stay with the
    // seed's own Last.fm matches.
    if blend == RadioBlend::Adventurous {
        lastfm_results.extend(lastfm_two_hop_candidates(db, seed_track_id, lfm_target));
    }

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
    // Both rank into a wider interim list so the hard artist cap below has
    // spare candidates to substitute when it rejects a slot.
    let interim_limit = limit.saturating_mul(2).max(limit.saturating_add(8));
    let mut rerank_counters = RerankCounters::default();
    let ranked = if flags.diversity_rerank_enabled {
        let primary_genres = primary_genres_for_candidates(db, &combined);
        diversity_rerank(
            combined,
            &profile_for_penalties,
            blend,
            interim_limit,
            &primary_genres,
            &taste.recent_track_ids,
            flags.source_quota_bonus_enabled,
            &mut rerank_counters,
        )
    } else {
        blend_interleave(combined, blend, interim_limit)
    };
    let recent_artists = recent_played_artist_names(db, ARTIST_HISTORY_MEMORY);
    let ordered = enforce_artist_diversity(ranked, &recent_artists, limit);

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
            // EXISTS, not COUNT(*): this runs on every radio request, and a
            // populated track_similarity table has hundreds of thousands of
            // rows — we only need the empty/non-empty bit.
            diag.engine_index_empty = conn
                .query_row(
                    "SELECT NOT EXISTS(SELECT 1 FROM track_similarity)",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(false);
            crate::services::radio_config::log_radio_diagnostics(conn, &diag)
        }) {
            // Diagnostics failures should never break a radio request — log and move on.
            tracing::warn!(seed_track_id, error = %err, "failed to log radio diagnostics");
        }
    }

    // Cold-start fallback: when embedding neighbors, Last.fm, and track_similarity
    // all return empty (e.g. newly-synced tracks not yet in the learning model),
    // surface other library tracks by the same artist so radio doesn't fail silently.
    let ordered = if ordered.is_empty() {
        let fallback = fallback_same_artist_tracks(db, seed_track_id, limit, exclude_track_ids);
        if fallback.is_empty() {
            tracing::warn!(
                seed_track_id,
                "orchestrate_song: all sources empty including same-artist fallback; returning empty queue"
            );
        } else {
            tracing::info!(
                seed_track_id,
                count = fallback.len(),
                "orchestrate_song: using same-artist fallback"
            );
        }
        fallback
    } else {
        ordered
    };

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
    lastfm_similar_cache: Option<&LastFmSimilarCache>,
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
        if let Ok(q) = orchestrate_song(
            db,
            lastfm,
            lastfm_similar_cache,
            *tid,
            blend,
            per_seed_limit,
            exclude_track_ids,
        )
        .await
        {
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
    lastfm_similar_cache: Option<&LastFmSimilarCache>,
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
        if let Ok(q) = orchestrate_song(
            db,
            lastfm,
            lastfm_similar_cache,
            *tid,
            blend,
            per_seed_limit,
            exclude_track_ids,
        )
        .await
        {
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
            title: artist_name
                .clone()
                .unwrap_or_else(|| format!("artist {seed_artist_id}")),
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
        let profile = crate::playback::player::build_session_taste_profile(conn, &seed_track)?;
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

/// Cold-start fallback: returns tracks by the same artist as the seed, randomly ordered.
/// Used when all three primary candidate sources (embedding neighbors, Last.fm, track_similarity)
/// return zero results — typically for newly-synced tracks not yet in the learning model.
fn fallback_same_artist_tracks(
    db: &Database,
    seed_track_id: i64,
    limit: usize,
    exclude_ids: &[i64],
) -> Vec<RadioCandidate> {
    let exclude_set: HashSet<i64> = exclude_ids
        .iter()
        .copied()
        .chain(std::iter::once(seed_track_id))
        .collect();
    let fetch_limit = (limit * 3).max(20) as i64;
    let rows = db.with_conn(move |conn| {
        let artist_id: Option<i64> = conn
            .query_row(
                "SELECT artist_id FROM tracks WHERE id = ?1",
                rusqlite::params![seed_track_id],
                |row| row.get(0),
            )
            .ok()
            .flatten();
        let Some(artist_id) = artist_id else {
            return Ok(Vec::new());
        };
        let mut stmt = conn.prepare(
            "SELECT t.id, t.title, ar.name, al.title, al.artwork_url, t.duration_ms
             FROM tracks t
             LEFT JOIN artists ar ON ar.id = t.artist_id
             LEFT JOIN albums al ON al.id = t.album_id
             WHERE t.artist_id = ?1 AND t.id != ?2
             ORDER BY RANDOM()
             LIMIT ?3",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params![artist_id, seed_track_id, fetch_limit],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    });
    let rows = match rows {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    rows.into_iter()
        .filter(|(id, ..)| !exclude_set.contains(id))
        .take(limit)
        .map(|(id, title, artist, album, art, dur)| RadioCandidate {
            track_id: id,
            tidal_track_id: None,
            title,
            artist_name: artist.unwrap_or_default(),
            album_title: album,
            artwork_url: art,
            duration_ms: dur,
            isrc: None,
            is_in_library: true,
            source: RadioSource::Library,
            reason: "same artist · fallback".to_string(),
            similarity_score: 0.5,
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        })
        .collect()
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

/// A learned neighbor's own Last.fm links count at this fraction of its score.
const LASTFM_TWO_HOP_WEIGHT: f64 = 0.65;
/// Learned neighbors whose Last.fm links are followed.
const LASTFM_TWO_HOP_SEEDS: i64 = 5;
/// Weakest 2-hop link worth queueing.
const LASTFM_TWO_HOP_MIN_SCORE: f64 = 0.10;

/// Two hops through Last.fm: tracks Last.fm linked to the seed's learned
/// neighbors (stored sightings, no network call), as Last.fm-lane candidates.
fn lastfm_two_hop_candidates(
    db: &Database,
    seed_track_id: i64,
    limit: usize,
) -> Vec<RadioCandidate> {
    let rows = db.with_conn(|conn| {
        let Some(model) = crate::db::queries::get_selected_discovery_embedding_model(conn)? else {
            return Ok(Vec::new());
        };
        let seeds = crate::db::queries::get_track_neighbors(
            conn,
            model.id,
            seed_track_id,
            LASTFM_TWO_HOP_SEEDS,
            &[],
        )?
        .into_iter()
        .map(|row| {
            (
                row.track_id,
                LASTFM_TWO_HOP_WEIGHT * row.score.clamp(0.0, 1.0),
            )
        })
        .collect::<Vec<_>>();
        crate::db::queries::get_sighted_external_candidates(
            conn,
            &seeds,
            LASTFM_TWO_HOP_MIN_SCORE,
            limit.max(1) as i64,
        )
    });
    rows.unwrap_or_default()
        .into_iter()
        .map(|row| RadioCandidate {
            track_id: 0,
            tidal_track_id: row.tidal_id,
            title: row.title,
            artist_name: row.artist_name,
            album_title: None,
            artwork_url: None,
            duration_ms: row.duration_ms,
            isrc: None,
            is_in_library: false,
            source: RadioSource::Lastfm,
            reason: format!("Last.fm 2-hop {:.2}", row.score),
            similarity_score: row.score.clamp(0.0, 1.0),
            confidence: None,
            candidate_in_degree_percentile: None,
            support_count: None,
            primary_reason: None,
        })
        .collect()
}

/// Most recent artists from listen history (chronological, oldest first),
/// bounded by ARTIST_HISTORY_MAX_AGE_HOURS so stale sessions do not leak in.
fn recent_played_artist_names(db: &Database, limit: usize) -> Vec<String> {
    let rows: Vec<String> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(ar.name, '')
                 FROM listen_history lh
                 JOIN tracks t ON t.id = lh.track_id
                 LEFT JOIN artists ar ON ar.id = t.artist_id
                 WHERE julianday(lh.started_at) >= julianday('now', printf('-%d hours', ?1))
                 ORDER BY julianday(lh.started_at) DESC
                 LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![ARTIST_HISTORY_MAX_AGE_HOURS, limit as i64],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .unwrap_or_default();
    rows.into_iter().rev().collect()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod radio_phase2_tests;

#[cfg(test)]
mod radio_diagnostic_harness;
