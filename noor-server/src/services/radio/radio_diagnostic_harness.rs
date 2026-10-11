//! # Radio quality diagnostic harness
//!
//! Permanent debug tool, not dead code. Run against the live
//! database to inspect what `orchestrate_song` actually produces for
//! a given seed track and why.
//!
//! ## Usage
//!
//! ```bash
//! NOOR_SEED=1634 NOOR_DB=<local-db> \
//!     cargo test -p noor-server radio_diagnostic_for_seed \
//!     -- --ignored --nocapture
//! ```
//!
//! Defaults: `NOOR_SEED=1634` (Doja Cat — "Go To Town" — Amala, the
//! seed that surfaced the Phase 2a affinity-zeroing regression). The DB
//! path uses the same shared `NOOR_DB` / `NOOR_DATA_DIR` / dev / portable
//! resolution as the app. Override with `NOOR_DB` if needed.
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

fn diagnostic_db_path() -> String {
    crate::paths::resolve_db_path_from_env()
        .to_string_lossy()
        .into_owned()
}

fn track_genres(db: &Database, track_id: i64) -> HashSet<i64> {
    if track_id <= 0 {
        return HashSet::new();
    }
    db.with_conn(|conn| {
        let mut stmt = conn.prepare("SELECT genre_id FROM track_genres WHERE track_id = ?1")?;
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
    let db_path = diagnostic_db_path();
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
        let creativity = blend.creativity();
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
    eprintln!(
        "\nLibrary source: {} candidates (target {})",
        library_results.len(),
        lib_target
    );

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
    eprintln!(
        "Engine source: {} candidates (target {})",
        engine_results.len(),
        eng_target
    );

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
        "#",
        "src",
        "tid",
        "pre_aff",
        "post_aff",
        "final",
        "aff_m",
        "gen_m",
        "jacc",
        "g_olp",
        "artist",
        "title"
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
        let post_aff = post_affinity_lookup.get(&key).copied().unwrap_or(f64::NAN);
        let aff_m = if pre > 0.0 { post_aff / pre } else { f64::NAN };
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
            RadioSource::Tidal => "tidal",
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
        if let Some(pre) = pre_affinity.get(&key).copied()
            && cand.similarity_score > pre * 1.05
            && cand.track_id > 0
        {
            let cg = track_genres(&db, cand.track_id);
            if !cg.is_empty() && cg.intersection(&seed_genres).count() == 0 {
                h2_promoted += 1;
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
