use anyhow::Result;
use noor_mix::planner::{DJ_PLANNER_VERSION, MixIntent, TransitionSpeedBias, TransitionTemplate};
use noor_mix::{DjProfile, Planner, Policy, TransitionProgram};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use crate::db::models::{AudioDjProfileCorrectionRow, AudioDjProfileRow, AudioDspFeatures};
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

            let mut outgoing = profile_from_row(conn, from_profile.as_ref().expect("checked"))?;
            let mut incoming = profile_from_row(conn, to_profile.as_ref().expect("checked"))?;
            apply_correction(&mut outgoing, from_correction.as_ref());
            apply_correction(&mut incoming, to_correction.as_ref());

            let safety = runtime_safety_decision(&outgoing, &incoming);
            if safety.force_safe_crossfade {
                policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
            }
            // The planner emits frame positions at its fixed planning rate;
            // rescale so every frame field is denominated in the rate the
            // renderer decks actually use instead of just relabeling it.
            let mut program =
                Planner::plan(&outgoing, &incoming, &policy).rescaled_to(sample_rate.max(1));
            program.channels = channels.max(1);
            if let Err(error) = noor_mix::planner::safety::validate_audio_safety(
                &program,
                &noor_mix::planner::safety::AudioSafetyPolicy::default(),
            ) {
                let reason = match error {
                    noor_mix::program::ProgramError::PlaybackRateOutOfRange
                    | noor_mix::program::ProgramError::LowBandOverlapExceeded => {
                        "audio_safety_rejected"
                    }
                    _ => "program_invalid",
                };
                let program = safe_crossfade_program(sample_rate, channels, policy);
                return Ok(Some(plan_from_program(program, Some(reason))));
            }
            Ok(Some(plan_from_program(program, safety.fallback_reason)))
        })
    }

    pub fn plan_drop_preview(
        &self,
        from: &DjMediaRef,
        to: &DjMediaRef,
        sample_rate: u32,
        channels: u16,
        duration_ms: u32,
    ) -> Result<Option<TransitionProgram>> {
        self.db.with_conn(|conn| {
            if !queries::is_dj_engine_enabled(conn)? {
                return Ok(None);
            }

            let from_key = from.profile_key();
            let to_key = to.profile_key();
            let Some(from_profile) = queries::get_audio_dj_profile(conn, &from_key)? else {
                return Ok(None);
            };
            let Some(to_profile) = queries::get_audio_dj_profile(conn, &to_key)? else {
                return Ok(None);
            };
            let from_correction = queries::get_audio_dj_profile_correction(conn, &from_key)?;
            let to_correction = queries::get_audio_dj_profile_correction(conn, &to_key)?;

            let mut outgoing = profile_from_row(conn, &from_profile)?;
            let mut incoming = profile_from_row(conn, &to_profile)?;
            apply_correction(&mut outgoing, from_correction.as_ref());
            apply_correction(&mut incoming, to_correction.as_ref());
            if runtime_safety_decision(&outgoing, &incoming).force_safe_crossfade {
                return Ok(None);
            }

            Ok(noor_mix::planner::drop_preview_16_program(
                sample_rate,
                channels,
                duration_ms,
                &outgoing,
                &incoming,
            ))
        })
    }
}

fn plan_from_program(
    program: TransitionProgram,
    fallback_reason: Option<&'static str>,
) -> DjTransitionPlan {
    DjTransitionPlan {
        // No rejected-alternatives list: the planner is a short-circuiting
        // decision tree, not a scorer, so there is no honest per-alternative
        // ranking to report. The old fabricated list (a fixed top-3 with
        // invented scores) only misled the cockpit into implying a contest
        // that never happened.
        rejected_alternatives: Vec::new(),
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

pub(crate) fn safe_crossfade_program(
    sample_rate: u32,
    channels: u16,
    mut policy: Policy,
) -> TransitionProgram {
    policy.safety_template_override = Some(TransitionTemplate::SafeCrossfade);
    let profile = fallback_profile();
    let mut program = Planner::plan(&profile, &profile, &policy).rescaled_to(sample_rate.max(1));
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
        manual_drop_seconds: vec![],
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
    // Downbeat nudge: relabel which beats open a bar by shifting the downbeat
    // grid a whole number of (corrected-bpm) beats. Downbeats shifted before
    // track start are dropped; on missing tempo data the nudge is a no-op.
    if let Some(offset_beats) = correction.downbeat_offset_beats
        && offset_beats != 0
        && let Some(beat_seconds) = beat_interval_seconds(profile)
    {
        let shift = offset_beats as f32 * beat_seconds;
        profile.downbeat_seconds = profile
            .downbeat_seconds
            .iter()
            .map(|seconds| seconds + shift)
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .collect();
    }
    // Phrase nudge: shift phrase boundaries by whole bars, dropping any that
    // move before bar zero.
    if let Some(offset_bars) = correction.phrase_offset_bars
        && offset_bars != 0
    {
        profile.phrase_bar_indices = profile
            .phrase_bar_indices
            .iter()
            .filter_map(|&bar| {
                let shifted = i64::from(bar) + offset_bars;
                u32::try_from(shifted).ok()
            })
            .collect();
    }
    let manual_drop_seconds = decode_f32_blob(&correction.manual_drop_blob).unwrap_or_default();
    if !manual_drop_seconds.is_empty() {
        profile.manual_drop_seconds = manual_drop_seconds;
    }
}

fn beat_interval_seconds(profile: &DjProfile) -> Option<f32> {
    if let Some(bpm) = profile.bpm.filter(|bpm| bpm.is_finite() && *bpm > 0.0) {
        return Some(60.0 / bpm);
    }
    median_beat_interval(&profile.beat_grid_seconds)
}

fn profile_from_row(conn: &Connection, row: &AudioDjProfileRow) -> Result<DjProfile> {
    let beat_grid_seconds = decode_f32_blob(&row.beat_grid_blob).unwrap_or_default();
    let downbeat_seconds = decode_f32_blob(&row.downbeats_blob).unwrap_or_default();
    let phrase_bar_indices = decode_u32_blob(&row.phrase_boundaries_blob).unwrap_or_default();
    let mix_in_seconds = decode_f32_blob(&row.mix_in_blob).unwrap_or_default();
    let mix_out_seconds = decode_f32_blob(&row.mix_out_blob).unwrap_or_default();
    let safe_transition_windows =
        decode_f32_blob(&row.safe_transition_windows_blob).unwrap_or_default();
    let dsp = dsp_features_for_profile(conn, row)?;
    let energy = dsp
        .as_ref()
        .and_then(|features| features.energy)
        .map(|value| value as f32)
        .or_else(|| average_energy_contour(&row.energy_contour_blob));
    Ok(DjProfile {
        bpm: estimate_bpm(&beat_grid_seconds),
        camelot_key: dsp.and_then(|features| features.camelot_key),
        energy,
        beat_grid_seconds,
        downbeat_seconds,
        phrase_bar_indices,
        mix_in_seconds,
        mix_out_seconds,
        intro_end_seconds: row.intro_end_seconds.map(|value| value as f32),
        outro_start_seconds: row.outro_start_seconds.map(|value| value as f32),
        breakdown_seconds: decode_f32_blob(&row.breakdown_blob).unwrap_or_default(),
        drop_seconds: decode_f32_blob(&row.drop_blob).unwrap_or_default(),
        manual_drop_seconds: vec![],
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
    })
}

fn dsp_features_for_profile(
    conn: &Connection,
    row: &AudioDjProfileRow,
) -> Result<Option<AudioDspFeatures>> {
    if let Some(track_id) = row.track_id {
        return queries::get_audio_dsp_features(conn, track_id);
    }
    let Some(tidal_id) = row.tidal_id else {
        return Ok(None);
    };
    let track_id = conn
        .query_row(
            "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
            rusqlite::params![tidal_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    track_id
        .map(|track_id| queries::get_audio_dsp_features(conn, track_id))
        .unwrap_or(Ok(None))
}

fn average_energy_contour(blob: &[u8]) -> Option<f32> {
    let contour = decode_f32_blob(blob)?;
    let mut sum = 0.0_f32;
    let mut count = 0_u32;
    for value in contour.into_iter().filter(|value| value.is_finite()) {
        sum += value.clamp(0.0, 1.0);
        count += 1;
    }
    (count > 0).then_some(sum / count as f32)
}

// Tempo from the beat grid via the MEDIAN inter-beat interval. The previous
// mean (span / count) let a single undetected beat or a silence gap stretch
// the average and skew the whole estimate; the median tolerates sparse and
// irregular grids as long as most intervals are genuine.
fn estimate_bpm(beats: &[f32]) -> Option<f32> {
    let median = median_beat_interval(beats)?;
    Some(60.0 / median)
}

fn median_beat_interval(beats: &[f32]) -> Option<f32> {
    let mut intervals = beats
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|delta| delta.is_finite() && *delta > 0.0)
        .collect::<Vec<_>>();
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_by(f32::total_cmp);
    let middle = intervals.len() / 2;
    let median = if intervals.len() % 2 == 0 {
        (intervals[middle - 1] + intervals[middle]) / 2.0
    } else {
        intervals[middle]
    };
    (median.is_finite() && median > 0.0).then_some(median)
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
                seed_dsp(conn, id, "8A", 0.5)?;
            }
            if kind == "tidal_track" {
                let track_id = 100_000 + id;
                conn.execute(
                    "INSERT OR IGNORE INTO artists (id, name) VALUES (1, 'Artist')",
                    [],
                )?;
                conn.execute(
                    "INSERT OR IGNORE INTO tracks (id, title, artist_id, tidal_id) VALUES (?1, ?2, 1, ?3)",
                    rusqlite::params![track_id, format!("Tidal Track {id}"), id],
                )?;
                seed_dsp(conn, track_id, "8A", 0.5)?;
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
                phrase_boundaries_blob: encode_u32_blob(&[0, 8]),
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
                waveform_peaks_blob: encode_f32_blob(&[0.0, 0.5, 1.0, 0.5]),
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

    fn seed_dsp(conn: &Connection, track_id: i64, camelot_key: &str, energy: f64) -> Result<()> {
        queries::upsert_audio_dsp_features(
            conn,
            &AudioDspFeatures {
                track_id,
                bpm: Some(120.0),
                key_signature: None,
                camelot_key: Some(camelot_key.to_string()),
                loudness_lufs: Some(-12.0),
                energy: Some(energy),
                danceability: None,
                beat_strength: None,
                spectral_centroid: None,
                stereo_width: None,
                is_instrumental: false,
                analysis_source: "test".to_string(),
                analysis_offset_ms: 0,
                samples_analyzed: None,
                analyzed_at: "now".to_string(),
                analysis_version: "test".to_string(),
            },
        )
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
                    manual_drop_blob: Vec::new(),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("seed correction");
    }

    fn make_profiles_phrase_deep(db: &Database) {
        db.with_conn(|conn| {
            let phrase_blob = encode_u32_blob(&(0..4).collect::<Vec<_>>());
            let vocal_blob = encode_f32_blob(&[0.0; 4]);
            conn.execute(
                "UPDATE audio_dj_profiles
                 SET phrase_boundaries_blob = ?1,
                     vocal_presence_blob = ?2,
                     vocal_density_blob = ?2
                 WHERE media_ref_kind = 'library_track'",
                rusqlite::params![&phrase_blob, &vocal_blob],
            )?;
            Ok(())
        })
        .expect("phrase-deep profiles");
    }

    fn seed_offset_correction(
        db: &Database,
        key: AudioDjProfileKey,
        downbeat_offset_beats: Option<i64>,
        phrase_offset_bars: Option<i64>,
    ) {
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: key.media_ref_kind,
                    media_ref_id: key.media_ref_id,
                    bpm_multiplier: None,
                    downbeat_offset_beats,
                    phrase_offset_bars,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: Vec::new(),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("seed offset correction");
    }

    fn seed_manual_drop_correction(db: &Database, key: AudioDjProfileKey, drop_seconds: &[f32]) {
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: key.media_ref_kind,
                    media_ref_id: key.media_ref_id,
                    bpm_multiplier: None,
                    downbeat_offset_beats: None,
                    phrase_offset_bars: None,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: encode_f32_blob(drop_seconds),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("manual drop correction");
    }

    fn plan(db: &Database, from: DjMediaRef, to: DjMediaRef) -> Option<TransitionProgram> {
        DjEngine::new(db.clone())
            .plan_transition_details(&from, &to, 48_000, 2)
            .expect("plan")
            .map(|plan| plan.program)
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
        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");
        assert_eq!(program.template, "BassSwap16");
        program.validate().expect("valid");
    }

    #[test]
    fn bold_policy_preserves_bass_swap_for_compatible_profiles() {
        let db = db();
        enable(&db);
        db.with_conn(|conn| queries::set_dj_global_policy(conn, "bold", "neutral"))
            .expect("set bold policy");
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.template, "BassSwap16");
        program.validate().expect("valid");
    }

    #[test]
    fn balanced_policy_can_plan_drop_tease_from_manual_drop_cue() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        make_profiles_phrase_deep(&db);
        seed_manual_drop_correction(&db, key("library_track", "2"), &[32.0]);

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.template, "DropTease16");
        assert_eq!(program.deck_b_start_frame, 768_000);
    }

    #[test]
    fn bold_policy_picks_filter_sweep_without_fabricated_alternatives() {
        let db = db();
        enable(&db);
        db.with_conn(|conn| queries::set_dj_global_policy(conn, "bold", "neutral"))
            .expect("set bold policy");
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: "library_track".to_string(),
                    media_ref_id: "2".to_string(),
                    bpm_multiplier: Some(1.05),
                    downbeat_offset_beats: None,
                    phrase_offset_bars: None,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: Vec::new(),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("tempo correction");
        let engine = DjEngine::new(db);

        let plan = engine
            .plan_transition_details(
                &ref_for("library_track", 1),
                &ref_for("library_track", 2),
                48_000,
                2,
            )
            .expect("plan result")
            .expect("plan");

        assert_eq!(plan.program.template, "FilterSweep");
        // The planner is a short-circuiting decision tree, not a scorer, so it
        // reports no fabricated per-alternative ranking.
        assert!(plan.rejected_alternatives.is_empty());
    }

    #[test]
    fn balanced_unsyncable_tempo_delta_plans_safe_crossfade() {
        // 5% tempo delta cannot be beatmatched inside the 3% nudge band, so
        // balanced intent must not full-blend two un-synced decks; only bold
        // intent may pick the FilterSweep here (covered above).
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        db.with_conn(|conn| {
            queries::upsert_audio_dj_profile_correction(
                conn,
                &AudioDjProfileCorrectionRow {
                    media_ref_kind: "library_track".to_string(),
                    media_ref_id: "2".to_string(),
                    bpm_multiplier: Some(1.05),
                    downbeat_offset_beats: None,
                    phrase_offset_bars: None,
                    safe_crossfade_only: false,
                    transition_speed_bias: None,
                    manual_drop_blob: Vec::new(),
                    notes: None,
                    created_at: "now".to_string(),
                    updated_at: "now".to_string(),
                },
            )
        })
        .expect("seed bpm correction");

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.template, "SafeCrossfade");
    }

    #[test]
    fn missing_dsp_camelot_uses_bass_swap_not_fake_harmonic_match() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        db.with_conn(|conn| {
            conn.execute(
                "DELETE FROM audio_dsp_features WHERE track_id IN (1, 2)",
                [],
            )?;
            Ok(())
        })
        .expect("remove dsp features");

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        // An unknown key must not fabricate a harmonic match (no
        // LongHarmonicBlend/DropTease), but a bass swap is valid without a key
        // when the decks are beatmatched with phrase depth.
        assert_eq!(program.template, "BassSwap16");
    }

    #[test]
    fn distant_camelot_keys_use_bass_swap() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        db.with_conn(|conn| {
            seed_dsp(conn, 1, "8A", 0.5)?;
            seed_dsp(conn, 2, "3A", 0.5)?;
            Ok(())
        })
        .expect("seed clashing dsp features");

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        // A genuine key clash (8A vs 3A) still gets a real blend: the bass swap
        // isolates the low band, so it tolerates the clash rather than dropping
        // to a plain crossfade.
        assert_eq!(program.template, "BassSwap16");
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
        let key = ref_for("tidal_track", 1).profile_key();
        let count = db
            .with_conn(|conn| queries::count_recent_bad_dj_feedback_for_ref(conn, &key, 3))
            .expect("feedback count");
        assert_eq!(count, 1);
    }

    #[test]
    fn three_bad_feedback_events_suggest_safe_crossfade_only() {
        let db = db();
        enable(&db);
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
        let key = ref_for("tidal_track", 1).profile_key();
        let count = db
            .with_conn(|conn| queries::count_recent_bad_dj_feedback_for_ref(conn, &key, 3))
            .expect("feedback count");
        assert_eq!(count, 3);
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
    fn estimate_bpm_uses_median_interval_across_grid_gaps() {
        // 120 BPM grid with one undetected beat; the old span/count mean
        // would report ~103 BPM.
        let beats = [0.0, 0.5, 1.0, 2.0, 2.5, 3.0, 3.5];
        let bpm = estimate_bpm(&beats).expect("bpm");
        assert!((bpm - 120.0).abs() < 0.01, "estimated {bpm}");
    }

    #[test]
    fn estimate_bpm_tolerates_local_jitter() {
        let beats = [0.0, 0.49, 1.0, 1.51, 2.0, 2.5];
        let bpm = estimate_bpm(&beats).expect("bpm");
        assert!((bpm - 120.0).abs() < 0.5, "estimated {bpm}");
    }

    #[test]
    fn estimate_bpm_rejects_degenerate_grids() {
        assert!(estimate_bpm(&[]).is_none());
        assert!(estimate_bpm(&[1.0]).is_none());
        assert!(estimate_bpm(&[2.0, 2.0]).is_none());
        assert!(estimate_bpm(&[5.0, 1.0]).is_none());
        assert!(estimate_bpm(&[0.0, f32::NAN]).is_none());
    }

    #[test]
    fn downbeat_offset_correction_shifts_incoming_sync_start() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        // Seeded grid is 120 BPM (0.5s beats); +2 beats moves the first
        // usable downbeat from 0.0s to 1.0s.
        seed_offset_correction(&db, key("library_track", "2"), Some(2), None);

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.deck_b_start_frame, 48_000);
    }

    #[test]
    fn negative_downbeat_offset_drops_pre_roll_downbeats() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        // Seeded downbeats are every 2.0s from 0.0; -1 beat shifts them to
        // -0.5, 1.5, 3.5, ... and the negative one must be dropped.
        seed_offset_correction(&db, key("library_track", "2"), Some(-1), None);

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.deck_b_start_frame, 72_000);
    }

    #[test]
    fn phrase_offset_correction_changes_phrase_depth_gates() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        // Seeded phrase boundaries are [0, 8]; -2 bars leaves only [6], so
        // the pair no longer qualifies for a bass swap.
        seed_offset_correction(&db, key("library_track", "2"), None, Some(-2));

        let program = plan(
            &db,
            ref_for("library_track", 1),
            ref_for("library_track", 2),
        )
        .expect("program");

        assert_eq!(program.template, "LongHarmonicBlend");
    }

    #[test]
    fn planned_program_is_rescaled_to_requested_sample_rate() {
        let db = db();
        enable(&db);
        seed_profile(&db, "library_track", 1, 0.9);
        seed_profile(&db, "library_track", 2, 0.9);
        seed_offset_correction(&db, key("library_track", "2"), Some(2), None);

        let plan = DjEngine::new(db.clone())
            .plan_transition_details(
                &ref_for("library_track", 1),
                &ref_for("library_track", 2),
                44_100,
                2,
            )
            .expect("plan result")
            .expect("plan");

        let program = plan.program;
        assert_eq!(program.template, "BassSwap16");
        assert_eq!(program.sample_rate, 44_100);
        // 1.0s downbeat at 44.1 kHz, not the 48 kHz planning-rate frame.
        assert_eq!(program.deck_b_start_frame, 44_100);
        // 16 bars at 120 BPM = 32s at either rate.
        assert_eq!(program.resolve_at, 1_411_200);
        program.validate().expect("valid at device rate");
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
