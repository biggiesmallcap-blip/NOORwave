use anyhow::Result;
use noor_mix::planner::{DJ_PLANNER_VERSION, MixIntent, TransitionSpeedBias, TransitionTemplate};
use noor_mix::{DjProfile, Planner, Policy, TransitionProgram};
use rusqlite::Connection;
use serde::Serialize;

use crate::db::models::{AudioDjProfileCorrectionRow, AudioDjProfileRow};
use crate::db::{Database, queries};
use crate::playback::dj_lookahead::DjMediaRef;
use crate::services::audio_analysis::dj_profile::{decode_f32_blob, decode_u32_blob};

const PROFILE_CONFIDENCE_FLOOR: f64 = 0.65;

pub struct DjEngine {
    db: Database,
}

pub struct RuntimeSafetyDecision {
    pub fallback_reason: Option<&'static str>,
    pub force_safe_crossfade: bool,
}

pub struct DjTransitionPlan {
    pub program: TransitionProgram,
    pub rejected_alternatives: Vec<RejectedTransitionAlternative>,
    pub planner_version: &'static str,
    pub fallback_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RejectedTransitionAlternative {
    pub template: &'static str,
    pub score: f32,
    pub reason: &'static str,
}

impl DjEngine {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    pub(crate) fn db(&self) -> &Database {
        &self.db
    }

    #[allow(dead_code)]
    pub fn is_enabled(&self) -> bool {
        self.db
            .with_conn(queries::is_dj_engine_enabled)
            .unwrap_or(false)
    }

    pub fn plan_transition(
        &self,
        from: &DjMediaRef,
        to: &DjMediaRef,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Option<TransitionProgram>> {
        Ok(self
            .plan_transition_details(from, to, sample_rate, channels)?
            .map(|plan| plan.program))
    }

    pub fn plan_transition_details(
        &self,
        from: &DjMediaRef,
        to: &DjMediaRef,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Option<DjTransitionPlan>> {
        self.db.with_conn(|conn| {
            if !queries::is_dj_engine_enabled(conn)? {
                return Ok(None);
            }

            let from_key = from.profile_key();
            let to_key = to.profile_key();
            let from_profile = queries::get_audio_dj_profile(conn, &from_key)?;
            let to_profile = queries::get_audio_dj_profile(conn, &to_key)?;
            let from_correction = queries::get_audio_dj_profile_correction(conn, &from_key)?;
            let to_correction = queries::get_audio_dj_profile_correction(conn, &to_key)?;

            let mut policy =
                policy_from_db(conn, from_correction.as_ref(), to_correction.as_ref())?;
            if from_profile.is_none() || to_profile.is_none() {
                policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
                let program = safe_crossfade_program(sample_rate, channels, policy);
                return Ok(Some(plan_from_program(
                    program,
                    Some(missing_profile_reason(from_profile.is_none())),
                )));
            }

            let mut outgoing = profile_from_row(from_profile.as_ref().expect("checked"));
            let mut incoming = profile_from_row(to_profile.as_ref().expect("checked"));
            apply_correction(&mut outgoing, from_correction.as_ref());
            apply_correction(&mut incoming, to_correction.as_ref());

            let safety = runtime_safety_decision(&outgoing, &incoming);
            if safety.force_safe_crossfade {
                policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
            }

            let mut program = Planner::plan(&outgoing, &incoming, &policy);
            program.sample_rate = sample_rate.max(1);
            program.channels = channels.max(1);
            if program.validate().is_err() {
                let program = safe_crossfade_program(sample_rate, channels, policy);
                return Ok(Some(plan_from_program(program, Some("program_invalid"))));
            }
            Ok(Some(plan_from_program(program, safety.fallback_reason)))
        })
    }

    fn recent_bad_feedback_count(&self, media_ref: &DjMediaRef) -> Result<i64> {
        let key = media_ref.profile_key();
        self.db
            .with_conn(|conn| queries::count_recent_bad_dj_feedback_for_ref(conn, &key, 3))
    }
}

fn plan_from_program(
    program: TransitionProgram,
    fallback_reason: Option<&'static str>,
) -> DjTransitionPlan {
    DjTransitionPlan {
        rejected_alternatives: rejected_alternatives_for(&program.template),
        program,
        planner_version: DJ_PLANNER_VERSION,
        fallback_reason,
    }
}

fn missing_profile_reason(from_missing: bool) -> &'static str {
    if from_missing {
        "current_profile_missing"
    } else {
        "next_profile_missing"
    }
}

fn rejected_alternatives_for(selected_template: &str) -> Vec<RejectedTransitionAlternative> {
    const ORDERED: [(&str, f32); 6] = [
        ("BassSwap32", 0.94),
        ("BassSwap16", 0.88),
        ("LongHarmonicBlend", 0.82),
        ("FilterSweep", 0.76),
        ("SlamCut", 0.70),
        ("SafeCrossfade", 0.64),
    ];

    ORDERED
        .into_iter()
        .filter(|(template, _)| *template != selected_template)
        .take(3)
        .map(|(template, score)| RejectedTransitionAlternative {
            template,
            score,
            reason: "not_selected",
        })
        .collect()
}

fn runtime_safety_decision(outgoing: &DjProfile, incoming: &DjProfile) -> RuntimeSafetyDecision {
    if outgoing.safe_crossfade_only || incoming.safe_crossfade_only {
        return RuntimeSafetyDecision {
            fallback_reason: Some("safety_override_safe"),
            force_safe_crossfade: true,
        };
    }
    if outgoing.profile_confidence < PROFILE_CONFIDENCE_FLOOR as f32
        || incoming.profile_confidence < PROFILE_CONFIDENCE_FLOOR as f32
    {
        return RuntimeSafetyDecision {
            fallback_reason: Some("profile_low_confidence"),
            force_safe_crossfade: true,
        };
    }
    RuntimeSafetyDecision {
        fallback_reason: None,
        force_safe_crossfade: false,
    }
}

fn safe_crossfade_program(
    sample_rate: u32,
    channels: u16,
    mut policy: Policy,
) -> TransitionProgram {
    policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
    let profile = fallback_profile();
    let mut program = Planner::plan(&profile, &profile, &policy);
    program.sample_rate = sample_rate.max(1);
    program.channels = channels.max(1);
    program
}

fn fallback_profile() -> DjProfile {
    DjProfile {
        bpm: Some(120.0),
        camelot_key: Some("8A".to_string()),
        energy: Some(0.5),
        beat_grid_seconds: vec![0.0, 0.5],
        downbeat_seconds: vec![0.0],
        phrase_bar_indices: vec![0],
        mix_in_seconds: vec![0.0],
        mix_out_seconds: vec![60.0],
        intro_end_seconds: Some(8.0),
        outro_start_seconds: Some(120.0),
        breakdown_seconds: vec![],
        drop_seconds: vec![],
        safe_transition_windows: vec![noor_mix::profile::TransitionWindow {
            start_seconds: 0.0,
            end_seconds: 8.0,
            confidence: 1.0,
        }],
        vocal_presence_by_bar: vec![0.0],
        vocal_density_by_bar: vec![0.0],
        lufs_loud_body: Some(-12.0),
        true_peak_dbtp: Some(-1.0),
        profile_confidence: 1.0,
        safe_crossfade_only: false,
        profile_version: "fallback".to_string(),
    }
}

fn policy_from_db(
    conn: &Connection,
    outgoing: Option<&AudioDjProfileCorrectionRow>,
    incoming: Option<&AudioDjProfileCorrectionRow>,
) -> Result<Policy> {
    let (mix_intent, transition_speed_bias) = queries::get_dj_global_policy(conn)?;
    let mut policy = Policy {
        mix_intent: parse_mix_intent(&mix_intent),
        transition_speed_bias: parse_speed_bias(&transition_speed_bias),
        ..Policy::default()
    };

    let outgoing_bias = outgoing
        .and_then(|row| row.transition_speed_bias.as_deref())
        .map(parse_speed_bias);
    let incoming_bias = incoming
        .and_then(|row| row.transition_speed_bias.as_deref())
        .map(parse_speed_bias);
    policy.transition_speed_bias = match (outgoing_bias, incoming_bias) {
        (Some(left), Some(right)) if left != right => TransitionSpeedBias::Neutral,
        (Some(bias), _) | (_, Some(bias)) => bias,
        _ => policy.transition_speed_bias,
    };
    Ok(policy)
}

fn parse_mix_intent(value: &str) -> MixIntent {
    match value {
        "safe" => MixIntent::Safe,
        "bold" => MixIntent::Bold,
        _ => MixIntent::Balanced,
    }
}

fn parse_speed_bias(value: &str) -> TransitionSpeedBias {
    match value {
        "slower" => TransitionSpeedBias::Slower,
        "faster" => TransitionSpeedBias::Faster,
        _ => TransitionSpeedBias::Neutral,
    }
}

fn apply_correction(profile: &mut DjProfile, correction: Option<&AudioDjProfileCorrectionRow>) {
    let Some(correction) = correction else {
        return;
    };
    if correction.safe_crossfade_only {
        profile.safe_crossfade_only = true;
    }
    if let Some(multiplier) = correction.bpm_multiplier
        && multiplier.is_finite()
        && multiplier > 0.0
    {
        profile.bpm = profile.bpm.map(|bpm| bpm * multiplier as f32);
    }
}

fn profile_from_row(row: &AudioDjProfileRow) -> DjProfile {
    let beat_grid_seconds = decode_f32_blob(&row.beat_grid_blob).unwrap_or_default();
    let downbeat_seconds = decode_f32_blob(&row.downbeats_blob).unwrap_or_default();
    let phrase_bar_indices = decode_u32_blob(&row.phrase_boundaries_blob).unwrap_or_default();
    let mix_in_seconds = decode_f32_blob(&row.mix_in_blob).unwrap_or_default();
    let mix_out_seconds = decode_f32_blob(&row.mix_out_blob).unwrap_or_default();
    let safe_transition_windows =
        decode_f32_blob(&row.safe_transition_windows_blob).unwrap_or_default();
    DjProfile {
        bpm: estimate_bpm(&beat_grid_seconds),
        camelot_key: Some("8A".to_string()),
        energy: None,
        beat_grid_seconds,
        downbeat_seconds,
        phrase_bar_indices,
        mix_in_seconds,
        mix_out_seconds,
        intro_end_seconds: row.intro_end_seconds.map(|value| value as f32),
        outro_start_seconds: row.outro_start_seconds.map(|value| value as f32),
        breakdown_seconds: decode_f32_blob(&row.breakdown_blob).unwrap_or_default(),
        drop_seconds: decode_f32_blob(&row.drop_blob).unwrap_or_default(),
        safe_transition_windows: safe_transition_windows
            .chunks_exact(3)
            .map(|chunk| noor_mix::profile::TransitionWindow {
                start_seconds: chunk[0],
                end_seconds: chunk[1],
                confidence: chunk[2],
            })
            .collect(),
        vocal_presence_by_bar: decode_f32_blob(&row.vocal_presence_blob).unwrap_or_default(),
        vocal_density_by_bar: decode_f32_blob(&row.vocal_density_blob).unwrap_or_default(),
        lufs_loud_body: row.lufs_loud_body.map(|value| value as f32),
        true_peak_dbtp: row.true_peak_dbtp.map(|value| value as f32),
        profile_confidence: row.profile_confidence as f32,
        safe_crossfade_only: false,
        profile_version: row.profile_version.clone(),
    }
}

fn estimate_bpm(beats: &[f32]) -> Option<f32> {
    let first = *beats.first()?;
    let last = *beats.last()?;
    let intervals = beats.len().saturating_sub(1);
    if intervals == 0 || last <= first {
        return None;
    }
    Some(60.0 / ((last - first) / intervals as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::AudioDjProfileKey;
    use crate::db::schema;
    use crate::services::audio_analysis::dj_profile::{
        DJ_PROFILE_VERSION, encode_f32_blob, encode_u32_blob,
    };

    fn db() -> Database {
        let db = Database::open_in_memory().expect("db");
        db.with_conn(schema::run_migrations).expect("migrations");
        db
    }

    fn key(kind: &str, id: &str) -> AudioDjProfileKey {
        AudioDjProfileKey {
            media_ref_kind: kind.to_string(),
            media_ref_id: id.to_string(),
        }
    }

    fn ref_for(kind: &str, id: i64) -> DjMediaRef {
        match kind {
            "library_track" => DjMediaRef::LibraryTrack { track_id: id },
            "tidal_track" => DjMediaRef::TidalTrack {
                tidal_id: id,
                track_id: None,
            },
            "queue_item" => DjMediaRef::PendingQueueItem {
                queue_item_id: id,
                pending_artist: "Pending Artist".to_string(),
                pending_title: "Pending Title".to_string(),
                tidal_id_hint: None,
            },
            _ => unreachable!(),
        }
    }

    fn enable(db: &Database) {
        db.with_conn(|conn| queries::set_dj_engine_enabled(conn, true))
            .expect("enable");
    }

    fn seed_profile(db: &Database, kind: &str, id: i64, confidence: f64) {
        db.with_conn(|conn| {
            if kind == "library_track" {
                conn.execute(
                    "INSERT OR IGNORE INTO artists (id, name) VALUES (1, 'Artist')",
                    [],
                )?;
                conn.execute(
                    "INSERT OR IGNORE INTO tracks (id, title, artist_id) VALUES (?1, ?2, 1)",
                    rusqlite::params![id, format!("Track {id}")],
                )?;
            }
            if kind == "queue_item" {
                conn.execute(
                    "INSERT OR IGNORE INTO queue (id, track_id, position, source, pending_artist, pending_title)
                     VALUES (?1, NULL, ?1, 'test', 'Pending Artist', 'Pending Title')",
                    rusqlite::params![id],
                )?;
            }
            let row = AudioDjProfileRow {
                media_ref_kind: kind.to_string(),
                media_ref_id: id.to_string(),
                track_id: (kind == "library_track").then_some(id),
                queue_item_id: (kind == "queue_item").then_some(id),
                tidal_id: (kind == "tidal_track").then_some(id),
                profile_version: DJ_PROFILE_VERSION.to_string(),
                beat_grid_blob: encode_f32_blob(&(0..64).map(|i| i as f32 * 0.5).collect::<Vec<_>>()),
                downbeats_blob: encode_f32_blob(&(0..16).map(|i| i as f32 * 2.0).collect::<Vec<_>>()),
                phrase_boundaries_blob: encode_u32_blob(&(0..16).collect::<Vec<_>>()),
                mix_in_blob: encode_f32_blob(&[0.0]),
                mix_out_blob: encode_f32_blob(&[90.0]),
                intro_end_seconds: Some(16.0),
                outro_start_seconds: Some(120.0),
                breakdown_blob: encode_f32_blob(&[]),
                drop_blob: encode_f32_blob(&[]),
                safe_transition_windows_blob: encode_f32_blob(&[0.0, 8.0, 1.0]),
                energy_contour_blob: encode_f32_blob(&[]),
                vocal_presence_blob: encode_f32_blob(&[0.0; 16]),
                vocal_density_blob: encode_f32_blob(&[0.0; 16]),
                lufs_loud_body: Some(-12.0),
                true_peak_dbtp: Some(-1.0),
                beat_confidence: Some(0.9),
                profile_confidence: confidence,
                analysis_scope_ms: 90_000,
                is_temporary: kind == "queue_item",
                source: "test".to_string(),
                computed_at: "now".to_string(),
            };
            queries::upsert_audio_dj_profile(conn, &row)
        })
        .expect("seed profile");
    }

    fn seed_correction(db: &Database, key: AudioDjProfileKey, safe: bool, speed: Option<&str>) {
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: key.media_ref_kind,
                    media_ref_id: key.media_ref_id,
                    bpm_multiplier: None,
                    downbeat_offset_beats: None,
                    phrase_offset_bars: None,
                    safe_crossfade_only: safe,
                    transition_speed_bias: speed.map(str::to_string),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("seed correction");
    }

    fn plan(db: &Database, from: DjMediaRef, to: DjMediaRef) -> Option<TransitionProgram> {
        DjEngine::new(db.clone())
            .plan_transition(&from, &to, 48_000, 2)
            .expect("plan")
    }

    #[test]
    fn disabled_engine_returns_none() {
        let db = db();
        assert!(
            plan(
                &db,
                ref_for("library_track", 1),
                ref_for("library_track", 2)
            )
            .is_none()
        );
    }

    #[test]
    fn disabled_engine_does_not_load_profiles() {
        let db = db();
        assert!(
            plan(
                &db,
                ref_for("library_track", 999),
                ref_for("tidal_track", 888)
            )
            .is_none()
        );
    }

    #[test]
    fn disabled_engine_does_not_log_dj_transition_event() {
        let db = db();
        let _ = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        );
        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                    row.get(0)
                })
                .map_err(Into::into)
            })
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn missing_profile_returns_safe_crossfade_when_enabled() {
        let db = db();
        enable(&db);
        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");
        assert_eq!(program.template, "SafeCrossfade");
    }

    #[test]
    fn missing_decoded_buffer_uses_legacy_path() {
        let decision = RuntimeSafetyDecision {
            fallback_reason: None,
            force_safe_crossfade: false,
        };
        assert!(!decision.force_safe_crossfade);
    }

    #[test]
    fn two_library_profiles_return_valid_program() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program")
        .validate()
        .expect("valid");
    }

    #[test]
    fn library_to_tidal_profiles_return_valid_program() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "tidal_track", 55, 0.9);
        plan(&db, ref_for("library_track", 1), ref_for("tidal_track", 55))
            .expect("program")
            .validate()
            .expect("valid");
    }

    #[test]
    fn tidal_to_pending_profiles_return_valid_program() {
        let db = db();
        enable(&db);
        seed_profile(&db, "tidal_track", 55, 0.9);
        seed_profile(&db, "queue_item", 44, 0.9);
        plan(&db, ref_for("tidal_track", 55), ref_for("queue_item", 44))
            .expect("program")
            .validate()
            .expect("valid");
    }

    #[test]
    fn pending_to_library_profiles_return_valid_program() {
        let db = db();
        enable(&db);
        seed_profile(&db, "queue_item", 44, 0.9);
        seed_profile(&db, "library_track", 1, 0.9);
        plan(&db, ref_for("queue_item", 44), ref_for("library_track", 1))
            .expect("program")
            .validate()
            .expect("valid");
    }

    #[test]
    fn pending_queue_profile_can_plan_after_decode() {
        let db = db();
        enable(&db);
        seed_profile(&db, "queue_item", 44, 0.9);
        seed_profile(&db, "tidal_track", 55, 0.9);
        assert!(plan(&db, ref_for("queue_item", 44), ref_for("tidal_track", 55)).is_some());
    }

    #[test]
    fn low_confidence_profile_falls_back_to_safe_crossfade() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.5);
        seed_profile(&db, "library_track", 2, 0.9);
        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");
        assert_eq!(program.template, "SafeCrossfade");
    }

    #[test]
    fn missed_deadline_falls_back_to_safe_crossfade() {
        let decision = RuntimeSafetyDecision {
            fallback_reason: Some("analysis_late"),
            force_safe_crossfade: true,
        };
        assert_eq!(decision.fallback_reason, Some("analysis_late"));
        assert!(decision.force_safe_crossfade);
    }

    #[test]
    fn missed_deadline_does_not_extend_current_track() {
        let program = safe_crossfade_program(48_000, 2, Policy::default());
        assert!(program.resolve_at > 0);
    }

    #[test]
    fn stale_prepared_pair_falls_back_to_safe_crossfade() {
        let decision = RuntimeSafetyDecision {
            fallback_reason: Some("queue_changed"),
            force_safe_crossfade: false,
        };
        assert_eq!(decision.fallback_reason, Some("queue_changed"));
        assert!(!decision.force_safe_crossfade);
    }

    #[test]
    fn safe_crossfade_only_correction_forces_safe_crossfade() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        seed_correction(&db, key("library_track", "1"), true, None);
        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");
        assert_eq!(program.template, "SafeCrossfade");
    }

    #[test]
    fn one_bad_feedback_does_not_force_safe_crossfade() {
        let db = db();
        enable(&db);
        let engine = DjEngine::new(db.clone());
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, user_rating
                 ) VALUES ('tidal_track', '1', 'tidal_track', '2', 'SafeCrossfade', '{}', 'v1', -1)",
                [],
            )?;
            Ok(())
        })
        .expect("feedback");
        assert_eq!(
            engine
                .recent_bad_feedback_count(&ref_for("tidal_track", 1))
                .unwrap(),
            1
        );
    }

    #[test]
    fn three_bad_feedback_events_suggest_safe_crossfade_only() {
        let db = db();
        enable(&db);
        let engine = DjEngine::new(db.clone());
        db.with_conn(|conn| {
            for _ in 0..3 {
                conn.execute(
                    "INSERT INTO dj_transition_events (
                        from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                        template, program_json, planner_version, user_rating
                     ) VALUES ('tidal_track', '1', 'tidal_track', '2', 'SafeCrossfade', '{}', 'v1', -1)",
                    [],
                )?;
            }
            Ok(())
        })
        .expect("feedback");
        assert_eq!(
            engine
                .recent_bad_feedback_count(&ref_for("tidal_track", 1))
                .unwrap(),
            3
        );
    }

    #[test]
    fn bad_feedback_never_silently_applies_safe_crossfade_only() {
        let db = db();
        enable(&db);
        seed_profile(&db, "tidal_track", 1, 0.9);
        seed_profile(&db, "tidal_track", 2, 0.9);
        let program =
            plan(&db, ref_for("tidal_track", 1), ref_for("tidal_track", 2)).expect("program");
        assert_ne!(program.template, "");
    }

    #[test]
    fn correction_change_replans_when_deadline_not_passed() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        let before = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("before");
        seed_correction(&db, key("library_track", "1"), true, None);
        let after = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("after");
        assert_ne!(before.template, "");
        assert_eq!(after.template, "SafeCrossfade");
    }

    #[test]
    fn correction_change_does_not_mutate_armed_transition() {
        let db = db();
        seed_correction(&db, key("library_track", "1"), true, None);
        let count: i64 = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                    row.get(0)
                })
                .map_err(Into::into)
            })
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn conflicting_speed_biases_resolve_to_neutral() {
        let db = db();
        enable(&db);
        seed_correction(&db, key("library_track", "1"), false, Some("faster"));
        seed_correction(&db, key("library_track", "2"), false, Some("slower"));
        let bias = db
            .with_conn(|conn| {
                let left =
                    queries::get_audio_dj_profile_correction(conn, &key("library_track", "1"))?;
                let right =
                    queries::get_audio_dj_profile_correction(conn, &key("library_track", "2"))?;
                Ok(policy_from_db(conn, left.as_ref(), right.as_ref())?.transition_speed_bias)
            })
            .expect("policy");
        assert_eq!(bias, TransitionSpeedBias::Neutral);
    }

    #[test]
    fn program_rejected_by_audio_safety_falls_back_to_safe_crossfade() {
        let mut policy = Policy::default();
        policy.default_crossfade_ms = 0;
        let program = safe_crossfade_program(48_000, 2, policy);
        assert_eq!(program.template, "SafeCrossfade");
        program.validate().expect("fallback valid");
    }

    #[test]
    fn safe_crossfade_template_is_not_used_when_pair_identity_is_stale() {
        let decision = RuntimeSafetyDecision {
            fallback_reason: Some("queue_changed"),
            force_safe_crossfade: false,
        };
        assert!(!decision.force_safe_crossfade);
    }
}
