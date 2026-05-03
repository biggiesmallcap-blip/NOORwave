// Tier 2 radio configuration: feature flags, internal profile struct, and the
// diagnostics writer. Kept out of radio.rs to keep that file focused on
// orchestration logic.
//
// Behavior model:
//   - All five behavior flags + the kill-switch are stored in `server_config`.
//     orchestrate_song reads them once per request via `load_radio_flags`.
//   - Each flag corresponds to a single Tier 2 surgery; flags are independent so
//     an operator can enable normalization without enabling the diversity
//     re-ranker, and vice versa.
//   - `radio_use_legacy_pipeline` short-circuits the entire new code path —
//     intended as a one-flip emergency rollback that requires no deploy.
//   - `RadioProfile` is the internal knob bag that consolidates what used to be
//     scattered constants. Stays internal until the presets prove out in
//     production; UI keeps the 3-button selector for now.

use anyhow::Result;
use rusqlite::{Connection, params};

use super::radio::{RadioBlend, RadioSource};

#[derive(Debug, Clone, Copy, Default)]
pub struct RadioFlags {
    pub use_legacy_pipeline: bool,
    pub score_normalization_enabled: bool,
    pub confidence_penalty_enabled: bool,
    pub hub_penalty_enabled: bool,
    pub diversity_rerank_enabled: bool,
    pub source_quota_bonus_enabled: bool,
}

impl RadioFlags {
    pub const fn all_off() -> Self {
        Self {
            use_legacy_pipeline: false,
            score_normalization_enabled: false,
            confidence_penalty_enabled: false,
            hub_penalty_enabled: false,
            diversity_rerank_enabled: false,
            source_quota_bonus_enabled: false,
        }
    }
}

fn read_bool_flag(conn: &Connection, key: &str) -> bool {
    conn.query_row(
        "SELECT value FROM server_config WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    .unwrap_or(false)
}

pub fn load_radio_flags(conn: &Connection) -> RadioFlags {
    RadioFlags {
        use_legacy_pipeline: read_bool_flag(conn, "radio_use_legacy_pipeline"),
        score_normalization_enabled: read_bool_flag(conn, "radio_score_normalization_enabled"),
        confidence_penalty_enabled: read_bool_flag(conn, "radio_confidence_penalty_enabled"),
        hub_penalty_enabled: read_bool_flag(conn, "radio_hub_penalty_enabled"),
        diversity_rerank_enabled: read_bool_flag(conn, "radio_diversity_rerank_enabled"),
        source_quota_bonus_enabled: read_bool_flag(conn, "radio_source_quota_bonus_enabled"),
    }
}

// All Tier 2 tunable knobs in one place. Defaults reproduce the legacy behavior
// when their corresponding flags are off; once flags are enabled, the knobs
// shape the new behaviors. `max_genre_distance` is reserved for future genre-
// distance gating and stays None in this round.
#[derive(Debug, Clone)]
pub struct RadioProfile {
    pub blend: RadioBlend,
    pub library_weight: f64,
    pub lastfm_weight: f64,
    pub engine_weight: f64,
    pub creativity: f64,
    pub min_confidence: f64,
    pub diversity_weight: f64,
    pub same_artist_penalty: f64,
    pub same_album_penalty: f64,
    pub genre_saturation_penalty: f64,
    pub hub_penalty: f64,
    pub max_genre_distance: Option<f64>,
    pub novelty_weight: f64,
}

// ─── Tuning runbook ──────────────────────────────────────────────────────────
// The RadioProfile defaults below are starting points, not final values. Once
// `radio_diagnostics` has accumulated 2-4 weeks of data with all five Tier 2
// flags on, tune by inspecting:
//
//   1. avg_confidence per profile_name. If one profile sits below ~0.5,
//      bump that profile's `min_confidence` *down* — the threshold is firing
//      too aggressively and dropping otherwise-fine candidates.
//
//        SELECT profile_name, AVG(avg_confidence)
//        FROM radio_diagnostics
//        WHERE confidence_penalty_enabled = 1
//        GROUP BY profile_name;
//
//   2. avg_candidate_in_degree_pct per profile. Familiar should be lowest
//      (anchored to popular tracks is fine), Adventurous highest. If
//      Adventurous looks similar to Familiar, raise its `hub_penalty`.
//
//        SELECT profile_name, AVG(avg_candidate_in_degree_pct)
//        FROM radio_diagnostics
//        WHERE hub_penalty_enabled = 1
//        GROUP BY profile_name;
//
//   3. Penalty-counter ratios. same_artist_penalties / queue_size > 0.30
//      means the rerank is fighting the candidate pool too hard — either
//      raise diversity_weight (more rejection of same-artist) or lower it
//      (accept more clustering). penalty_relaxations > 0.10 of queues means
//      the candidate pool is consistently too narrow; widen the upstream
//      lib_target factor or raise lastfm/engine recall.
//
//        SELECT profile_name,
//               SUM(same_artist_penalties) * 1.0 / SUM(queue_size) AS artist_rate,
//               SUM(penalty_relaxations) * 1.0 / COUNT(*)         AS relax_rate
//        FROM radio_diagnostics
//        WHERE diversity_rerank_enabled = 1
//        GROUP BY profile_name;
//
//   4. Per-reason hit-rates from `discovery_diagnostics`. If `harmonic_match`
//      shows hit_rate < 0.05 while `behavioral` shows > 0.20, the harmonic
//      bonus is ornamental — its 0.14 metadata-score weight in the trainer
//      should drop. Open a follow-up to tune trainer constants, not these.
//
//        SELECT primary_reason, hit_rate, impressions
//        FROM discovery_diagnostics
//        WHERE insufficient_data = 0
//        ORDER BY hit_rate DESC;
//
// Don't tune from a single seed-run; aggregate over ≥1000 radio_diagnostics
// rows per profile before adjusting. Single-flip changes between deploys
// are easier to attribute than batched edits.

impl RadioProfile {
    pub fn from_blend(blend: RadioBlend) -> Self {
        let (library_weight, lastfm_weight, engine_weight) = blend.weights();
        let creativity = match blend {
            RadioBlend::Familiar => 0.15,
            RadioBlend::Mixed => 0.30,
            RadioBlend::Adventurous => 0.50,
        };
        // Same-artist / same-album / genre_saturation penalties only fire when
        // diversity_rerank_enabled, so these defaults are inert until then.
        // min_confidence default 0.4 is the fused-edge floor — penalty applies
        // to library candidates below that threshold, leaving cold-start
        // edges (0.25 floor) unaffected.
        let (same_artist_penalty, same_album_penalty, genre_saturation_penalty, hub_penalty) =
            match blend {
                RadioBlend::Familiar => (0.20, 0.10, 0.10, 0.20),
                RadioBlend::Mixed => (0.30, 0.15, 0.15, 0.35),
                RadioBlend::Adventurous => (0.40, 0.20, 0.25, 0.50),
            };
        Self {
            blend,
            library_weight,
            lastfm_weight,
            engine_weight,
            creativity,
            min_confidence: 0.4,
            diversity_weight: 1.0,
            same_artist_penalty,
            same_album_penalty,
            genre_saturation_penalty,
            hub_penalty,
            max_genre_distance: None,
            novelty_weight: 0.0,
        }
    }

    pub fn familiar() -> Self {
        Self::from_blend(RadioBlend::Familiar)
    }
    pub fn mixed() -> Self {
        Self::from_blend(RadioBlend::Mixed)
    }
    pub fn adventurous() -> Self {
        Self::from_blend(RadioBlend::Adventurous)
    }

    pub fn name(&self) -> &'static str {
        match self.blend {
            RadioBlend::Familiar => "familiar",
            RadioBlend::Mixed => "mixed",
            RadioBlend::Adventurous => "adventurous",
        }
    }
}

// One row recorded per orchestrate_song call (when the new pipeline runs).
// Source counts and penalty counters tally what the new pipeline did, so an
// operator can verify post-hoc whether a flag actually affected the queue.
#[derive(Debug, Clone, Default)]
pub struct RadioDiagnosticsRow {
    pub seed_track_id: Option<i64>,
    pub profile_name: String,
    pub creativity: f64,
    pub queue_size: i64,
    pub target_library_weight: f64,
    pub target_lastfm_weight: f64,
    pub target_engine_weight: f64,
    pub actual_library_count: i64,
    pub actual_lastfm_count: i64,
    pub actual_engine_count: i64,
    pub avg_confidence: Option<f64>,
    pub avg_candidate_in_degree_pct: Option<f64>,
    pub same_artist_penalties: i64,
    pub same_album_penalties: i64,
    pub genre_saturation_penalties: i64,
    pub repetition_skips: i64,
    pub penalty_relaxations: i64,
    pub hub_penalty_total: f64,
    pub flags: RadioFlags,
}

impl RadioDiagnosticsRow {
    pub fn count_source(&mut self, source: RadioSource) {
        match source {
            RadioSource::Library => self.actual_library_count += 1,
            RadioSource::Lastfm => self.actual_lastfm_count += 1,
            RadioSource::Engine => self.actual_engine_count += 1,
        }
    }
}

pub fn log_radio_diagnostics(conn: &Connection, row: &RadioDiagnosticsRow) -> Result<()> {
    conn.execute(
        "INSERT INTO radio_diagnostics
         (seed_track_id, profile_name, creativity, queue_size,
          target_library_weight, target_lastfm_weight, target_engine_weight,
          actual_library_count, actual_lastfm_count, actual_engine_count,
          avg_confidence, avg_candidate_in_degree_pct,
          same_artist_penalties, same_album_penalties, genre_saturation_penalties,
          repetition_skips, penalty_relaxations, hub_penalty_total,
          normalization_enabled, confidence_penalty_enabled, hub_penalty_enabled,
          diversity_rerank_enabled, source_quota_bonus_enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                 ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
        params![
            row.seed_track_id,
            row.profile_name,
            row.creativity,
            row.queue_size,
            row.target_library_weight,
            row.target_lastfm_weight,
            row.target_engine_weight,
            row.actual_library_count,
            row.actual_lastfm_count,
            row.actual_engine_count,
            row.avg_confidence,
            row.avg_candidate_in_degree_pct,
            row.same_artist_penalties,
            row.same_album_penalties,
            row.genre_saturation_penalties,
            row.repetition_skips,
            row.penalty_relaxations,
            row.hub_penalty_total,
            row.flags.score_normalization_enabled as i32,
            row.flags.confidence_penalty_enabled as i32,
            row.flags.hub_penalty_enabled as i32,
            row.flags.diversity_rerank_enabled as i32,
            row.flags.source_quota_bonus_enabled as i32,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    #[test]
    fn flags_default_to_all_false_after_migrations() {
        let conn = Connection::open_in_memory().expect("memory db");
        schema::run_migrations(&conn).expect("migrations");
        let flags = load_radio_flags(&conn);
        assert!(!flags.use_legacy_pipeline);
        assert!(!flags.score_normalization_enabled);
        assert!(!flags.confidence_penalty_enabled);
        assert!(!flags.hub_penalty_enabled);
        assert!(!flags.diversity_rerank_enabled);
        assert!(!flags.source_quota_bonus_enabled);
    }

    #[test]
    fn flag_round_trips_through_server_config() {
        let conn = Connection::open_in_memory().expect("memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn.execute(
            "UPDATE server_config SET value = '1' WHERE key = 'radio_score_normalization_enabled'",
            [],
        )
        .expect("update");
        let flags = load_radio_flags(&conn);
        assert!(flags.score_normalization_enabled);
        assert!(!flags.diversity_rerank_enabled);
    }

    #[test]
    fn profile_from_blend_carries_weights_and_creativity() {
        let p = RadioProfile::from_blend(RadioBlend::Familiar);
        assert_eq!(p.name(), "familiar");
        assert!((p.library_weight - 0.60).abs() < 1e-9);
        assert!((p.creativity - 0.15).abs() < 1e-9);
        let total = p.library_weight + p.lastfm_weight + p.engine_weight;
        assert!((total - 1.0).abs() < 1e-9, "weights sum {}", total);
    }

    #[test]
    fn diagnostics_round_trip() {
        let conn = Connection::open_in_memory().expect("memory db");
        schema::run_migrations(&conn).expect("migrations");
        let mut row = RadioDiagnosticsRow {
            seed_track_id: Some(42),
            profile_name: "mixed".to_string(),
            creativity: 0.30,
            queue_size: 20,
            target_library_weight: 0.30,
            target_lastfm_weight: 0.40,
            target_engine_weight: 0.30,
            avg_confidence: Some(0.65),
            avg_candidate_in_degree_pct: Some(0.42),
            ..Default::default()
        };
        row.flags = RadioFlags {
            use_legacy_pipeline: false,
            score_normalization_enabled: true,
            ..RadioFlags::all_off()
        };
        row.count_source(RadioSource::Library);
        row.count_source(RadioSource::Library);
        row.count_source(RadioSource::Lastfm);
        log_radio_diagnostics(&conn, &row).expect("log");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM radio_diagnostics WHERE seed_track_id = 42",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let actual_lib: i64 = conn
            .query_row(
                "SELECT actual_library_count FROM radio_diagnostics WHERE seed_track_id = 42",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actual_lib, 2);
    }
}
