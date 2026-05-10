//! Galaxy display-filter rules.
//!
//! `track_genres` holds the rich multi-source tag set. Most consumers (radio
//! coherence, discovery, queue decoration) read it raw because they want the
//! richness — they do their own weighting. The Genre Galaxy and its
//! supporting endpoints are different: when a track has a low-confidence
//! Last.fm tag like `psychedelic rock` (community shorthand for "trippy")
//! that name-matches a real taxonomy node, treating it as full cluster
//! membership pollutes the Rock galaxy with psytrance acts.
//!
//! These rules let the galaxy queries inject a `WHERE` fragment that filters
//! `track_genres` rows down to the trustworthy ones before any aggregation or
//! ancestry walk happens.
//!
//! Default is [`GalaxyFilterRule::ConfidenceMin`] at 0.5 — preserves rich
//! tracks (Mac Miller still spans Hip-Hop / Pop / Cloud Rap / Trap) while
//! dropping borderline community noise.

use std::borrow::Cow;

/// Filter applied to `track_genres` rows before galaxy aggregation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GalaxyFilterRule {
    /// No filtering — every row counts. Pre-refactor behavior.
    All,
    /// Keep only rows whose `confidence` is at least the given value. Strict —
    /// tracks where every tag falls below the floor become galaxy-invisible.
    ConfidenceMin(f64),
    /// Same floor as [`Self::ConfidenceMin`], but tracks where *every* tag is
    /// below the floor still contribute their single strongest tag instead of
    /// vanishing entirely. Trades a small amount of cluster noise for keeping
    /// coverage-poor artists (Khruangbin, etc.) findable in the galaxy.
    ConfidenceMinWithRescue(f64),
    /// Keep only the top-N highest-confidence rows per `track_id`. Tie-broken
    /// by source priority (MB > Spotify > Last.fm) then `genre_id`.
    TopNPerTrack(u32),
    /// Keep only rows from the `musicbrainz` source.
    MbOnly,
    /// Keep only the single strongest tag per track — same pick rule as the
    /// `track_primary_genre` view.
    PrimaryOnly,
}

impl GalaxyFilterRule {
    /// Default rule used when no filter is explicitly requested. The rescue
    /// variant is the default because the strict variant orphans artists
    /// whose every tag falls below 0.5 (a common shape — see
    /// docs/genre-data-quality-2026-05-07.md, Khruangbin case).
    pub const fn default_rule() -> Self {
        GalaxyFilterRule::ConfidenceMinWithRescue(0.5)
    }

    /// Parse a query-string token into a rule. Unknown tokens fall back to
    /// the default rather than erroring — this is exposed on read-only
    /// galaxy endpoints where bad input shouldn't 400.
    ///
    /// Token convention: `conf05` = rescue-enabled at 0.5; `conf05_strict`
    /// = strict floor with no rescue. Pre-rescue callers using the bare
    /// `conf05` token are silently upgraded to the rescue variant.
    pub fn from_query(value: Option<&str>) -> Self {
        match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "" => Self::default_rule(),
            "all" | "raw" => GalaxyFilterRule::All,
            "conf05" | "confidence_0_5" => GalaxyFilterRule::ConfidenceMinWithRescue(0.5),
            "conf07" | "confidence_0_7" => GalaxyFilterRule::ConfidenceMinWithRescue(0.7),
            "conf05_strict" | "confidence_0_5_strict" => GalaxyFilterRule::ConfidenceMin(0.5),
            "conf07_strict" | "confidence_0_7_strict" => GalaxyFilterRule::ConfidenceMin(0.7),
            "top2" => GalaxyFilterRule::TopNPerTrack(2),
            "top3" => GalaxyFilterRule::TopNPerTrack(3),
            "mb_only" | "mbonly" | "musicbrainz" => GalaxyFilterRule::MbOnly,
            "primary" | "primary_only" => GalaxyFilterRule::PrimaryOnly,
            _ => Self::default_rule(),
        }
    }

    /// Stable label for the rule — used in API responses so callers can echo
    /// back which rule was actually applied.
    pub fn label(self) -> Cow<'static, str> {
        match self {
            GalaxyFilterRule::All => Cow::Borrowed("all"),
            GalaxyFilterRule::ConfidenceMin(min) => {
                Cow::Owned(format!("confidence_{:.2}_strict", min).replace('.', "_"))
            }
            GalaxyFilterRule::ConfidenceMinWithRescue(min) => {
                Cow::Owned(format!("confidence_{:.2}", min).replace('.', "_"))
            }
            GalaxyFilterRule::TopNPerTrack(n) => Cow::Owned(format!("top{}", n)),
            GalaxyFilterRule::MbOnly => Cow::Borrowed("mb_only"),
            GalaxyFilterRule::PrimaryOnly => Cow::Borrowed("primary_only"),
        }
    }
}

/// Returns a SQL fragment producing the filtered `track_genres` rowset.
///
/// Callers use this in place of the raw `track_genres` table reference, e.g.
/// ```sql
/// FROM ({fragment}) tg JOIN genres g ON g.id = tg.genre_id
/// ```
///
/// The fragment is always a self-contained subquery yielding the same
/// `(track_id, genre_id, source, confidence)` shape as `track_genres`. No
/// user-supplied data flows into the fragment — values come from a closed
/// enum — so this is not an injection risk.
pub fn filter_subquery(rule: GalaxyFilterRule) -> Cow<'static, str> {
    match rule {
        GalaxyFilterRule::All => {
            Cow::Borrowed("SELECT track_id, genre_id, source, confidence FROM track_genres")
        }
        GalaxyFilterRule::ConfidenceMin(min) => Cow::Owned(format!(
            "SELECT track_id, genre_id, source, confidence FROM track_genres WHERE confidence >= {:.4}",
            min.clamp(0.0, 10.0)
        )),
        GalaxyFilterRule::ConfidenceMinWithRescue(min) => {
            // Two-step union expressed as a single window-pass:
            //   1. confidence >= min — normal floor
            //   2. OR (track's max confidence is below min AND this is the
            //      strongest tag on the track) — rescue branch, fires only
            //      when the WHOLE track is below threshold.
            // Tracks with at least one tag clearing the floor get *just* their
            // qualifying tags (not the floor + rescue together), so noise on
            // well-tagged tracks isn't re-admitted.
            let clamped = min.clamp(0.0, 10.0);
            Cow::Owned(format!(
                "SELECT track_id, genre_id, source, confidence FROM (\
                    SELECT track_id, genre_id, source, confidence, \
                        ROW_NUMBER() OVER (\
                            PARTITION BY track_id \
                            ORDER BY confidence DESC, \
                                CASE source WHEN 'musicbrainz' THEN 1 WHEN 'spotify' THEN 2 WHEN 'lastfm' THEN 3 ELSE 9 END, \
                                genre_id\
                        ) AS rn, \
                        MAX(confidence) OVER (PARTITION BY track_id) AS track_max_conf \
                    FROM track_genres\
                 ) WHERE confidence >= {clamped:.4} \
                       OR (track_max_conf < {clamped:.4} AND rn = 1)"
            ))
        }
        GalaxyFilterRule::TopNPerTrack(n) => Cow::Owned(format!(
            "SELECT track_id, genre_id, source, confidence FROM (\
                SELECT track_id, genre_id, source, confidence, \
                    ROW_NUMBER() OVER (\
                        PARTITION BY track_id \
                        ORDER BY confidence DESC, \
                            CASE source WHEN 'musicbrainz' THEN 1 WHEN 'spotify' THEN 2 WHEN 'lastfm' THEN 3 ELSE 9 END, \
                            genre_id\
                    ) AS rn \
                FROM track_genres\
             ) WHERE rn <= {}",
            n.max(1)
        )),
        GalaxyFilterRule::MbOnly => Cow::Borrowed(
            "SELECT track_id, genre_id, source, confidence FROM track_genres WHERE source = 'musicbrainz'",
        ),
        GalaxyFilterRule::PrimaryOnly => Cow::Borrowed(
            "SELECT track_id, primary_genre_id AS genre_id, source, confidence FROM track_primary_genre",
        ),
    }
}

/// Album-tier fallback confidence multiplier. Applied to the strongest sibling
/// confidence per (track, genre). 0.6 lands fallback rows just above
/// `ConfidenceMinWithRescue(0.5)` for sibling confidence ≥ 0.83 — strong
/// signals propagate, weak ones don't pollute strict clusters.
pub const ALBUM_FALLBACK_CONF: f64 = 0.6;

/// Artist-tier fallback confidence multiplier. Half of album, reflecting the
/// looser coherence of "another track by this artist" vs. "another track on
/// this album."
pub const ARTIST_FALLBACK_CONF: f64 = 0.4;

/// Maximum fallback rows admitted per track per tier. Without a cap, an album
/// with 12 tagged genres dumps 12 rows onto an empty sibling — wide genre sets
/// inflate the Phase-2b ancestor bonus artificially. Top-3 carries the
/// dominant signals (root + leaf + one secondary) without widening the path
/// set beyond a real track's typical breadth.
pub const FALLBACK_ROWS_PER_TRACK: u32 = 3;

/// Returns a SQL fragment that wraps [`filter_subquery`] with album-then-artist
/// fallback. Tracks whose inner-rule output is empty get rescued from siblings
/// on the same album (skipping multi-artist compilations) or, failing that,
/// from other tracks by the same artist. Top-[`FALLBACK_ROWS_PER_TRACK`] rows
/// per tier per track, source-priority tie-broken (MB > Spotify > Last.fm).
///
/// Sibling rows are taken from the inner rule's filtered output — so
/// `filter_subquery_with_fallback(MbOnly)` gives MB-only sibling material on
/// both tiers, no cross-source bleed.
///
/// Emitted fallback rows carry `source = 'album_fallback'` /
/// `'artist_fallback'`, distinct from underlying sibling sources, so
/// downstream consumers can identify rescued rows.
///
/// Output shape matches `filter_subquery`:
/// `(track_id, genre_id, source, confidence)`. Caller embeds the same way.
///
/// **Whole-library cost.** `needs_fallback` scans `tracks` end-to-end. For
/// narrow batches (radio's typical 30–60 candidates), use
/// [`filter_subquery_with_fallback_for_tracks`] instead, which inlines an
/// `IN (?,?,...)` filter so SQLite never touches non-requested tracks.
pub fn filter_subquery_with_fallback(rule: GalaxyFilterRule) -> Cow<'static, str> {
    let inner_sql = filter_subquery(rule);
    let album_mult = ALBUM_FALLBACK_CONF;
    let artist_mult = ARTIST_FALLBACK_CONF;
    let cap = FALLBACK_ROWS_PER_TRACK.max(1);
    Cow::Owned(format!(
        "WITH filtered AS ({inner_sql}), \
         mixed_albums AS (\
            SELECT album_id FROM tracks \
            WHERE album_id IS NOT NULL \
            GROUP BY album_id HAVING COUNT(DISTINCT artist_id) > 1\
         ), \
         needs_fallback AS (\
            SELECT t.id AS track_id, t.album_id, t.artist_id \
            FROM tracks t \
            WHERE NOT EXISTS (SELECT 1 FROM filtered f WHERE f.track_id = t.id)\
         ), \
         album_fallback_raw AS (\
            SELECT \
                nf.track_id, \
                f.genre_id, \
                MAX(f.confidence) AS sibling_max_conf, \
                MIN(CASE f.source \
                        WHEN 'musicbrainz' THEN 1 \
                        WHEN 'spotify' THEN 2 \
                        WHEN 'lastfm' THEN 3 \
                        ELSE 9 END) AS best_source_rank \
            FROM needs_fallback nf \
            JOIN tracks sib ON sib.album_id = nf.album_id AND sib.id != nf.track_id \
            JOIN filtered f ON f.track_id = sib.id \
            WHERE nf.album_id IS NOT NULL \
              AND nf.album_id NOT IN (SELECT album_id FROM mixed_albums) \
            GROUP BY nf.track_id, f.genre_id\
         ), \
         album_fallback_ranked AS (\
            SELECT \
                track_id, \
                genre_id, \
                'album_fallback' AS source, \
                sibling_max_conf * {album_mult} AS confidence, \
                ROW_NUMBER() OVER (\
                    PARTITION BY track_id \
                    ORDER BY sibling_max_conf DESC, best_source_rank ASC, genre_id ASC\
                ) AS rn \
            FROM album_fallback_raw\
         ), \
         album_fallback AS (\
            SELECT track_id, genre_id, source, confidence \
            FROM album_fallback_ranked WHERE rn <= {cap}\
         ), \
         still_needs AS (\
            SELECT nf.track_id, nf.artist_id \
            FROM needs_fallback nf \
            WHERE NOT EXISTS (SELECT 1 FROM album_fallback af WHERE af.track_id = nf.track_id)\
         ), \
         artist_fallback_raw AS (\
            SELECT \
                sn.track_id, \
                f.genre_id, \
                MAX(f.confidence) AS sibling_max_conf, \
                MIN(CASE f.source \
                        WHEN 'musicbrainz' THEN 1 \
                        WHEN 'spotify' THEN 2 \
                        WHEN 'lastfm' THEN 3 \
                        ELSE 9 END) AS best_source_rank \
            FROM still_needs sn \
            JOIN tracks sib ON sib.artist_id = sn.artist_id AND sib.id != sn.track_id \
            JOIN filtered f ON f.track_id = sib.id \
            GROUP BY sn.track_id, f.genre_id\
         ), \
         artist_fallback_ranked AS (\
            SELECT \
                track_id, \
                genre_id, \
                'artist_fallback' AS source, \
                sibling_max_conf * {artist_mult} AS confidence, \
                ROW_NUMBER() OVER (\
                    PARTITION BY track_id \
                    ORDER BY sibling_max_conf DESC, best_source_rank ASC, genre_id ASC\
                ) AS rn \
            FROM artist_fallback_raw\
         ), \
         artist_fallback AS (\
            SELECT track_id, genre_id, source, confidence \
            FROM artist_fallback_ranked WHERE rn <= {cap}\
         ) \
         SELECT track_id, genre_id, source, confidence FROM filtered \
         UNION ALL \
         SELECT track_id, genre_id, source, confidence FROM album_fallback \
         UNION ALL \
         SELECT track_id, genre_id, source, confidence FROM artist_fallback"
    ))
}

/// Variant of [`filter_subquery_with_fallback`] tuned for narrow per-track
/// queries (radio coherence, ad-hoc genre lookups). Inlines an
/// `IN (?,?,...)` filter into `needs_fallback` and the final primary-row
/// selection so SQLite never enumerates tracks the caller didn't ask for.
///
/// `placeholder_count` controls how many `?` parameters the caller will
/// bind. The SQL embeds them in a `requested(id) AS (VALUES (?),(?),...)`
/// CTE referenced by both the primary-row branch and `needs_fallback`,
/// so each id is bound exactly once.
///
/// Use for radio's 30–60-candidate Jaccard pass and similar narrow lookups.
/// For galaxy clustering and whole-library exports use the unfiltered form.
pub fn filter_subquery_with_fallback_for_tracks(
    rule: GalaxyFilterRule,
    placeholder_count: usize,
) -> Cow<'static, str> {
    if placeholder_count == 0 {
        // Empty IN-list would parse to a syntax error and is meaningless
        // anyway — match shape of the unfiltered cascade so the caller can
        // safely substitute regardless of count. Caller will bind nothing.
        return Cow::Borrowed(
            "SELECT NULL AS track_id, NULL AS genre_id, NULL AS source, NULL AS confidence WHERE 0",
        );
    }
    let inner_sql = filter_subquery(rule);
    let album_mult = ALBUM_FALLBACK_CONF;
    let artist_mult = ARTIST_FALLBACK_CONF;
    let cap = FALLBACK_ROWS_PER_TRACK.max(1);
    let values_list = std::iter::repeat_n("(?)", placeholder_count)
        .collect::<Vec<_>>()
        .join(",");
    Cow::Owned(format!(
        "WITH requested(id) AS (VALUES {values_list}), \
         filtered AS ({inner_sql}), \
         mixed_albums AS (\
            SELECT album_id FROM tracks \
            WHERE album_id IS NOT NULL \
            GROUP BY album_id HAVING COUNT(DISTINCT artist_id) > 1\
         ), \
         needs_fallback AS (\
            SELECT t.id AS track_id, t.album_id, t.artist_id \
            FROM tracks t \
            JOIN requested r ON r.id = t.id \
            WHERE NOT EXISTS (SELECT 1 FROM filtered f WHERE f.track_id = t.id)\
         ), \
         album_fallback_raw AS (\
            SELECT \
                nf.track_id, \
                f.genre_id, \
                MAX(f.confidence) AS sibling_max_conf, \
                MIN(CASE f.source \
                        WHEN 'musicbrainz' THEN 1 \
                        WHEN 'spotify' THEN 2 \
                        WHEN 'lastfm' THEN 3 \
                        ELSE 9 END) AS best_source_rank \
            FROM needs_fallback nf \
            JOIN tracks sib ON sib.album_id = nf.album_id AND sib.id != nf.track_id \
            JOIN filtered f ON f.track_id = sib.id \
            WHERE nf.album_id IS NOT NULL \
              AND nf.album_id NOT IN (SELECT album_id FROM mixed_albums) \
            GROUP BY nf.track_id, f.genre_id\
         ), \
         album_fallback_ranked AS (\
            SELECT \
                track_id, \
                genre_id, \
                'album_fallback' AS source, \
                sibling_max_conf * {album_mult} AS confidence, \
                ROW_NUMBER() OVER (\
                    PARTITION BY track_id \
                    ORDER BY sibling_max_conf DESC, best_source_rank ASC, genre_id ASC\
                ) AS rn \
            FROM album_fallback_raw\
         ), \
         album_fallback AS (\
            SELECT track_id, genre_id, source, confidence \
            FROM album_fallback_ranked WHERE rn <= {cap}\
         ), \
         still_needs AS (\
            SELECT nf.track_id, nf.artist_id \
            FROM needs_fallback nf \
            WHERE NOT EXISTS (SELECT 1 FROM album_fallback af WHERE af.track_id = nf.track_id)\
         ), \
         artist_fallback_raw AS (\
            SELECT \
                sn.track_id, \
                f.genre_id, \
                MAX(f.confidence) AS sibling_max_conf, \
                MIN(CASE f.source \
                        WHEN 'musicbrainz' THEN 1 \
                        WHEN 'spotify' THEN 2 \
                        WHEN 'lastfm' THEN 3 \
                        ELSE 9 END) AS best_source_rank \
            FROM still_needs sn \
            JOIN tracks sib ON sib.artist_id = sn.artist_id AND sib.id != sn.track_id \
            JOIN filtered f ON f.track_id = sib.id \
            GROUP BY sn.track_id, f.genre_id\
         ), \
         artist_fallback_ranked AS (\
            SELECT \
                track_id, \
                genre_id, \
                'artist_fallback' AS source, \
                sibling_max_conf * {artist_mult} AS confidence, \
                ROW_NUMBER() OVER (\
                    PARTITION BY track_id \
                    ORDER BY sibling_max_conf DESC, best_source_rank ASC, genre_id ASC\
                ) AS rn \
            FROM artist_fallback_raw\
         ), \
         artist_fallback AS (\
            SELECT track_id, genre_id, source, confidence \
            FROM artist_fallback_ranked WHERE rn <= {cap}\
         ) \
         SELECT f.track_id, f.genre_id, f.source, f.confidence \
         FROM filtered f JOIN requested r ON r.id = f.track_id \
         UNION ALL \
         SELECT track_id, genre_id, source, confidence FROM album_fallback \
         UNION ALL \
         SELECT track_id, genre_id, source, confidence FROM artist_fallback"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_query_handles_known_tokens() {
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("all")),
            GalaxyFilterRule::All
        ));
        // Bare `conf05` upgrades to the rescue variant — that's the new default.
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("conf05")),
            GalaxyFilterRule::ConfidenceMinWithRescue(v) if (v - 0.5).abs() < 1e-9
        ));
        // `_strict` suffix opts back into the no-rescue floor.
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("conf05_strict")),
            GalaxyFilterRule::ConfidenceMin(v) if (v - 0.5).abs() < 1e-9
        ));
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("top2")),
            GalaxyFilterRule::TopNPerTrack(2)
        ));
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("primary")),
            GalaxyFilterRule::PrimaryOnly
        ));
    }

    #[test]
    fn from_query_unknown_falls_back_to_default() {
        assert_eq!(
            GalaxyFilterRule::from_query(Some("nonsense")),
            GalaxyFilterRule::default_rule(),
        );
        assert_eq!(
            GalaxyFilterRule::from_query(None),
            GalaxyFilterRule::default_rule(),
        );
    }

    #[test]
    fn filter_subquery_for_all_matches_table_shape() {
        let frag = filter_subquery(GalaxyFilterRule::All);
        assert!(frag.contains("FROM track_genres"));
        assert!(!frag.contains("WHERE"));
    }

    #[test]
    fn filter_subquery_clamps_confidence_input() {
        // Out-of-range floats can't blow up the SQL.
        let frag = filter_subquery(GalaxyFilterRule::ConfidenceMin(99.0));
        assert!(frag.contains(">= 10."));
        let frag2 = filter_subquery(GalaxyFilterRule::ConfidenceMinWithRescue(99.0));
        assert!(frag2.contains(">= 10."));
    }

    #[test]
    fn filter_subquery_topn_floors_at_one() {
        let frag = filter_subquery(GalaxyFilterRule::TopNPerTrack(0));
        assert!(frag.contains("rn <= 1"));
    }

    #[test]
    fn rescue_subquery_includes_both_branches() {
        let frag = filter_subquery(GalaxyFilterRule::ConfidenceMinWithRescue(0.5));
        assert!(frag.contains("track_max_conf"));
        assert!(frag.contains("rn = 1"));
        assert!(frag.contains("confidence >= 0.5000"));
    }

    #[test]
    fn label_is_stable() {
        assert_eq!(GalaxyFilterRule::All.label(), "all");
        assert_eq!(GalaxyFilterRule::MbOnly.label(), "mb_only");
        assert_eq!(GalaxyFilterRule::PrimaryOnly.label(), "primary_only");
        assert_eq!(GalaxyFilterRule::TopNPerTrack(2).label(), "top2");
        assert_eq!(
            GalaxyFilterRule::ConfidenceMinWithRescue(0.5).label(),
            "confidence_0_50"
        );
        assert_eq!(
            GalaxyFilterRule::ConfidenceMin(0.5).label(),
            "confidence_0_50_strict"
        );
    }

    #[test]
    fn fallback_subquery_emits_three_tier_union() {
        let frag = filter_subquery_with_fallback(GalaxyFilterRule::default_rule());
        // Inner rule SQL is wrapped as the `filtered` CTE.
        assert!(frag.contains("WITH filtered AS"));
        // Compilation-skip CTE.
        assert!(frag.contains("mixed_albums"));
        assert!(frag.contains("COUNT(DISTINCT artist_id) > 1"));
        // Both fallback tiers present, with their distinct source labels.
        assert!(frag.contains("'album_fallback'"));
        assert!(frag.contains("'artist_fallback'"));
        // Multiplicative discount expressed via the constants.
        assert!(frag.contains(&format!("* {}", ALBUM_FALLBACK_CONF)));
        assert!(frag.contains(&format!("* {}", ARTIST_FALLBACK_CONF)));
        // Top-N cap applied.
        assert!(frag.contains(&format!("rn <= {}", FALLBACK_ROWS_PER_TRACK)));
        // Three-way UNION ALL at the top.
        assert_eq!(frag.matches("UNION ALL").count(), 2);
    }

    #[test]
    fn fallback_wraps_inner_rule_sql() {
        // filter_subquery_with_fallback(MbOnly) must inline MbOnly's SQL inside
        // the `filtered` CTE so sibling rows feeding the fallback are also
        // MB-only — no cross-source bleed when the user asked for MB-only data.
        let frag = filter_subquery_with_fallback(GalaxyFilterRule::MbOnly);
        assert!(frag.contains("source = 'musicbrainz'"));
    }

    #[test]
    fn fallback_subquery_excludes_seed_track_from_siblings() {
        // The SQL must not let a track use itself as its own sibling — that
        // would let an empty-genre track stay empty (sibling pool = {self}
        // which is empty under track_genres).
        let frag = filter_subquery_with_fallback(GalaxyFilterRule::default_rule());
        assert!(frag.contains("sib.id != nf.track_id"));
        assert!(frag.contains("sib.id != sn.track_id"));
    }

    /// End-to-end: run the cascade SQL against an in-memory DB and assert
    /// that the right rescues fire. This is the integration test the audit
    /// asked for — string content alone can't catch a misshapen JOIN.
    #[test]
    fn fallback_cascade_rescues_correctly_against_in_memory_db() {
        use rusqlite::Connection;

        let conn = Connection::open_in_memory().expect("in-memory db");
        crate::db::schema::run_migrations(&conn).expect("migrations");

        // Three artists. Artist 1 has a coherent single-artist album where
        // one track has genres and one is empty (album rescue should fire).
        // Artist 2 has only one tagged track and one empty track on different
        // albums (artist rescue should fire). Artist 3 contributes to a
        // multi-artist compilation along with Artist 4 — empty tracks on the
        // comp must NOT inherit each other's genres.
        conn.execute_batch(
            "INSERT INTO genres (id, name, slug, parent_id) VALUES
                (1, 'Electronic', 'electronic', NULL),
                (2, 'Drum and Bass', 'drum-and-bass', 1),
                (3, 'Jazz', 'jazz', NULL),
                (4, 'Rock', 'rock', NULL);
             INSERT INTO artists (id, name) VALUES
                (1, 'CoherentArtist'),
                (2, 'PartiallyTaggedArtist'),
                (3, 'CompContributorA'),
                (4, 'CompContributorB');
             INSERT INTO albums (id, title, artist_id, source, is_favorite) VALUES
                (10, 'CoherentAlbum', 1, 'tidal', 1),
                (20, 'PartialAlbumA', 2, 'tidal', 0),
                (21, 'PartialAlbumB', 2, 'tidal', 0),
                (30, 'MultiArtistComp', 3, 'tidal', 1);
             INSERT INTO tracks
                (id, title, artist_id, album_id, duration_ms, best_quality, best_source, fidelity_score, source)
             VALUES
                (100, 'Tagged on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (101, 'Empty on coherent', 1, 10, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (200, 'Tagged on partial A', 2, 20, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (201, 'Empty on partial B', 2, 21, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (300, 'Tagged on comp (artist 3)', 3, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal'),
                (301, 'Empty on comp (artist 4)', 4, 30, 1000, 'LOSSLESS', 'tidal', 10, 'tidal');
             INSERT INTO track_genres (track_id, genre_id, source, confidence) VALUES
                (100, 2, 'musicbrainz', 1.0),
                (200, 3, 'musicbrainz', 0.9),
                (300, 4, 'lastfm', 0.7);",
        )
        .expect("seed fixtures");

        let frag = filter_subquery_with_fallback(GalaxyFilterRule::All);
        let sql = format!(
            "SELECT track_id, genre_id, source, confidence FROM ({frag}) tg ORDER BY track_id, genre_id"
        );

        let mut stmt = conn.prepare(&sql).expect("prepare cascade SQL");
        let rows: Vec<(i64, i64, String, f64)> = stmt
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("execute")
            .collect::<Result<_, _>>()
            .expect("collect");

        // Track 100: direct genre, untouched.
        assert!(
            rows.iter()
                .any(|(t, g, s, _)| *t == 100 && *g == 2 && s == "musicbrainz"),
            "track 100 should keep its direct genre",
        );

        // Track 101: empty, single-artist album sibling (track 100, genre 2)
        // available → album fallback fires with discount.
        let track_101: Vec<_> = rows.iter().filter(|(t, ..)| *t == 101).collect();
        assert_eq!(
            track_101.len(),
            1,
            "track 101 should get exactly one rescued row from album sibling"
        );
        assert_eq!(track_101[0].1, 2, "rescued genre id must be 2 (DnB)");
        assert_eq!(track_101[0].2, "album_fallback");
        assert!(
            (track_101[0].3 - 1.0 * ALBUM_FALLBACK_CONF).abs() < 1e-6,
            "album fallback confidence = sibling_max * {ALBUM_FALLBACK_CONF}, got {}",
            track_101[0].3
        );

        // Track 201: empty, no sibling on its album (album 21 has only itself),
        // but artist 2 has track 200 tagged → artist fallback fires.
        let track_201: Vec<_> = rows.iter().filter(|(t, ..)| *t == 201).collect();
        assert_eq!(
            track_201.len(),
            1,
            "track 201 should get exactly one rescued row from artist sibling"
        );
        assert_eq!(track_201[0].1, 3, "rescued genre id must be 3 (Jazz)");
        assert_eq!(track_201[0].2, "artist_fallback");
        assert!(
            (track_201[0].3 - 0.9 * ARTIST_FALLBACK_CONF).abs() < 1e-6,
            "artist fallback confidence = 0.9 * {ARTIST_FALLBACK_CONF}, got {}",
            track_201[0].3
        );

        // Track 301: empty, on the multi-artist comp (album 30, artists 3 & 4).
        // Album tier MUST skip this album. Artist 4 has no other tagged tracks
        // → no artist rescue either. Result: track 301 stays unrescued.
        let track_301: Vec<_> = rows.iter().filter(|(t, ..)| *t == 301).collect();
        assert!(
            track_301.is_empty(),
            "track 301 must NOT inherit album-mate genres on a multi-artist comp; got {track_301:?}"
        );
    }
}
