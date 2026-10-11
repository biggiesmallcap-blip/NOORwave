//! Track similarity, neighbours, trained models and discovery learning state.

use super::*;

// ─── Track Similarity (Similar Radio) ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackSimilarityResult {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub similarity_score: f64,
    pub co_listen_score: f64,
    pub co_album_score: f64,
    pub co_artist_score: f64,
    pub genre_proximity: f64,
}

/// Genres with more members than this are too broad to propose candidate pairs
/// (every pop track "shares a genre" with every other); they still count toward
/// genre_proximity for pairs found another way.
pub(super) const GENRE_CANDIDATE_MAX_MEMBERS: i64 = 1_000;
/// Genre-only candidates kept per track, strongest shared rarity first. Replaces
/// a global LIMIT that filled up with the lowest track ids.
pub(super) const GENRE_CANDIDATES_PER_TRACK: i64 = 50;
/// Co-listen window and lookback for the co_listen component.
pub(super) const CO_LISTEN_WINDOW_MINUTES: f64 = 30.0;
pub(super) const CO_LISTEN_LOOKBACK_DAYS: i64 = 90;

/// Build pre-computed similarity pairs for the radio feature, in four stages:
///   1. per-(track, genre) weights (genre rarity x tag confidence);
///   2. co-listened pairs from listen_history, recency-weighted PPMI;
///   3. candidate pairs: same album, same artist (artists with <= 100 tracks),
///      every co-listened pair, and each track's strongest genre partners;
///   4. per-component scores and the weighted total.
pub fn compute_track_similarity(conn: &Connection) -> Result<usize> {
    // Do the expensive work in temporary tables so the background rebuild does
    // not hold SQLite's single writer slot while playback is trying to update
    // playback_state. The real table is swapped in one short transaction at
    // the end.
    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _track_similarity_build;
        CREATE TEMP TABLE _track_similarity_build (
            track_a INTEGER NOT NULL,
            track_b INTEGER NOT NULL,
            similarity_score REAL NOT NULL DEFAULT 0,
            co_listen_score REAL DEFAULT 0,
            co_album_score REAL DEFAULT 0,
            co_artist_score REAL DEFAULT 0,
            genre_proximity REAL DEFAULT 0,
            duration_proximity REAL DEFAULT 0,
            era_proximity REAL DEFAULT 0,
            computed_at TEXT DEFAULT (datetime('now')),
            PRIMARY KEY (track_a, track_b),
            CHECK (track_a < track_b)
        );
    ",
    )?;

    // -- Stage 1: per-track genre weights --

    // Per-(track, genre) weight = genre rarity (IDF) x tag confidence.
    //
    //   1. IDF: weight each genre by ln(total_tracks / members) so a rare genre (a
    //      tight cluster) dominates a broad one (almost no signal - knowing two
    //      tracks are both "Hip-Hop" in a hip-hop-heavy library says little). A
    //      genre covering the whole library weighs 0.
    //   2. Confidence: scale by track_genres.confidence, the scorer's per-tag trust
    //      (source strength x folksonomy vote count). A weakly-attested tag - a
    //      single-vote MusicBrainz "jazz" bleeding onto an emo-rap track - then
    //      contributes little, while a well-attested genre counts fully. Clamped to
    //      1.0 so an unusually high accumulated score can't let one genre dominate
    //      by raw magnitude.
    //
    // Computed in Rust and staged in a temp table because the bundled SQLite has no ln().
    let total_tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |row| row.get(0))?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS _track_genre_weight;
         CREATE TEMP TABLE _track_genre_weight (
             track_id INTEGER NOT NULL,
             genre_id INTEGER NOT NULL,
             weight REAL NOT NULL,
             PRIMARY KEY (track_id, genre_id)
         );",
    )?;
    if total_tracks > 0 {
        // genre_id -> IDF weight
        let mut idf: HashMap<i64, f64> = HashMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT genre_id, COUNT(DISTINCT track_id) FROM track_genres GROUP BY genre_id",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            for (genre_id, members) in rows {
                let weight = if members > 0 {
                    (total_tracks as f64 / members as f64).ln().max(0.0)
                } else {
                    0.0
                };
                idf.insert(genre_id, weight);
            }
        }

        // weight each (track, genre) by IDF x clamped confidence.
        let tags: Vec<(i64, i64, f64)> = {
            let mut stmt =
                conn.prepare("SELECT track_id, genre_id, confidence FROM track_genres")?;
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
        };
        let mut insert = conn.prepare(
            "INSERT OR IGNORE INTO _track_genre_weight (track_id, genre_id, weight)
             VALUES (?1, ?2, ?3)",
        )?;
        for (track_id, genre_id, confidence) in tags {
            let base = idf.get(&genre_id).copied().unwrap_or(0.0);
            let trust = confidence.clamp(0.0, 1.0);
            insert.execute(params![track_id, genre_id, base * trust])?;
        }
    }

    // -- Stage 2: co-listened pairs --

    // listen_history.started_at is RFC 3339 ("2026-10-07T03:54:43...+00:00").
    // Compared as text with datetime() output ("2026-10-07 04:24:43") it fails
    // on the same day, which left every co_listen_score at 0, so all time math
    // goes through julianday(). Early skips (under 30 s, not completed) are not
    // co-listens. Each listen counts by recency: 1.0 now, fading to 0.2 at the
    // end of the lookback window.
    conn.execute_batch(&format!(
        "
        DROP TABLE IF EXISTS _listen_jd;
        CREATE TEMP TABLE _listen_jd AS
        SELECT track_id, julianday(started_at) AS jd,
               MAX(0.2, 1.0 - (julianday('now') - julianday(started_at)) / {lookback}.0) AS w
        FROM listen_history
        WHERE track_id IS NOT NULL
          AND julianday(started_at) >= julianday('now', '-{lookback} days')
          AND (completed = 1 OR COALESCE(duration_listened_ms, 0) >= 30000);
        CREATE INDEX _listen_jd_idx ON _listen_jd(jd);

        DROP TABLE IF EXISTS _co_listen;
        CREATE TEMP TABLE _co_listen (
            ta INTEGER NOT NULL,
            tb INTEGER NOT NULL,
            n REAL NOT NULL,
            score REAL NOT NULL DEFAULT 0,
            PRIMARY KEY (ta, tb)
        );
        INSERT INTO _co_listen (ta, tb, n)
        SELECT MIN(a.track_id, b.track_id), MAX(a.track_id, b.track_id), SUM(MIN(a.w, b.w))
        FROM _listen_jd a
        JOIN _listen_jd b
            ON b.track_id != a.track_id
           AND b.jd BETWEEN a.jd AND a.jd + {window:.1} / 1440.0
        GROUP BY 1, 2
        HAVING COUNT(*) >= 2;
        ",
        lookback = CO_LISTEN_LOOKBACK_DAYS,
        window = CO_LISTEN_WINDOW_MINUTES,
    ))?;

    // Positive PMI times log count: a pair heard together more often than the
    // two tracks' own play counts predict scores high; two popular tracks that
    // merely co-occur score low. The log count keeps a single lucky pairing
    // from topping the list. Normalized so the strongest pair scores 1.0. Done
    // in Rust; the bundled SQLite has no ln().
    {
        let marginals: HashMap<i64, f64> = {
            let mut stmt =
                conn.prepare("SELECT track_id, SUM(w) FROM _listen_jd GROUP BY track_id")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<HashMap<_, _>, _>>()?
        };
        let total: f64 = marginals.values().sum();
        let pairs: Vec<(i64, i64, f64)> = {
            let mut stmt = conn.prepare("SELECT ta, tb, n FROM _co_listen")?;
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let raw = pairs
            .into_iter()
            .map(|(ta, tb, n)| {
                let wa = marginals.get(&ta).copied().unwrap_or(0.0);
                let wb = marginals.get(&tb).copied().unwrap_or(0.0);
                let pmi = if wa > 0.0 && wb > 0.0 && total > 0.0 {
                    (n * total / (wa * wb)).ln().max(0.0)
                } else {
                    0.0
                };
                (ta, tb, pmi * n.ln_1p())
            })
            .collect::<Vec<_>>();
        let max_raw = raw.iter().map(|pair| pair.2).fold(0.0f64, f64::max);
        if max_raw > 0.0 {
            let mut update =
                conn.prepare("UPDATE _co_listen SET score = ?3 WHERE ta = ?1 AND tb = ?2")?;
            for (ta, tb, value) in raw {
                update.execute(params![ta, tb, value / max_raw])?;
            }
        }
    }
    conn.execute_batch("DROP TABLE IF EXISTS _listen_jd;")?;

    // -- Stage 3: candidate pairs --
    // Each INSERT must satisfy CHECK (track_a < track_b).

    conn.execute_batch(
        "
        -- 3a: Same-album pairs (all combinations, not just min/max)
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.album_id = a.album_id AND b.id > a.id
        WHERE a.album_id IS NOT NULL;

        -- 3b: Same-artist pairs (cap at artists with <=100 tracks)
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT a.id, b.id
        FROM tracks a
        JOIN tracks b ON b.artist_id = a.artist_id AND b.id > a.id
        WHERE a.artist_id IN (
            SELECT artist_id FROM tracks GROUP BY artist_id HAVING COUNT(*) <= 100
        );

        -- 3c: Co-listened pairs, whatever their artist or album. Before this
        -- they could only be scored if they also shared one.
        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT cl.ta, cl.tb FROM _co_listen cl
        WHERE EXISTS (SELECT 1 FROM tracks WHERE id = cl.ta)
          AND EXISTS (SELECT 1 FROM tracks WHERE id = cl.tb);
    ",
    )?;

    // 3d: Genre pairs: each track's strongest partners by shared genre rarity,
    // from genres narrow enough to tell tracks apart.
    conn.execute_batch(&format!(
        "
        DROP TABLE IF EXISTS _genre_candidates;
        CREATE TEMP TABLE _genre_candidates AS
        SELECT a.track_id AS ta, b.track_id AS tb, SUM(MIN(a.weight, b.weight)) AS shared
        FROM _track_genre_weight a
        JOIN _track_genre_weight b ON b.genre_id = a.genre_id AND b.track_id > a.track_id
        WHERE a.genre_id IN (
            SELECT genre_id FROM _track_genre_weight
            GROUP BY genre_id HAVING COUNT(*) <= {max_members}
        )
        GROUP BY a.track_id, b.track_id
        HAVING shared > 0;

        INSERT OR IGNORE INTO _track_similarity_build (track_a, track_b)
        SELECT MIN(t, o), MAX(t, o)
        FROM (
            SELECT t, o, ROW_NUMBER() OVER (PARTITION BY t ORDER BY shared DESC, o) AS rn
            FROM (
                SELECT ta AS t, tb AS o, shared FROM _genre_candidates
                UNION ALL
                SELECT tb AS t, ta AS o, shared FROM _genre_candidates
            )
        )
        WHERE rn <= {per_track};
        DROP TABLE _genre_candidates;
        ",
        max_members = GENRE_CANDIDATE_MAX_MEMBERS,
        per_track = GENRE_CANDIDATES_PER_TRACK,
    ))?;

    // Track -> release year, for era_proximity. Albums table holds the year.
    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _track_year;
        CREATE TEMP TABLE _track_year AS
        SELECT t.id AS track_id, al.year AS year
        FROM tracks t
        JOIN albums al ON al.id = t.album_id
        WHERE al.year IS NOT NULL;
        CREATE INDEX _track_year_idx ON _track_year(track_id);
    ",
    )?;

    // -- Stage 4: score each component --

    // co_album: 1.0 if same album
    conn.execute(
        "
        UPDATE _track_similarity_build SET co_album_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a
              AND b.id = _track_similarity_build.track_b
              AND a.album_id IS NOT NULL
              AND a.album_id = b.album_id
        )
    ",
        [],
    )?;

    // co_artist: 1.0 if same artist
    conn.execute(
        "
        UPDATE _track_similarity_build SET co_artist_score = 1.0
        WHERE EXISTS (
            SELECT 1 FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a
              AND b.id = _track_similarity_build.track_b
              AND a.artist_id IS NOT NULL
              AND a.artist_id = b.artist_id
        )
    ",
        [],
    )?;

    // genre_proximity: summed rarity of the genres both tracks hold (each counted
    // as strongly as the weaker side believes it, so a damped mis-tag on one
    // track can't inflate the match), normalized so the strongest pair scores
    // 1.0. Computed per candidate instead of over every genre pair.
    conn.execute(
        "UPDATE _track_similarity_build SET genre_proximity = COALESCE((
            SELECT SUM(MIN(a.weight, b.weight))
            FROM _track_genre_weight a
            JOIN _track_genre_weight b
              ON b.track_id = _track_similarity_build.track_b AND b.genre_id = a.genre_id
            WHERE a.track_id = _track_similarity_build.track_a
        ), 0)",
        [],
    )?;
    let max_genre: f64 = conn.query_row(
        "SELECT COALESCE(MAX(genre_proximity), 0) FROM _track_similarity_build",
        [],
        |row| row.get(0),
    )?;
    if max_genre > 0.0 {
        conn.execute(
            "UPDATE _track_similarity_build SET genre_proximity = genre_proximity / ?1",
            params![max_genre],
        )?;
    }

    // duration_proximity: 1 - |dur_a - dur_b| / 180s, clamped 0-1
    conn.execute(
        "
        UPDATE _track_similarity_build SET duration_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(a.duration_ms - b.duration_ms) AS REAL) / 180000.0, 1.0)
            FROM tracks a, tracks b
            WHERE a.id = _track_similarity_build.track_a AND b.id = _track_similarity_build.track_b
              AND a.duration_ms IS NOT NULL AND b.duration_ms IS NOT NULL
        ), 0)
    ",
        [],
    )?;

    // co_listen: log-scaled co-occurrence (Stage 2)
    conn.execute(
        "UPDATE _track_similarity_build SET co_listen_score = COALESCE((
            SELECT cl.score FROM _co_listen cl
            WHERE cl.ta = _track_similarity_build.track_a AND cl.tb = _track_similarity_build.track_b
        ), 0)",
        [],
    )?;

    // era_proximity: 1 - |year_a - year_b| / 25, clamped 0-1. Zero when either year is unknown.
    conn.execute(
        "
        UPDATE _track_similarity_build SET era_proximity = COALESCE((
            SELECT 1.0 - MIN(CAST(ABS(ya.year - yb.year) AS REAL) / 25.0, 1.0)
            FROM _track_year ya, _track_year yb
            WHERE ya.track_id = _track_similarity_build.track_a
              AND yb.track_id = _track_similarity_build.track_b
        ), 0)
    ",
        [],
    )?;

    // Final weighted score. era_proximity replaces some duration_proximity weight
    // because era is a stronger taste signal than song length.
    conn.execute(
        "
        UPDATE _track_similarity_build SET similarity_score =
            co_listen_score    * 0.30 +
            co_album_score     * 0.20 +
            co_artist_score    * 0.20 +
            genre_proximity    * 0.15 +
            era_proximity      * 0.10 +
            duration_proximity * 0.05
    ",
        [],
    )?;

    conn.execute_batch(
        "
        DROP TABLE IF EXISTS _co_listen;
        DROP TABLE IF EXISTS _track_year;
        DROP TABLE IF EXISTS _track_genre_weight;
    ",
    )?;

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM _track_similarity_build", [], |row| {
        row.get(0)
    })?;

    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM track_similarity", [])?;
    tx.execute(
        "
        INSERT INTO track_similarity (
            track_a,
            track_b,
            similarity_score,
            co_listen_score,
            co_album_score,
            co_artist_score,
            genre_proximity,
            duration_proximity,
            era_proximity,
            computed_at
        )
        SELECT
            track_a,
            track_b,
            similarity_score,
            co_listen_score,
            co_album_score,
            co_artist_score,
            genre_proximity,
            duration_proximity,
            era_proximity,
            computed_at
        FROM _track_similarity_build
        ",
        [],
    )?;
    tx.commit()?;
    conn.execute_batch("DROP TABLE IF EXISTS _track_similarity_build;")?;
    Ok(count as usize)
}

/// Get similar tracks to a given track, ordered by similarity.
/// Returns up to `limit` tracks with similarity scores.
pub fn get_similar_tracks(
    conn: &Connection,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<TrackSimilarityResult>> {
    // For simplicity, handle exclude via post-filtering (limit is small, ~20-50)
    let sql = "SELECT t.id, t.title, a.name, al.title, al.artwork_url,
                      t.duration_ms, t.best_quality,
                      ts.similarity_score, ts.co_listen_score, ts.co_album_score,
                      ts.co_artist_score, ts.genre_proximity
               FROM track_similarity ts
               JOIN tracks t ON t.id = CASE
                   WHEN ts.track_a = ?1 THEN ts.track_b
                   ELSE ts.track_a
               END
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE (ts.track_a = ?1 OR ts.track_b = ?1)
                 AND t.id != ?1
               ORDER BY ts.similarity_score DESC
               LIMIT ?2";

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![track_id, limit], |row| {
        Ok(TrackSimilarityResult {
            track_id: row.get(0)?,
            title: row.get(1)?,
            artist_name: row.get(2)?,
            album_title: row.get(3)?,
            artwork_url: row.get(4)?,
            duration_ms: row.get(5)?,
            best_quality: row.get(6)?,
            similarity_score: row.get(7)?,
            co_listen_score: row.get(8)?,
            co_album_score: row.get(9)?,
            co_artist_score: row.get(10)?,
            genre_proximity: row.get(11)?,
        })
    })?;

    let mut results: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;

    // Post-filter excluded IDs
    if !exclude_ids.is_empty() {
        let exclude_set: HashSet<i64> = exclude_ids.iter().copied().collect();
        results.retain(|r| !exclude_set.contains(&r.track_id));
    }

    Ok(results)
}

/// Get similarity computation status
pub fn get_similarity_computed_at(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT MAX(computed_at) FROM track_similarity", [], |row| {
            row.get(0)
        })
        .optional()?)
}

/// Row count of the precomputed `track_similarity` index.
pub fn count_track_similarity(conn: &Connection) -> Result<i64> {
    Ok(
        conn.query_row("SELECT COUNT(*) FROM track_similarity", [], |row| {
            row.get(0)
        })?,
    )
}

/// Start timestamp of the last successful radio similarity rebuild. Recorded in
/// `server_config` independently of row count — a valid library can produce
/// zero similarity pairs, so an empty table is not the same as "never built".
pub fn get_radio_similarity_built_at(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'radio_similarity_built_at'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// True when a discovery training run is in progress. The radio similarity
/// rebuild must not run alongside training: training writes heavily through the
/// shared connection, and the rebuild's long write transaction would starve it
/// past the busy timeout and fail the run.
pub fn is_discovery_training_running(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM training_runs WHERE status = 'running')",
        [],
        |row| row.get(0),
    )?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingTrackRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub source: String,
    pub play_count: i32,
    pub is_favorite: bool,
    pub playlist_memberships: i64,
    pub genre_paths: Vec<String>,
    // DSP features (None if not yet analyzed)
    pub bpm: Option<f64>,
    pub energy: Option<f64>,
    pub camelot_key: Option<String>,
    pub danceability: Option<f64>,
    pub beat_strength: Option<f64>,
    pub loudness_lufs: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingNeighborRow {
    pub track_id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub best_quality: Option<String>,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub support_transition: f64,
    pub support_colisten: f64,
    pub support_structure: f64,
    pub support_metadata: f64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
    pub primary_reason: Option<String>,
}

// Trainer write payload. Replaces the 9-tuple that replace_track_neighbors used
// to take — at 16 fields, named struct fields are necessary for any chance at
// not mixing up arguments.
#[derive(Debug, Clone)]
pub struct NeighborWriteRow {
    pub track_id: i64,
    pub neighbor_track_id: i64,
    pub rank: i32,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
    pub primary_reason: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub support_transition: f64,
    pub support_colisten: f64,
    pub support_structure: f64,
    pub support_metadata: f64,
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTrackCandidateRow {
    pub id: i64,
    pub tidal_id: Option<i64>,
    pub mbid: Option<String>,
    pub dedupe_key: String,
    pub title: String,
    pub artist_name: String,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub expires_at: String,
    pub updated_at: String,
    pub source_tags_json: Option<String>,
    pub resolved_track_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedLastfmExternalSightingRow {
    pub seed_track_id: i64,
    pub resolved_track_id: i64,
    pub similarity: f64,
    pub source_payload_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTidalResolutionCandidateRow {
    pub id: i64,
    pub title: String,
    pub artist_name: String,
    pub duration_ms: Option<i64>,
    pub sighting_count: i64,
    pub max_similarity: Option<f64>,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTidalSimilarSeedRow {
    pub track_id: i64,
    pub artist_tidal_id: i64,
}

#[derive(Debug, Clone)]
pub(super) struct ExternalCandidateFallbackIdentity {
    pub(super) normalized_artist_name: String,
    pub(super) normalized_title: String,
    pub(super) duration_bucket: i64,
}

#[derive(Debug, Clone)]
pub struct ExternalTrackCandidateUpsert {
    pub tidal_id: Option<i64>,
    pub mbid: Option<String>,
    pub dedupe_key: String,
    pub title: String,
    pub artist_name: String,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateTidalResolution {
    pub tidal_id: i64,
    pub genre_tags_json: Option<String>,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateSightingUpsert {
    pub candidate_id: i64,
    pub seed_track_id: i64,
    pub source: String,
    pub source_payload_json: Option<String>,
    pub similarity: Option<f64>,
    pub expires_at: String,
}

#[derive(Debug, Clone)]
pub struct ExternalCandidateNeighborWriteRow {
    pub candidate_id: i64,
    pub rank: i32,
    pub score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalCandidateNeighborRow {
    pub candidate_id: i64,
    pub tidal_id: Option<i64>,
    pub title: String,
    pub artist_name: String,
    pub duration_ms: Option<i64>,
    pub rank: i32,
    pub score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEmbeddingRow {
    pub track_id: i64,
    pub vector_blob: Vec<u8>,
    pub l2_norm: f64,
}

#[allow(dead_code)]
pub fn upsert_embedding_model(
    conn: &Connection,
    model_key: &str,
    family: &str,
    dimension: i32,
    status: &str,
    config_json: Option<&str>,
) -> Result<EmbeddingModel> {
    conn.execute(
        "INSERT INTO embedding_models (model_key, family, dimension, status, config_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(model_key) DO UPDATE SET
             family = excluded.family,
             dimension = excluded.dimension,
             status = excluded.status,
             config_json = excluded.config_json",
        params![model_key, family, dimension, status, config_json],
    )?;

    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models WHERE model_key = ?1",
        params![model_key],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn create_embedding_model(
    conn: &Connection,
    model_key: &str,
    family: &str,
    dimension: i32,
    status: &str,
    config_json: Option<&str>,
) -> Result<EmbeddingModel> {
    conn.execute(
        "INSERT INTO embedding_models (model_key, family, dimension, status, config_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![model_key, family, dimension, status, config_json],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models WHERE id = ?1",
        params![id],
        |row| {
            Ok(EmbeddingModel {
                id: row.get(0)?,
                model_key: row.get(1)?,
                family: row.get(2)?,
                dimension: row.get(3)?,
                status: row.get(4)?,
                is_active: row.get(5)?,
                trained_at: row.get(6)?,
                config_json: row.get(7)?,
                metrics_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn update_embedding_model_metrics(
    conn: &Connection,
    model_id: i64,
    status: &str,
    metrics_json: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE embedding_models
         SET status = ?2, metrics_json = ?3, trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id, status, metrics_json],
    )?;
    Ok(())
}

pub fn fail_embedding_model(conn: &Connection, model_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE embedding_models
         SET status = 'failed'
         WHERE id = ?1",
        params![model_id],
    )?;
    Ok(())
}

pub(super) fn read_embedding_model(row: &Row<'_>) -> rusqlite::Result<EmbeddingModel> {
    Ok(EmbeddingModel {
        id: row.get(0)?,
        model_key: row.get(1)?,
        family: row.get(2)?,
        dimension: row.get(3)?,
        status: row.get(4)?,
        is_active: row.get(5)?,
        trained_at: row.get(6)?,
        config_json: row.get(7)?,
        metrics_json: row.get(8)?,
        created_at: row.get(9)?,
    })
}

pub fn get_ready_embedding_model_for_family(
    conn: &Connection,
    family: &str,
) -> Result<Option<EmbeddingModel>> {
    conn.query_row(
        "SELECT id, model_key, family, dimension, status, is_active, trained_at, config_json, metrics_json, created_at
         FROM embedding_models
         WHERE family = ?1 AND status = 'ready'
         ORDER BY is_active DESC, trained_at DESC, id DESC
         LIMIT 1",
        params![family],
        read_embedding_model,
    )
    .optional()
    .map_err(Into::into)
}

pub fn discovery_model_family_for_engine(engine: &str) -> &'static str {
    match engine.trim().to_ascii_lowercase().as_str() {
        DISCOVERY_ENGINE_V1 => DISCOVERY_ENGINE_V1_FAMILY,
        _ => DISCOVERY_ENGINE_V2_FAMILY,
    }
}

pub fn selected_discovery_engine(conn: &Connection) -> Result<String> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'discovery_engine'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let normalized = match raw.as_deref().map(str::trim) {
        Some(DISCOVERY_ENGINE_V1) => DISCOVERY_ENGINE_V1,
        _ => DISCOVERY_ENGINE_V2,
    };
    Ok(normalized.to_string())
}

pub fn get_selected_discovery_embedding_model(conn: &Connection) -> Result<Option<EmbeddingModel>> {
    let engine = selected_discovery_engine(conn)?;
    get_ready_embedding_model_for_family(conn, discovery_model_family_for_engine(&engine))
}

pub fn deactivate_embedding_models(conn: &Connection) -> Result<()> {
    conn.execute("UPDATE embedding_models SET is_active = 0", [])?;
    Ok(())
}

pub fn activate_embedding_model(conn: &Connection, model_id: i64) -> Result<()> {
    deactivate_embedding_models(conn)?;
    conn.execute(
        "UPDATE embedding_models SET is_active = 1, status = 'ready', trained_at = datetime('now')
         WHERE id = ?1",
        params![model_id],
    )?;
    Ok(())
}

/// Retired models kept alongside the active one, so a bad retrain can be rolled
/// back by reactivating the previous model without retraining from scratch.
pub const EMBEDDING_MODELS_KEPT: usize = 1;
/// Rows deleted per statement when pruning. A single unbounded DELETE over the
/// retired models is a multi-GB WAL write and minutes of exclusive lock on a
/// real library (measured: 16M rows, 3.7GB WAL and still going), which is not
/// something a background repair may do to a running app. Each batch also holds
/// the shared connection: 20k rows held it for about a second, which made
/// volume and transport controls lag, so batches stay small.
pub(super) const NEIGHBOR_PRUNE_BATCH: usize = 2_000;

/// Make sure the prune can seek by model instead of scanning.
///
/// `track_neighbors` is keyed (track_id, neighbor_track_id, model_id) and its
/// secondary indexes lead with the first two, so selecting by model alone scans
/// the whole table. Measured on an 18.4M-row table that capped the prune at ~4k
/// rows/sec - about an hour of scanning to clear one library.
///
/// This is deliberately NOT left to the schema migration that also creates it.
/// `run_migrations` decides what to apply by COUNTING rows in `_migrations`, so
/// on any database whose count already exceeds the new migration's position the
/// migration is silently treated as applied and never runs - which is the case
/// on existing installs. Creating it here keeps the prune correct regardless of
/// migration bookkeeping, and costs one no-op check once the index exists.
pub fn ensure_track_neighbors_model_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_track_neighbors_model ON track_neighbors(model_id);",
    )?;
    Ok(())
}

/// Model ids that are neither active nor within the rollback window.
pub fn retired_embedding_model_ids(conn: &Connection, keep: usize) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM embedding_models
         WHERE is_active = 0
           AND id NOT IN (
               SELECT id FROM embedding_models
               WHERE is_active = 0
               ORDER BY COALESCE(trained_at, created_at) DESC, id DESC
               LIMIT ?1
           )
         ORDER BY id",
    )?;
    let ids = stmt
        .query_map(params![keep as i64], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Delete up to `NEIGHBOR_PRUNE_BATCH` neighbour rows belonging to retired
/// models. Returns how many rows went; 0 means the neighbour table is clean.
///
/// Callers loop until this returns 0, yielding between batches. Training only
/// ever runs on user request, but nothing has ever deleted the previous model's
/// rows, so a library that has been retrained a dozen times carries millions of
/// dead neighbours (the bulk of the database file).
pub fn prune_retired_model_neighbors_batch(conn: &Connection, keep: usize) -> Result<usize> {
    let retired = retired_embedding_model_ids(conn, keep)?;
    if retired.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; retired.len()].join(",");
    let sql = format!(
        "DELETE FROM track_neighbors WHERE rowid IN (
             SELECT rowid FROM track_neighbors WHERE model_id IN ({placeholders}) LIMIT ?
         )"
    );
    let mut bound: Vec<i64> = retired.clone();
    bound.push(NEIGHBOR_PRUNE_BATCH as i64);
    let deleted = conn.execute(&sql, rusqlite::params_from_iter(bound.iter()))?;
    Ok(deleted)
}

/// One-shot prune of every retired model, used by the Compact action.
///
/// Deleting rows one batch at a time is throttled by index maintenance, not by
/// finding the rows: each deleted neighbour updates six secondary indexes, which
/// measured out at ~5k rows/sec even with a model_id index in place - about an
/// hour to clear a real backlog. This path drops the secondary indexes first,
/// deletes in one pass, then rebuilds them from the definitions it captured, so
/// the index work happens once over the surviving rows instead of once per
/// deleted row.
///
/// The whole thing runs in one transaction. SQLite DDL is transactional, so a
/// crash mid-prune rolls back to a database that still has all of its indexes
/// rather than a silently unindexed one. VACUUM is deliberately left to the
/// caller - it cannot run inside a transaction.
///
/// Only for the foreground Compact action, which already holds the connection
/// and warns the user. Background callers want `prune_retired_model_neighbors_batch`.
pub fn prune_retired_models_bulk(conn: &Connection, keep: usize) -> Result<usize> {
    let retired = retired_embedding_model_ids(conn, keep)?;
    if retired.is_empty() {
        return Ok(0);
    }

    // Capture the real definitions rather than hardcoding them, so this keeps
    // working when the schema adds an index. `sql IS NULL` skips the implicit
    // primary-key index, which must not be dropped.
    let index_sql: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT sql FROM sqlite_master
             WHERE type = 'index' AND tbl_name = 'track_neighbors' AND sql IS NOT NULL",
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let index_names: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'index' AND tbl_name = 'track_neighbors' AND sql IS NOT NULL",
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };

    let placeholders = vec!["?"; retired.len()].join(",");

    conn.execute_batch("BEGIN IMMEDIATE;")?;
    let result = (|| -> Result<usize> {
        for name in &index_names {
            conn.execute_batch(&format!("DROP INDEX IF EXISTS \"{name}\";"))?;
        }
        let deleted = conn.execute(
            &format!("DELETE FROM track_neighbors WHERE model_id IN ({placeholders})"),
            rusqlite::params_from_iter(retired.iter()),
        )?;
        for sql in &index_sql {
            conn.execute_batch(&format!("{sql};"))?;
        }
        // Cascades to track_embeddings for the same models.
        conn.execute(
            &format!("DELETE FROM embedding_models WHERE id IN ({placeholders})"),
            rusqlite::params_from_iter(retired.iter()),
        )?;
        Ok(deleted)
    })();

    match result {
        Ok(deleted) => {
            conn.execute_batch("COMMIT;")?;
            Ok(deleted)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// Drop the retired model rows themselves once their neighbours are gone.
/// `track_embeddings` and any straggler `track_neighbors` cascade (the
/// connection runs with `foreign_keys = ON`). Returns models removed.
pub fn delete_retired_embedding_models(conn: &Connection, keep: usize) -> Result<usize> {
    let retired = retired_embedding_model_ids(conn, keep)?;
    if retired.is_empty() {
        return Ok(0);
    }
    let placeholders = vec!["?"; retired.len()].join(",");
    let sql = format!("DELETE FROM embedding_models WHERE id IN ({placeholders})");
    let removed = conn.execute(&sql, rusqlite::params_from_iter(retired.iter()))?;
    Ok(removed)
}

pub fn create_training_run(
    conn: &Connection,
    model_id: Option<i64>,
    stage: &str,
    status: &str,
) -> Result<DiscoveryTrainingRun> {
    conn.execute(
        "INSERT INTO training_runs (model_id, stage, status)
         VALUES (?1, ?2, ?3)",
        params![model_id, stage, status],
    )?;
    let id = conn.last_insert_rowid();
    get_training_run(conn, id)?.ok_or_else(|| anyhow::anyhow!("training run missing after insert"))
}

pub fn update_training_run_model(conn: &Connection, run_id: i64, model_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE training_runs SET model_id = ?2 WHERE id = ?1",
        params![run_id, model_id],
    )?;
    Ok(())
}

pub fn get_training_run(conn: &Connection, run_id: i64) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs WHERE id = ?1",
        params![run_id],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_latest_training_run(conn: &Connection) -> Result<Option<DiscoveryTrainingRun>> {
    conn.query_row(
        "SELECT id, model_id, stage, status, progress, items_total, items_done, started_at, finished_at, error_text
         FROM training_runs
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        [],
        |row| {
            Ok(DiscoveryTrainingRun {
                id: row.get(0)?,
                model_id: row.get(1)?,
                stage: row.get(2)?,
                status: row.get(3)?,
                progress: row.get(4)?,
                items_total: row.get(5)?,
                items_done: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                error_text: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn update_training_run_progress(
    conn: &Connection,
    run_id: i64,
    stage: &str,
    status: &str,
    progress: f64,
    items_total: Option<i64>,
    items_done: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET stage = ?2, status = ?3, progress = ?4, items_total = ?5, items_done = ?6
         WHERE id = ?1",
        params![run_id, stage, status, progress, items_total, items_done],
    )?;
    Ok(())
}

pub fn finish_training_run(conn: &Connection, run_id: i64, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = ?2, progress = 1.0, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, status],
    )?;
    Ok(())
}

pub fn finish_training_run_with_error(
    conn: &Connection,
    run_id: i64,
    status: &str,
    error_text: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = ?2, progress = 1.0, error_text = ?3, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, status, error_text],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn fail_training_run(conn: &Connection, run_id: i64, error_text: &str) -> Result<()> {
    conn.execute(
        "UPDATE training_runs
         SET status = 'failed', error_text = ?2, finished_at = datetime('now')
         WHERE id = ?1",
        params![run_id, error_text],
    )?;
    Ok(())
}

pub fn replace_track_embeddings(
    conn: &Connection,
    model_id: i64,
    embeddings: &[(i64, Vec<u8>, f64)],
) -> Result<()> {
    // Refuse to wipe an already-populated model with an empty payload. The
    // trainer is supposed to bail before reaching here when it produces no
    // output (cancel checks at every stage), but a logic bug or panic-recovery
    // path could still get us here with an empty slice — and a silent wipe
    // turns a recoverable issue into a "discovery engine just died" bug for
    // the user. Leave the prior rows in place; tracing makes the skip visible.
    if embeddings.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping embedding wipe: trainer returned 0 vectors but model has prior data"
            );
            return Ok(());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_embeddings WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_embeddings (track_id, model_id, vector_blob, l2_norm)
             VALUES (?1, ?2, ?3, ?4)",
        )?;
        for (track_id, blob, norm) in embeddings {
            stmt.execute(params![track_id, model_id, blob, norm])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Cached row from `track_audio_features`, used by Incremental Refresh to
/// avoid recomputing the audio-proxy stage when the prior run's features are
/// still valid. Caller is responsible for filtering rows whose unpacked
/// vector dimension doesn't match the current intensity tier.
pub struct CachedAudioFeatureRow {
    pub track_id: i64,
    pub feature_version: String,
    pub vector_blob: Vec<u8>,
    pub clip_start_ms: i64,
    pub clip_duration_ms: i64,
}

pub fn get_cached_audio_features(conn: &Connection) -> Result<Vec<CachedAudioFeatureRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms
         FROM track_audio_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CachedAudioFeatureRow {
                track_id: row.get(0)?,
                feature_version: row.get(1)?,
                vector_blob: row.get(2)?,
                clip_start_ms: row.get(3)?,
                clip_duration_ms: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn replace_track_audio_features(
    conn: &Connection,
    features: &[(i64, String, Vec<u8>, i64, i64)],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_audio_features (track_id, feature_version, vector_blob, clip_start_ms, clip_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET
                 feature_version = excluded.feature_version,
                 vector_blob = excluded.vector_blob,
                 clip_start_ms = excluded.clip_start_ms,
                 clip_duration_ms = excluded.clip_duration_ms,
                 computed_at = datetime('now')",
        )?;
        for (track_id, version, blob, start_ms, duration_ms) in features {
            stmt.execute(params![track_id, version, blob, start_ms, duration_ms])?;
        }
    }
    tx.commit()?;
    Ok(())
}

// Per-reason held-out hit-rate row, mirroring the structure emitted by the
// trainer. Kept as a separate struct on the queries side so the trainer module
// doesn't need to depend on rusqlite param plumbing.
pub struct ReasonHitRateRow {
    pub primary_reason: String,
    pub impressions: i64,
    pub hits: i64,
    pub hit_rate: f64,
    pub mean_rank: Option<f64>,
    pub mrr_contribution: f64,
    pub insufficient_data: bool,
}

// Replaces all per-reason hit-rate rows for a model. Wrapped in a transaction
// so a partial replacement can't leave stale rows from a prior training run.
pub fn replace_discovery_diagnostics(
    conn: &Connection,
    model_id: i64,
    rates: &[ReasonHitRateRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM discovery_diagnostics WHERE model_id = ?1",
        params![model_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO discovery_diagnostics
             (model_id, primary_reason, impressions, hits, hit_rate,
              mean_rank, mrr_contribution, insufficient_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for r in rates {
            stmt.execute(params![
                model_id,
                r.primary_reason,
                r.impressions,
                r.hits,
                r.hit_rate,
                r.mean_rank,
                r.mrr_contribution,
                r.insufficient_data as i32,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

#[allow(dead_code)]
pub fn get_per_reason_hit_rates(conn: &Connection, model_id: i64) -> Result<Vec<ReasonHitRateRow>> {
    let mut stmt = conn.prepare(
        "SELECT primary_reason, impressions, hits, hit_rate,
                mean_rank, mrr_contribution, insufficient_data
         FROM discovery_diagnostics
         WHERE model_id = ?1
         ORDER BY impressions DESC",
    )?;
    let rows = stmt
        .query_map(params![model_id], |row| {
            Ok(ReasonHitRateRow {
                primary_reason: row.get(0)?,
                impressions: row.get(1)?,
                hits: row.get(2)?,
                hit_rate: row.get(3)?,
                mean_rank: row.get(4)?,
                mrr_contribution: row.get(5)?,
                insufficient_data: row.get::<_, i32>(6)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn replace_track_neighbors(
    conn: &Connection,
    model_id: i64,
    neighbors: &[NeighborWriteRow],
) -> Result<()> {
    // Single transaction: ~2M+ INSERTs auto-committing one-by-one is what makes
    // training appear to hang. Batching also makes the DELETE+INSERT atomic so a
    // killed process can't leave the table half-populated.
    //
    // Same defensive skip as `replace_track_embeddings`: an empty slice on a
    // populated model leaves the prior graph intact rather than wiping it.
    if neighbors.is_empty() {
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM track_neighbors WHERE model_id = ?1",
            params![model_id],
            |row| row.get(0),
        )?;
        if existing > 0 {
            tracing::warn!(
                target: "noor.discovery.training",
                model_id,
                existing_rows = existing,
                "skipping neighbor wipe: trainer returned 0 edges but model has prior data"
            );
            return Ok(());
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1",
        params![model_id],
    )?;
    insert_neighbor_rows(&tx, model_id, neighbors)?;
    tx.commit()?;
    Ok(())
}

/// Add neighbour rows to a model without clearing it, in one short
/// transaction. Training writes a multi-million-row graph as
/// `replace_track_neighbors` (first chunk) plus these appends, releasing the
/// shared connection between chunks so playback and requests keep running.
/// The model is still inactive while it fills, so readers never see it half
/// written.
pub fn append_track_neighbors(
    conn: &Connection,
    model_id: i64,
    neighbors: &[NeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    insert_neighbor_rows(&tx, model_id, neighbors)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn insert_neighbor_rows(
    conn: &Connection,
    model_id: i64,
    neighbors: &[NeighborWriteRow],
) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT INTO track_neighbors
         (track_id, neighbor_track_id, model_id, rank, score,
          behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
          confidence, support_count, support_transition, support_colisten, support_structure,
          support_metadata, candidate_in_degree, candidate_in_degree_percentile,
          play_count_seed, play_count_candidate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
    )?;
    for n in neighbors {
        stmt.execute(params![
            n.track_id,
            n.neighbor_track_id,
            model_id,
            n.rank,
            n.score,
            n.behavioral_score,
            n.audio_score,
            n.metadata_score,
            n.reason_json,
            n.primary_reason,
            n.confidence,
            n.support_count,
            n.support_transition,
            n.support_colisten,
            n.support_structure,
            n.support_metadata,
            n.candidate_in_degree,
            n.candidate_in_degree_percentile,
            n.play_count_seed,
            n.play_count_candidate,
        ])?;
    }
    Ok(())
}

/// Replace neighbor rows for a single seed track only — used by the background
/// per-seed refresh so it doesn't wipe every other track's neighbors.
/// True when `seed_id` already has neighbor rows under `model_id`.
pub fn seed_has_neighbors(conn: &Connection, model_id: i64, seed_id: i64) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM track_neighbors WHERE model_id = ?1 AND track_id = ?2)",
        params![model_id, seed_id],
        |row| row.get(0),
    )?)
}

pub fn replace_seed_neighbors(
    conn: &Connection,
    model_id: i64,
    seed_id: i64,
    rows: &[NeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM track_neighbors WHERE model_id = ?1 AND track_id = ?2",
        params![model_id, seed_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO track_neighbors
             (track_id, neighbor_track_id, model_id, rank, score,
              behavioral_score, audio_score, metadata_score, reason_json, primary_reason,
              confidence, support_count, support_transition, support_colisten, support_structure,
              support_metadata, candidate_in_degree, candidate_in_degree_percentile,
              play_count_seed, play_count_candidate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        )?;
        for n in rows {
            stmt.execute(params![
                n.track_id,
                n.neighbor_track_id,
                model_id,
                n.rank,
                n.score,
                n.behavioral_score,
                n.audio_score,
                n.metadata_score,
                n.reason_json,
                n.primary_reason,
                n.confidence,
                n.support_count,
                n.support_transition,
                n.support_colisten,
                n.support_structure,
                n.support_metadata,
                n.candidate_in_degree,
                n.candidate_in_degree_percentile,
                n.play_count_seed,
                n.play_count_candidate,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn get_track_neighbors(
    conn: &Connection,
    model_id: i64,
    track_id: i64,
    limit: i64,
    exclude_ids: &[i64],
) -> Result<Vec<EmbeddingNeighborRow>> {
    let sql =
        "SELECT t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                      n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json,
                      n.confidence, n.support_count, n.candidate_in_degree,
                      n.candidate_in_degree_percentile, n.play_count_seed, n.play_count_candidate,
                      n.primary_reason, n.support_transition, n.support_colisten,
                      n.support_structure, n.support_metadata
               FROM track_neighbors n
               JOIN tracks t ON t.id = n.neighbor_track_id
               LEFT JOIN artists a ON a.id = t.artist_id
               LEFT JOIN albums al ON al.id = t.album_id
               WHERE n.model_id = ?1 AND n.track_id = ?2
               ORDER BY n.rank ASC
               LIMIT ?3";
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt
        .query_map(params![model_id, track_id, limit.max(1)], |row| {
            Ok(EmbeddingNeighborRow {
                track_id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                artwork_url: row.get(4)?,
                duration_ms: row.get(5)?,
                best_quality: row.get(6)?,
                score: row.get(7)?,
                behavioral_score: row.get(8)?,
                audio_score: row.get(9)?,
                metadata_score: row.get(10)?,
                reason_json: row.get(11)?,
                confidence: row.get(12)?,
                support_count: row.get(13)?,
                candidate_in_degree: row.get(14)?,
                candidate_in_degree_percentile: row.get(15)?,
                play_count_seed: row.get(16)?,
                play_count_candidate: row.get(17)?,
                primary_reason: row.get(18)?,
                support_transition: row.get(19)?,
                support_colisten: row.get(20)?,
                support_structure: row.get(21)?,
                support_metadata: row.get(22)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !exclude_ids.is_empty() {
        let exclude = exclude_ids.iter().copied().collect::<HashSet<_>>();
        rows.retain(|row| !exclude.contains(&row.track_id));
    }
    Ok(rows)
}

pub fn get_track_neighbors_for_seeds(
    conn: &Connection,
    model_id: i64,
    seed_ids: &[i64],
    limit_per_seed: i64,
) -> Result<HashMap<i64, Vec<EmbeddingNeighborRow>>> {
    if seed_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = (0..seed_ids.len())
        .map(|idx| format!("?{}", idx + 3))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT n.track_id, t.id, t.title, a.name, al.title, al.artwork_url, t.duration_ms, t.best_quality,
                n.score, n.behavioral_score, n.audio_score, n.metadata_score, n.reason_json,
                n.confidence, n.support_count, n.candidate_in_degree,
                n.candidate_in_degree_percentile, n.play_count_seed, n.play_count_candidate,
                n.primary_reason, n.support_transition, n.support_colisten,
                n.support_structure, n.support_metadata
         FROM track_neighbors n
         JOIN tracks t ON t.id = n.neighbor_track_id
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         WHERE n.model_id = ?1
           AND n.rank <= ?2
           AND n.track_id IN ({placeholders})
         ORDER BY n.track_id ASC, n.rank ASC"
    );
    let mut values = vec![model_id, limit_per_seed.max(1)];
    values.extend(seed_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |row| {
            let seed_id = row.get::<_, i64>(0)?;
            let neighbor = EmbeddingNeighborRow {
                track_id: row.get(1)?,
                title: row.get(2)?,
                artist_name: row.get(3)?,
                album_title: row.get(4)?,
                artwork_url: row.get(5)?,
                duration_ms: row.get(6)?,
                best_quality: row.get(7)?,
                score: row.get(8)?,
                behavioral_score: row.get(9)?,
                audio_score: row.get(10)?,
                metadata_score: row.get(11)?,
                reason_json: row.get(12)?,
                confidence: row.get(13)?,
                support_count: row.get(14)?,
                candidate_in_degree: row.get(15)?,
                candidate_in_degree_percentile: row.get(16)?,
                play_count_seed: row.get(17)?,
                play_count_candidate: row.get(18)?,
                primary_reason: row.get(19)?,
                support_transition: row.get(20)?,
                support_colisten: row.get(21)?,
                support_structure: row.get(22)?,
                support_metadata: row.get(23)?,
            };
            Ok((seed_id, neighbor))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<EmbeddingNeighborRow>> = HashMap::new();
    for (seed_id, neighbor) in rows {
        grouped.entry(seed_id).or_default().push(neighbor);
    }
    Ok(grouped)
}

/// Artist-level "hub-ness": for each requested artist, the highest in-degree
/// percentile any of that artist's tracks carries as a neighbour, across every
/// model. In most libraries a few artists end up over-connected in the similarity
/// graph (heavy co-listen history, a large catalogue, broad genre tags) and get
/// listed as a neighbour for a huge share of seeds. That pollution is artist-wide,
/// not per-track: an artist's deep cuts can have a low individual in-degree yet
/// still ride into every pool because the *artist* is a top neighbour everywhere.
/// Keying on the artist's max in-degree lets one genuinely hubby track flag the
/// whole catalogue, which is what catches that low-in-degree filler. Artists with
/// no neighbour rows are omitted, so the caller treats a missing entry as 0.
pub fn get_artist_hub_percentiles(
    conn: &Connection,
    artist_ids: &[i64],
) -> Result<HashMap<i64, f64>> {
    let mut out = HashMap::new();
    if artist_ids.is_empty() {
        return Ok(out);
    }
    // Chunk well under SQLite's bound-parameter ceiling.
    for chunk in artist_ids.chunks(400) {
        let placeholders = (0..chunk.len())
            .map(|idx| format!("?{}", idx + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT t.artist_id, MAX(n.candidate_in_degree_percentile)
             FROM track_neighbors n
             JOIN tracks t ON t.id = n.neighbor_track_id
             WHERE t.artist_id IN ({placeholders})
             GROUP BY t.artist_id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?))
        })?;
        for row in rows {
            let (artist_id, pct) = row?;
            if let Some(pct) = pct {
                out.insert(artist_id, pct);
            }
        }
    }
    Ok(out)
}

pub(super) fn get_external_track_candidate_by_id(
    conn: &Connection,
    id: i64,
) -> Result<ExternalTrackCandidateRow> {
    conn.query_row(
        "SELECT id, tidal_id, mbid, dedupe_key, title, artist_name, genre_tags_json,
                duration_ms, expires_at, updated_at, NULL AS source_tags_json, resolved_track_id
         FROM external_track_candidates
         WHERE id = ?1",
        params![id],
        |row| {
            Ok(ExternalTrackCandidateRow {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                mbid: row.get(2)?,
                dedupe_key: row.get(3)?,
                title: row.get(4)?,
                artist_name: row.get(5)?,
                genre_tags_json: row.get(6)?,
                duration_ms: row.get(7)?,
                expires_at: row.get(8)?,
                updated_at: row.get(9)?,
                source_tags_json: row.get(10)?,
                resolved_track_id: row.get(11)?,
            })
        },
    )
    .map_err(Into::into)
}

pub fn get_external_track_candidates_for_training(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ExternalTrackCandidateRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, tidal_id, mbid, dedupe_key, title, artist_name, genre_tags_json,
                duration_ms, expires_at, updated_at,
                (
                    SELECT json_group_array(tag)
                    FROM (
                        SELECT DISTINCT source AS tag
                        FROM external_track_candidate_sightings s
                        WHERE s.candidate_id = c.id
                          AND s.expires_at > ?1
                        UNION
                        SELECT DISTINCT
                            CASE
                                WHEN s.source = 'lastfm_similar'
                                 AND s.source_payload_json LIKE '%\"branch_from\":%'
                                 AND s.source_payload_json NOT LIKE '%\"branch_from\":null%'
                                THEN 'lastfm_branch'
                                WHEN s.source = 'lastfm_similar'
                                THEN 'lastfm_direct'
                            END AS tag
                        FROM external_track_candidate_sightings s
                        WHERE s.candidate_id = c.id
                          AND s.expires_at > ?1
                          AND s.source = 'lastfm_similar'
                        ORDER BY tag
                    )
                    WHERE tag IS NOT NULL
                ) AS source_tags_json,
                resolved_track_id
         FROM external_track_candidates c
         WHERE c.expires_at > ?1
           AND c.resolved_track_id IS NULL
         ORDER BY c.updated_at DESC, c.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ExternalTrackCandidateRow {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                mbid: row.get(2)?,
                dedupe_key: row.get(3)?,
                title: row.get(4)?,
                artist_name: row.get(5)?,
                genre_tags_json: row.get(6)?,
                duration_ms: row.get(7)?,
                expires_at: row.get(8)?,
                updated_at: row.get(9)?,
                source_tags_json: row.get(10)?,
                resolved_track_id: row.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_resolved_lastfm_external_sightings_for_training(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ResolvedLastfmExternalSightingRow>> {
    let mut stmt = conn.prepare(
        "SELECT s.seed_track_id,
                c.resolved_track_id,
                COALESCE(s.similarity, 0.0),
                s.source_payload_json
         FROM external_track_candidate_sightings s
         JOIN external_track_candidates c ON c.id = s.candidate_id
         WHERE s.source = 'lastfm_similar'
           AND s.expires_at > ?1
           AND c.expires_at > ?1
           AND c.resolved_track_id IS NOT NULL
           AND c.resolved_track_id <> s.seed_track_id
         ORDER BY COALESCE(s.similarity, 0.0) DESC,
                  s.expires_at DESC,
                  s.candidate_id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ResolvedLastfmExternalSightingRow {
                seed_track_id: row.get(0)?,
                resolved_track_id: row.get(1)?,
                similarity: row.get(2)?,
                source_payload_json: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_unresolved_lastfm_external_candidates_for_tidal_resolution(
    conn: &Connection,
    now: &str,
    limit: i64,
) -> Result<Vec<ExternalTidalResolutionCandidateRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.title, c.artist_name, c.duration_ms,
                COUNT(s.seed_track_id) AS sighting_count,
                MAX(s.similarity) AS max_similarity,
                c.expires_at
         FROM external_track_candidates c
         JOIN external_track_candidate_sightings s ON s.candidate_id = c.id
         WHERE c.expires_at > ?1
           AND c.resolved_track_id IS NULL
           AND c.tidal_id IS NULL
           AND s.source = 'lastfm_similar'
           AND s.expires_at > ?1
         GROUP BY c.id
         ORDER BY sighting_count DESC,
                  COALESCE(max_similarity, 0) DESC,
                  c.expires_at DESC,
                  c.updated_at DESC,
                  c.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![now, limit.max(1)], |row| {
            Ok(ExternalTidalResolutionCandidateRow {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                duration_ms: row.get(3)?,
                sighting_count: row.get(4)?,
                max_similarity: row.get(5)?,
                expires_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn count_playable_external_candidates(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM external_track_candidates
         WHERE tidal_id IS NOT NULL
           AND resolved_track_id IS NULL
           AND expires_at > datetime('now')",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn get_tidal_similar_seed_rows(
    conn: &Connection,
    limit: i64,
) -> Result<Vec<ExternalTidalSimilarSeedRow>> {
    let mut stmt = conn.prepare(
        "SELECT t.id, ar.tidal_id
         FROM tracks t
         JOIN artists ar ON ar.id = t.artist_id
         WHERE t.tidal_id IS NOT NULL
           AND ar.tidal_id IS NOT NULL
         ORDER BY t.play_count DESC, t.last_played_at DESC, t.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit.max(1)], |row| {
            Ok(ExternalTidalSimilarSeedRow {
                track_id: row.get(0)?,
                artist_tidal_id: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn upsert_external_track_candidate(
    conn: &Connection,
    input: &ExternalTrackCandidateUpsert,
) -> Result<ExternalTrackCandidateRow> {
    let fallback_identity =
        external_candidate_fallback_identity(&input.artist_name, &input.title, input.duration_ms);
    let existing_id = if let Some(tidal_id) = input.tidal_id {
        conn.query_row(
            "SELECT id FROM external_track_candidates WHERE tidal_id = ?1",
            params![tidal_id],
            |row| row.get(0),
        )
        .optional()?
    } else if let Some(mbid) = input.mbid.as_deref() {
        conn.query_row(
            "SELECT id FROM external_track_candidates WHERE mbid = ?1",
            params![mbid],
            |row| row.get(0),
        )
        .optional()?
    } else {
        conn.query_row(
            "SELECT id FROM external_track_candidates
             WHERE dedupe_key = ?1
                OR (
                    tidal_id IS NULL
                    AND mbid IS NULL
                    AND
                    normalized_artist_name = ?2
                    AND normalized_title = ?3
                    AND duration_bucket = ?4
                )
             LIMIT 1",
            params![
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
            ],
            |row| row.get(0),
        )
        .optional()?
    };

    let id = if let Some(id) = existing_id {
        conn.execute(
            "UPDATE external_track_candidates
             SET tidal_id = COALESCE(?2, tidal_id),
                 mbid = COALESCE(?3, mbid),
                 dedupe_key = ?4,
                 normalized_artist_name = ?5,
                 normalized_title = ?6,
                 duration_bucket = ?7,
                 title = ?8,
                 artist_name = ?9,
                 genre_tags_json = ?10,
                 duration_ms = ?11,
                 expires_at = ?12,
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![
                id,
                input.tidal_id,
                input.mbid,
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
                input.title,
                input.artist_name,
                input.genre_tags_json,
                input.duration_ms,
                input.expires_at,
            ],
        )?;
        id
    } else {
        conn.execute(
            "INSERT INTO external_track_candidates
             (tidal_id, mbid, dedupe_key, normalized_artist_name, normalized_title,
              duration_bucket, title, artist_name, genre_tags_json, duration_ms, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                input.tidal_id,
                input.mbid,
                input.dedupe_key,
                &fallback_identity.normalized_artist_name,
                &fallback_identity.normalized_title,
                fallback_identity.duration_bucket,
                input.title,
                input.artist_name,
                input.genre_tags_json,
                input.duration_ms,
                input.expires_at,
            ],
        )?;
        conn.last_insert_rowid()
    };

    get_external_track_candidate_by_id(conn, id)
}

pub fn resolve_external_candidate_tidal_metadata(
    conn: &Connection,
    candidate_id: i64,
    input: &ExternalCandidateTidalResolution,
) -> Result<ExternalTrackCandidateRow> {
    if input.tidal_id <= 0 {
        bail!("external candidate tidal_id must be positive");
    }

    if let Some(existing_id) = conn
        .query_row(
            "SELECT id FROM external_track_candidates WHERE tidal_id = ?1 AND id <> ?2",
            params![input.tidal_id, candidate_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    {
        conn.execute(
            "UPDATE external_track_candidates
             SET genre_tags_json = COALESCE(genre_tags_json, ?2),
                 duration_ms = COALESCE(duration_ms, ?3),
                 updated_at = datetime('now')
             WHERE id = ?1",
            params![existing_id, input.genre_tags_json, input.duration_ms],
        )?;
        merge_external_track_candidates(conn, existing_id, candidate_id)?;
        return get_external_track_candidate_by_id(conn, existing_id);
    }

    conn.execute(
        "UPDATE external_track_candidates
         SET tidal_id = ?2,
             dedupe_key = ?3,
             genre_tags_json = COALESCE(genre_tags_json, ?4),
             duration_ms = COALESCE(duration_ms, ?5),
             updated_at = datetime('now')
         WHERE id = ?1",
        params![
            candidate_id,
            input.tidal_id,
            format!("tidal:{}", input.tidal_id),
            input.genre_tags_json,
            input.duration_ms,
        ],
    )?;
    get_external_track_candidate_by_id(conn, candidate_id)
}

pub(super) fn external_candidate_fallback_identity(
    artist_name: &str,
    title: &str,
    duration_ms: Option<i64>,
) -> ExternalCandidateFallbackIdentity {
    ExternalCandidateFallbackIdentity {
        normalized_artist_name: normalize_external_candidate_text(artist_name),
        normalized_title: normalize_external_candidate_text(title),
        duration_bucket: duration_ms.map(|value| value / 30_000).unwrap_or(0),
    }
}

pub(super) fn normalize_external_candidate_text(value: &str) -> String {
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

pub fn upsert_external_candidate_sighting(
    conn: &Connection,
    input: &ExternalCandidateSightingUpsert,
) -> Result<()> {
    conn.execute(
        "INSERT INTO external_track_candidate_sightings
         (candidate_id, seed_track_id, source, source_payload_json, similarity, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(candidate_id, seed_track_id, source) DO UPDATE SET
             source_payload_json = excluded.source_payload_json,
             similarity = excluded.similarity,
             seen_at = datetime('now'),
             expires_at = excluded.expires_at",
        params![
            input.candidate_id,
            input.seed_track_id,
            input.source,
            input.source_payload_json,
            input.similarity,
            input.expires_at,
        ],
    )?;
    Ok(())
}

pub fn replace_external_candidate_neighbors(
    conn: &Connection,
    model_id: i64,
    library_track_id: i64,
    rows: &[ExternalCandidateNeighborWriteRow],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM external_track_candidate_neighbors
         WHERE model_id = ?1 AND library_track_id = ?2",
        params![model_id, library_track_id],
    )?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO external_track_candidate_neighbors
             (library_track_id, candidate_id, model_id, rank, score, audio_score, metadata_score, reason_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for row in rows {
            stmt.execute(params![
                library_track_id,
                row.candidate_id,
                model_id,
                row.rank,
                row.score,
                row.audio_score,
                row.metadata_score,
                row.reason_json,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// External candidates Last.fm linked to the given seeds, best first. Each
/// seed carries a weight (1.0 for the playing track, less for its learned
/// neighbors); a candidate scores its best `similarity x weight`, and only
/// scores at or above `min_score` count. Playable (TIDAL id) and unexpired only.
pub fn get_sighted_external_candidates(
    conn: &Connection,
    weighted_seeds: &[(i64, f64)],
    min_score: f64,
    limit: i64,
) -> Result<Vec<ExternalCandidateNeighborRow>> {
    if weighted_seeds.is_empty() {
        return Ok(Vec::new());
    }
    let values = (0..weighted_seeds.len())
        .map(|i| format!("(?{}, ?{})", i * 2 + 3, i * 2 + 4))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH w(seed_id, weight) AS (VALUES {values})
         SELECT c.id, c.tidal_id, c.title, c.artist_name, c.duration_ms,
                MAX(COALESCE(s.similarity, 0) * w.weight) AS score
         FROM external_track_candidate_sightings s
         JOIN w ON w.seed_id = s.seed_track_id
         JOIN external_track_candidates c ON c.id = s.candidate_id
         WHERE c.tidal_id IS NOT NULL
           AND julianday(s.expires_at) > julianday('now')
           AND julianday(c.expires_at) > julianday('now')
         GROUP BY c.id
         HAVING score >= ?1
         ORDER BY score DESC, c.id
         LIMIT ?2"
    );
    let mut bound: Vec<rusqlite::types::Value> = vec![min_score.into(), limit.max(1).into()];
    for (seed_id, weight) in weighted_seeds {
        bound.push((*seed_id).into());
        bound.push((*weight).into());
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, f64>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .enumerate()
        .map(
            |(rank, (candidate_id, tidal_id, title, artist_name, duration_ms, score))| {
                ExternalCandidateNeighborRow {
                    candidate_id,
                    tidal_id,
                    title,
                    artist_name,
                    duration_ms,
                    rank: rank as i32 + 1,
                    score,
                    audio_score: 0.0,
                    metadata_score: 0.0,
                    reason_json: None,
                }
            },
        )
        .collect())
}

pub fn get_external_candidate_neighbors(
    conn: &Connection,
    model_id: i64,
    library_track_id: i64,
    limit: i64,
    require_tidal: bool,
) -> Result<Vec<ExternalCandidateNeighborRow>> {
    let mut stmt = conn.prepare(
        "SELECT c.id, c.tidal_id, c.title, c.artist_name, c.duration_ms,
                n.rank, n.score, n.audio_score, n.metadata_score, n.reason_json
         FROM external_track_candidate_neighbors n
         JOIN external_track_candidates c ON c.id = n.candidate_id
         WHERE n.model_id = ?1
           AND n.library_track_id = ?2
           AND (?3 = 0 OR c.tidal_id IS NOT NULL)
           AND julianday(c.expires_at) > julianday('now')
         ORDER BY n.rank ASC
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            params![
                model_id,
                library_track_id,
                require_tidal as i32,
                limit.max(1)
            ],
            |row| {
                Ok(ExternalCandidateNeighborRow {
                    candidate_id: row.get(0)?,
                    tidal_id: row.get(1)?,
                    title: row.get(2)?,
                    artist_name: row.get(3)?,
                    duration_ms: row.get(4)?,
                    rank: row.get(5)?,
                    score: row.get(6)?,
                    audio_score: row.get(7)?,
                    metadata_score: row.get(8)?,
                    reason_json: row.get(9)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn mark_external_candidate_resolved(
    conn: &Connection,
    tidal_id: Option<i64>,
    title: &str,
    artist_name: &str,
    resolved_track_id: i64,
) -> Result<usize> {
    let mut changed = 0usize;
    if let Some(tidal_id) = tidal_id.filter(|id| *id > 0) {
        changed += conn.execute(
            "UPDATE external_track_candidates
             SET resolved_track_id = ?2
             WHERE tidal_id = ?1",
            params![tidal_id, resolved_track_id],
        )?;
        if changed > 0 {
            return Ok(changed);
        }
    }

    changed += conn.execute(
        "UPDATE OR IGNORE external_track_candidates
         SET tidal_id = COALESCE(?1, tidal_id),
             resolved_track_id = ?4
         WHERE resolved_track_id IS NULL
           AND tidal_id IS NULL
           AND lower(trim(title)) = lower(trim(?2))
           AND lower(trim(artist_name)) = lower(trim(?3))",
        params![tidal_id, title, artist_name, resolved_track_id],
    )?;
    Ok(changed)
}

pub fn merge_external_track_candidates(
    conn: &Connection,
    winner_id: i64,
    loser_id: i64,
) -> Result<()> {
    if winner_id == loser_id {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_sightings
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_sightings WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_audio_features
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_audio_features WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_embeddings
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_embeddings WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "UPDATE OR IGNORE external_track_candidate_neighbors
         SET candidate_id = ?1
         WHERE candidate_id = ?2",
        params![winner_id, loser_id],
    )?;
    tx.execute(
        "DELETE FROM external_track_candidate_neighbors WHERE candidate_id = ?1",
        params![loser_id],
    )?;

    tx.execute(
        "DELETE FROM external_track_candidates WHERE id = ?1",
        params![loser_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn get_model_embeddings(conn: &Connection, model_id: i64) -> Result<Vec<ModelEmbeddingRow>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, vector_blob, l2_norm
         FROM track_embeddings
         WHERE model_id = ?1",
    )?;
    stmt.query_map(params![model_id], |row| {
        Ok(ModelEmbeddingRow {
            track_id: row.get(0)?,
            vector_blob: row.get(1)?,
            l2_norm: row.get(2)?,
        })
    })?
    .collect::<rusqlite::Result<Vec<_>>>()
    .map_err(Into::into)
}

pub fn get_embedding_track_rows(conn: &Connection) -> Result<Vec<EmbeddingTrackRow>> {
    // Discovery embedding training pulls in album/artist fallback genres so
    // niche tracks (no track-level enrichment yet) cluster near coherent
    // peers in the embedding instead of isolating. Cost ~2s on training start;
    // training is periodic, not per-request, so the whole-library scan is fine.
    let genre_paths_with_provenance = get_track_genre_paths_with_fallback(conn)?;
    let genre_paths: HashMap<i64, Vec<String>> = genre_paths_with_provenance
        .into_iter()
        .map(|(id, rows)| (id, ResolvedGenre::paths_only(&rows)))
        .collect();
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, a.name, al.title, t.duration_ms, t.best_quality, t.source,
                t.play_count, t.is_favorite,
                (SELECT COUNT(*) FROM playlist_tracks pt WHERE pt.track_id = t.id) AS playlist_memberships,
                d.bpm, d.energy, d.camelot_key, d.danceability, d.beat_strength, d.loudness_lufs
         FROM tracks t
         LEFT JOIN artists a ON a.id = t.artist_id
         LEFT JOIN albums al ON al.id = t.album_id
         LEFT JOIN audio_dsp_features d ON d.track_id = t.id
         WHERE t.tidal_id IS NOT NULL OR t.file_path IS NOT NULL OR t.ytmusic_id IS NOT NULL OR t.soundcloud_id IS NOT NULL",
    )?;
    let mut rows = stmt
        .query_map([], |row| {
            let track_id = row.get::<_, i64>(0)?;
            Ok(EmbeddingTrackRow {
                track_id,
                title: row.get(1)?,
                artist_name: row.get(2)?,
                album_title: row.get(3)?,
                duration_ms: row.get(4)?,
                best_quality: row.get(5)?,
                source: row.get(6)?,
                play_count: row.get(7)?,
                is_favorite: row.get(8)?,
                playlist_memberships: row.get(9)?,
                genre_paths: genre_paths.get(&track_id).cloned().unwrap_or_default(),
                bpm: row.get(10)?,
                energy: row.get(11)?,
                camelot_key: row.get(12)?,
                danceability: row.get(13)?,
                beat_strength: row.get(14)?,
                loudness_lufs: row.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.sort_by_key(|row| row.track_id);
    Ok(rows)
}

pub fn get_discovery_status(conn: &Connection) -> Result<DiscoveryStatus> {
    let selected_engine = selected_discovery_engine(conn)?;
    let selected_engine_family = discovery_model_family_for_engine(&selected_engine).to_string();
    let active_model = get_selected_discovery_embedding_model(conn)?;
    let latest_run = get_latest_training_run(conn)?;
    let playable_tracks: i64 = conn.query_row(
        "SELECT COUNT(*)
         FROM tracks
         WHERE tidal_id IS NOT NULL OR file_path IS NOT NULL OR ytmusic_id IS NOT NULL OR soundcloud_id IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    let embedded_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(*) FROM track_embeddings WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let neighbor_tracks: i64 = match active_model.as_ref() {
        Some(model) => conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM track_neighbors WHERE model_id = ?1",
            params![model.id],
            |row| row.get(0),
        )?,
        None => 0,
    };
    let clip_cache_tracks: i64 =
        conn.query_row("SELECT COUNT(*) FROM track_audio_features", [], |row| {
            row.get(0)
        })?;
    let coverage_ratio = if playable_tracks == 0 {
        0.0
    } else {
        neighbor_tracks as f64 / playable_tracks as f64
    };

    let selected_engine_trainable = selected_engine == DISCOVERY_ENGINE_V2;
    Ok(DiscoveryStatus {
        fallback_active: active_model.is_none(),
        active_model,
        selected_engine,
        selected_engine_family,
        selected_engine_trainable,
        latest_run,
        coverage_ratio,
        playable_tracks,
        embedded_tracks,
        neighbor_tracks,
        clip_cache_tracks,
    })
}

pub fn record_playback_transition(
    conn: &Connection,
    from_track_id: i64,
    to_track_id: i64,
    transition_source: &str,
    completed_prev: bool,
    gap_ms: i64,
) -> Result<()> {
    if from_track_id <= 0 || to_track_id <= 0 || from_track_id == to_track_id {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO playback_transitions
         (from_track_id, to_track_id, transition_source, completed_prev, gap_ms)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            from_track_id,
            to_track_id,
            transition_source,
            completed_prev,
            gap_ms
        ],
    )?;
    Ok(())
}

pub fn record_discovery_feedback(
    conn: &Connection,
    seed_track_id: i64,
    candidate_track_id: i64,
    action: &str,
    surface: &str,
    context_json: Option<&str>,
    session_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO discovery_feedback
         (seed_track_id, candidate_track_id, action, surface, context_json, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            seed_track_id,
            candidate_track_id,
            action,
            surface,
            context_json,
            session_id
        ],
    )?;
    Ok(())
}

/// Most recent feedback rows for a discovery session, newest first:
/// `(candidate_track_id, action, artist_id)`. The artist id comes along via a
/// join so the rerank taste builder needs only one extra batched genre lookup.
pub fn get_discovery_feedback_for_session(
    conn: &Connection,
    session_id: &str,
    limit: i64,
) -> Result<Vec<(i64, String, Option<i64>)>> {
    let mut stmt = conn.prepare(
        "SELECT df.candidate_track_id, df.action, t.artist_id
         FROM discovery_feedback df
         LEFT JOIN tracks t ON t.id = df.candidate_track_id
         WHERE df.session_id = ?1
         ORDER BY df.created_at DESC, df.id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_id, limit], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

#[allow(dead_code)]
pub fn get_playback_transition_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT from_track_id, to_track_id
         FROM playback_transitions
         ORDER BY created_at ASC, id ASC",
    )?;
    let pairs = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pairs.into_iter().map(|(a, b)| vec![a, b]).collect())
}

#[derive(Debug, Clone)]
pub struct WeightedTrackPairRow {
    pub event_id: String,
    pub from_track_id: i64,
    pub to_track_id: i64,
    pub weight: f64,
    pub source: Option<String>,
    pub completed_prev: Option<bool>,
}

pub fn get_playback_transition_edges(conn: &Connection) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "SELECT
            'playback_transition:' || id,
            from_track_id,
            to_track_id,
            1.0,
            transition_source,
            completed_prev
         FROM playback_transitions
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: Some(row.get::<_, bool>(5)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_completion_weighted_listen_edges(
    conn: &Connection,
    session_window_minutes: i64,
) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "WITH weighted AS (
            SELECT
                lh.id,
                lh.track_id,
                lh.started_at,
                lh.session_id,
                lh.source,
                CASE
                    WHEN t.duration_ms IS NOT NULL AND t.duration_ms > 0 THEN
                        MIN(1.0, CAST(COALESCE(lh.duration_listened_ms, 0) AS REAL) / CAST(t.duration_ms AS REAL))
                    WHEN COALESCE(lh.completed, 0) = 1 THEN 1.0
                    ELSE 0.25
                END AS completion_weight
            FROM listen_history lh
            JOIN tracks t ON t.id = lh.track_id
            -- Early skips are not co-listens.
            WHERE (COALESCE(lh.completed, 0) = 1
                OR (COALESCE(lh.duration_listened_ms, 0) >= 30000
                    AND (t.duration_ms IS NULL OR t.duration_ms <= 0
                         OR COALESCE(lh.duration_listened_ms, 0) * 4 >= t.duration_ms)))
        )
        SELECT
            'listen_history_pair:' || a.id || ':' || b.id,
            a.track_id,
            b.track_id,
            MIN(a.completion_weight, b.completion_weight),
            COALESCE(b.source, a.source)
        FROM weighted a
        JOIN weighted b
            ON b.id > a.id
           AND b.track_id != a.track_id
           AND (
                (a.session_id IS NOT NULL AND a.session_id = b.session_id)
                OR (
                    (a.session_id IS NULL OR b.session_id IS NULL)
                    -- julianday(): started_at is RFC 3339, datetime() output is not; text compares fail.
                    AND julianday(b.started_at)
                        BETWEEN julianday(a.started_at)
                            AND julianday(a.started_at) + (?1 / 1440.0)
                )
           )
        ORDER BY a.started_at ASC, a.id ASC, b.started_at ASC, b.id ASC",
    )?;
    let rows = stmt
        .query_map(params![session_window_minutes.max(1)], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn get_listen_history_transition_edges(conn: &Connection) -> Result<Vec<WeightedTrackPairRow>> {
    let mut stmt = conn.prepare(
        "SELECT
            'listen_history:' || lh.id,
            lh.transition_from_track_id,
            lh.track_id,
            CASE
                WHEN t.duration_ms IS NOT NULL AND t.duration_ms > 0 THEN
                    MIN(1.0, CAST(COALESCE(lh.duration_listened_ms, 0) AS REAL) / CAST(t.duration_ms AS REAL))
                WHEN COALESCE(lh.completed, 0) = 1 THEN 1.0
                ELSE 0.25
            END AS completion_weight,
            lh.source
         FROM listen_history lh
         JOIN tracks t ON t.id = lh.track_id
         WHERE lh.transition_from_track_id IS NOT NULL
           AND lh.transition_from_track_id != lh.track_id
           -- An early skip (under 30 s or a quarter) is not evidence the two go together.
           AND (COALESCE(lh.completed, 0) = 1
                OR (COALESCE(lh.duration_listened_ms, 0) >= 30000
                    AND (t.duration_ms IS NULL OR t.duration_ms <= 0
                         OR COALESCE(lh.duration_listened_ms, 0) * 4 >= t.duration_ms)))
         ORDER BY lh.started_at ASC, lh.id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WeightedTrackPairRow {
                event_id: row.get(0)?,
                from_track_id: row.get(1)?,
                to_track_id: row.get(2)?,
                weight: row.get(3)?,
                source: row.get(4)?,
                completed_prev: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[allow(dead_code)]
pub fn get_listen_history_sequences(
    conn: &Connection,
    session_window_minutes: i64,
) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, started_at
         FROM listen_history
         ORDER BY started_at ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut sequences = Vec::new();
    let mut current = Vec::new();
    let mut previous_at: Option<chrono::DateTime<chrono::Utc>> = None;
    for (track_id, started_at) in rows {
        let parsed = chrono::DateTime::parse_from_rfc3339(&format!(
            "{}{}",
            started_at,
            if started_at.ends_with('Z') { "" } else { "Z" }
        ))
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(&started_at, "%Y-%m-%d %H:%M:%S")
                .map(|dt| dt.and_utc())
        })
        .ok();
        if let Some(prev) = previous_at {
            if let Some(next) = parsed {
                if (next - prev).num_minutes() > session_window_minutes {
                    if current.len() > 1 {
                        sequences.push(current.clone());
                    }
                    current.clear();
                }
                previous_at = Some(next);
            }
        } else if let Some(next) = parsed {
            previous_at = Some(next);
        }
        current.push(track_id);
    }
    if current.len() > 1 {
        sequences.push(current);
    }
    Ok(sequences)
}

pub fn get_playlist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT playlist_id, track_id
         FROM playlist_tracks
         ORDER BY playlist_id ASC, position ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (playlist_id, track_id) in rows {
        grouped.entry(playlist_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_album_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT album_id, id
         FROM tracks
         WHERE album_id IS NOT NULL
         ORDER BY album_id ASC, disc_number ASC, track_number ASC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (album_id, track_id) in rows {
        grouped.entry(album_id).or_default().push(track_id);
    }
    Ok(grouped.into_values().filter(|seq| seq.len() > 1).collect())
}

pub fn get_artist_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT artist_id, id
         FROM tracks
         WHERE artist_id IS NOT NULL
         ORDER BY artist_id ASC, play_count DESC, id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (artist_id, track_id) in rows {
        grouped.entry(artist_id).or_default().push(track_id);
    }
    // Truncate large artists rather than dropping them — an artist with 200 tracks
    // should still contribute co-occurrence signal for the tracks it includes.
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80); // keep top-80 by play_count (already sorted DESC)
        }
    }
    Ok(seqs)
}

pub fn get_genre_sequences(conn: &Connection) -> Result<Vec<Vec<i64>>> {
    let mut stmt = conn.prepare(
        "SELECT tg.genre_id, tg.track_id
         FROM track_genres tg
         JOIN tracks t ON t.id = tg.track_id
         ORDER BY tg.genre_id ASC, t.play_count DESC, t.id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for (genre_id, track_id) in rows {
        grouped.entry(genre_id).or_default().push(track_id);
    }
    let mut seqs: Vec<Vec<i64>> = grouped.into_values().filter(|seq| seq.len() > 1).collect();
    for seq in &mut seqs {
        if seq.len() > 80 {
            seq.truncate(80);
        }
    }
    Ok(seqs)
}

pub fn get_favorite_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn
        .prepare("SELECT id FROM tracks WHERE is_favorite = 1 ORDER BY play_count DESC, id ASC")?;
    stmt.query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}
