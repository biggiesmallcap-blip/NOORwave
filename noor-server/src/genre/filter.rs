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
    /// Keep only rows whose `confidence` is at least the given value.
    ConfidenceMin(f64),
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
    /// Default rule used when no filter is explicitly requested.
    pub const fn default_rule() -> Self {
        GalaxyFilterRule::ConfidenceMin(0.5)
    }

    /// Parse a query-string token into a rule. Unknown tokens fall back to
    /// the default rather than erroring — this is exposed on read-only
    /// galaxy endpoints where bad input shouldn't 400.
    pub fn from_query(value: Option<&str>) -> Self {
        match value.unwrap_or("").trim().to_ascii_lowercase().as_str() {
            "" => Self::default_rule(),
            "all" | "raw" => GalaxyFilterRule::All,
            "conf05" | "confidence_0_5" => GalaxyFilterRule::ConfidenceMin(0.5),
            "conf07" | "confidence_0_7" => GalaxyFilterRule::ConfidenceMin(0.7),
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
        GalaxyFilterRule::All => Cow::Borrowed("SELECT track_id, genre_id, source, confidence FROM track_genres"),
        GalaxyFilterRule::ConfidenceMin(min) => Cow::Owned(format!(
            "SELECT track_id, genre_id, source, confidence FROM track_genres WHERE confidence >= {:.4}",
            min.clamp(0.0, 10.0)
        )),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_query_handles_known_tokens() {
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("all")),
            GalaxyFilterRule::All
        ));
        assert!(matches!(
            GalaxyFilterRule::from_query(Some("conf05")),
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
    }

    #[test]
    fn filter_subquery_topn_floors_at_one() {
        let frag = filter_subquery(GalaxyFilterRule::TopNPerTrack(0));
        assert!(frag.contains("rn <= 1"));
    }

    #[test]
    fn label_is_stable() {
        assert_eq!(GalaxyFilterRule::All.label(), "all");
        assert_eq!(GalaxyFilterRule::MbOnly.label(), "mb_only");
        assert_eq!(GalaxyFilterRule::PrimaryOnly.label(), "primary_only");
        assert_eq!(GalaxyFilterRule::TopNPerTrack(2).label(), "top2");
    }
}
