#[cfg(test)]
use crate::db::Database;
use crate::db::audio_settings::AudioQuality;
use crate::db::{
    models::{PlaybackState, QueueItem, Track},
    queries,
};
use crate::playback::automix::{AUTOMIX_MIN_UPCOMING, ensure_automix_queue_depth};
use crate::playback::dj_engine::{DjEngine, DjTransitionPlan};
#[cfg(test)]
use crate::playback::dj_lookahead::load_dj_lookahead_pair;
use crate::playback::dj_lookahead::{DjLookaheadPair, DjMediaRef};
use crate::playback::gapless::{self, GaplessPlan, GaplessSettings};
use crate::playback::queue::{self, ShuffleDebug, ShuffleMode};
use crate::playback::shuffle::generate_shuffle_seed;
use crate::services::audio_analysis::dj_profile::decode_f32_blob;
use crate::services::tidal::stream::{self, StreamInfo, StreamRequest};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
use crate::db::models::AudioDspFeatures;
#[cfg(test)]
use crate::services::audio_analysis::compute_harmonic_multiplier;
#[cfg(test)]
use crate::smart::taste_vector::adapters::from_session_profile;
#[cfg(test)]
use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub queue: Vec<QueueItem>,
}

#[derive(Debug, Clone)]
pub struct RemoveQueueItemOutcome {
    pub snapshot: PlaybackSnapshot,
    pub removed_current: bool,
    pub was_playing: bool,
}

#[derive(Debug, Clone)]
pub struct ShuffleModeUpdate {
    pub snapshot: PlaybackSnapshot,
    pub debug: Option<ShuffleDebug>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackSourceRequest {
    LocalLibrary,
    TidalStream(StreamRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackSourceKind {
    LocalLibrary,
    TidalStream,
}

impl PlaybackSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalLibrary => "local",
            Self::TidalStream => "tidal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedPlaybackJob {
    pub track: Track,
    pub source: PlaybackSourceRequest,
    pub gapless: GaplessPlan,
    pub generation: u64,
    pub output_sample_rate: Option<u32>,
    pub dj_media_ref: Option<DjMediaRef>,
    pub prepared_transition: Option<PreparedTransitionProgram>,
    // Segment-aware seek (option C): for DASH-segmented sources, these tell the
    // decoder to skip ahead by `start_from_segment_index` segments. Position
    // accounting in `PlaybackSharedState` is then absolute-track-samples seeded
    // from `start_from_offset_ms`. A fresh play has both = 0.
    pub start_from_segment_index: usize,
    pub start_from_offset_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedTransitionProgram {
    pub program: noor_mix::TransitionProgram,
    pub transition_event_id: Option<i64>,
    pub fire_ahead_ms: u32,
    pub queue_generation: u64,
    pub current_queue_item_id: Option<i64>,
    pub next_queue_item_id: Option<i64>,
    /// Absolute outgoing-track position (ms) of the grid marker the plan is
    /// beat-aligned to, when the overlap came from downbeat/beat sync. The
    /// runtime fires at this position directly (pos >= anchor) instead of
    /// counting back from the track end, because the track's metadata
    /// duration and its decoded length disagree by up to ~500ms and the
    /// analysis grid lives on the decoded-audio timeline.
    pub anchor_start_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DjLookaheadStart {
    pub current: Option<DjMediaRef>,
    pub next: Option<DjMediaRef>,
    pub current_queue_item_id: Option<i64>,
    pub next_queue_item_id: Option<i64>,
    pub queue_generation: u64,
    pub deadline_samples: u64,
}

impl DjLookaheadStart {
    #[allow(dead_code)]
    pub fn dispatch(
        &self,
        runtime: &crate::playback::runtime::PlaybackRuntimeHandle,
    ) -> Result<()> {
        runtime.start_dj_lookahead(
            self.current.clone(),
            self.next.clone(),
            self.current_queue_item_id,
            self.next_queue_item_id,
            self.queue_generation,
            self.deadline_samples,
        )
    }
}

impl PreparedPlaybackJob {
    #[cfg(test)]
    pub fn test_fixture(track_id: i64, generation: u64) -> Self {
        Self {
            track: Track {
                id: track_id,
                title: format!("test-track-{track_id}"),
                artist_id: 1,
                artist_name: None,
                album_id: None,
                album_title: None,
                disc_number: None,
                track_number: None,
                duration_ms: Some(180_000),
                isrc: None,
                tidal_id: None,
                artist_tidal_id: None,
                album_tidal_id: None,
                ytmusic_id: None,
                soundcloud_id: None,
                best_quality: None,
                best_source: None,
                fidelity_score: 0,
                is_favorite: false,
                play_count: 0,
                last_played_at: None,
                date_added: None,
                source: "local".to_string(),
                artwork_url: None,
            },
            source: PlaybackSourceRequest::LocalLibrary,
            gapless: GaplessPlan::disabled(),
            generation,
            output_sample_rate: None,
            dj_media_ref: None,
            prepared_transition: None,
            start_from_segment_index: 0,
            start_from_offset_ms: 0,
        }
    }
}

pub type PlaybackPreparation = PreparedPlaybackJob;

#[derive(Debug, Clone)]
pub struct ActiveListenSession {
    pub track_id: i64,
    pub started_at: DateTime<Utc>,
    pub accumulated_ms: i64,
    pub resumed_at: Option<DateTime<Utc>>,
    // Multi-track session context, captured at session start so it survives flush.
    pub session_id: String,
    pub source: crate::db::models::ListenSource,
    pub position_in_session: i32,
    pub transition_from_track_id: Option<i64>,
    pub dj_transition_event_id: Option<i64>,
}

// Tracks the rolling state of the user's current listening session across multiple
// tracks. A session continues across tracks if the gap between flush time and the
// next track's start is under SESSION_GAP_THRESHOLD; otherwise a new session_id
// is minted. Lives on AppState; updated in flush_active_listen_session_locked
// after every successful listen_history write.
#[derive(Debug, Clone)]
pub struct LiveListenSession {
    pub session_id: String,
    pub last_track_id: i64,
    pub last_finished_at: DateTime<Utc>,
    pub position: i32,
}

const SESSION_GAP_MINUTES: i64 = 30;

const SESSION_FEEDBACK_LIMIT: i64 = 60;

#[derive(Debug, Default)]
pub(crate) struct SessionTasteProfile {
    pub(crate) positive_artists: HashMap<i64, f64>,
    pub(crate) negative_artists: HashMap<i64, f64>,
    pub(crate) positive_genres: HashMap<String, f64>,
    pub(crate) negative_genres: HashMap<String, f64>,
    pub(crate) recent_track_ids: HashSet<i64>,
    pub(crate) skipped_track_ids: HashSet<i64>,
    pub(crate) current_artist_id: Option<i64>,
    pub(crate) current_album_id: Option<i64>,
    pub(crate) current_source: Option<String>,
    pub(crate) current_genres: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenSessionEndReason {
    Replaced,
    QueueEnded,
    Stopped,
}

impl PlaybackSourceRequest {
    pub fn kind(&self) -> PlaybackSourceKind {
        match self {
            Self::LocalLibrary => PlaybackSourceKind::LocalLibrary,
            Self::TidalStream(_) => PlaybackSourceKind::TidalStream,
        }
    }

    pub fn stream_request(&self) -> Option<&StreamRequest> {
        match self {
            Self::LocalLibrary => None,
            Self::TidalStream(request) => Some(request),
        }
    }
}

impl PreparedPlaybackJob {
    pub fn new(track: Track, source: PlaybackSourceRequest, gapless: GaplessPlan) -> Self {
        Self {
            track,
            source,
            gapless,
            generation: 0,
            output_sample_rate: None,
            dj_media_ref: None,
            prepared_transition: None,
            start_from_segment_index: 0,
            start_from_offset_ms: 0,
        }
    }

    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    pub fn with_dj_media_ref(mut self, media_ref: DjMediaRef) -> Self {
        self.dj_media_ref = Some(media_ref);
        self
    }

    pub fn with_prepared_transition(mut self, transition: PreparedTransitionProgram) -> Self {
        self.prepared_transition = Some(transition);
        self
    }

    pub fn source_kind(&self) -> PlaybackSourceKind {
        self.source.kind()
    }

    pub fn is_local(&self) -> bool {
        matches!(self.source, PlaybackSourceRequest::LocalLibrary)
    }

    pub fn is_tidal(&self) -> bool {
        matches!(self.source, PlaybackSourceRequest::TidalStream(_))
    }

    pub fn stream_request(&self) -> Option<&StreamRequest> {
        self.source.stream_request()
    }

    pub fn track_id(&self) -> i64 {
        self.track.id
    }

    pub fn source_label(&self) -> &'static str {
        self.source_kind().as_str()
    }
}

pub fn dj_lookahead_start_from_pair(
    pair: DjLookaheadPair,
    deadline_samples: u64,
) -> Option<DjLookaheadStart> {
    if pair.current.is_none() && pair.next.is_none() {
        return None;
    }
    Some(DjLookaheadStart {
        current: pair.current,
        next: pair.next,
        current_queue_item_id: pair.current_queue_item_id,
        next_queue_item_id: pair.next_queue_item_id,
        queue_generation: pair.queue_generation,
        deadline_samples,
    })
}

pub fn attach_dj_transition_plan_for_pair(
    engine: &DjEngine,
    job: PlaybackPreparation,
    pair: DjLookaheadPair,
    sample_rate: u32,
    channels: u16,
) -> Result<PlaybackPreparation> {
    attach_dj_transition_plan_for_pair_with_current_duration(
        engine,
        job,
        pair,
        sample_rate,
        channels,
        None,
    )
}

pub fn attach_dj_transition_plan_for_pair_with_current_duration(
    engine: &DjEngine,
    mut job: PlaybackPreparation,
    pair: DjLookaheadPair,
    sample_rate: u32,
    channels: u16,
    current_duration_ms: Option<i64>,
) -> Result<PlaybackPreparation> {
    let (Some(current), Some(next), Some(next_queue_item_id)) = (
        pair.current.as_ref(),
        pair.next.as_ref(),
        pair.next_queue_item_id,
    ) else {
        return Ok(job);
    };
    if next
        .track_id()
        .is_some_and(|next_track_id| next_track_id != job.track.id)
    {
        return Ok(job);
    }
    let existing_armed = engine
        .db()
        .with_conn(|conn| latest_armed_dj_transition_event_for_pair(conn, current, next))?;
    let replace_armed_event_id = if let Some(existing) = existing_armed.as_ref() {
        if engine.db().with_conn(|conn| {
            missing_profile_fallback_resolved(
                conn,
                current,
                next,
                existing.fallback_reason.as_deref(),
            )
        })? {
            Some(existing.id)
        } else {
            let existing_event_id = existing.id;
            let existing_program = existing.program.clone();
            let existing_anchor = existing.anchor_start_ms();
            let fire_ahead_ms = engine.db().with_conn(dj_transition_fire_ahead_ms)?;
            job = job.with_prepared_transition(PreparedTransitionProgram {
                program: existing_program,
                transition_event_id: Some(existing_event_id),
                fire_ahead_ms,
                queue_generation: pair.queue_generation,
                current_queue_item_id: pair.current_queue_item_id,
                next_queue_item_id: Some(next_queue_item_id),
                anchor_start_ms: existing_anchor,
            });
            return Ok(job);
        }
    } else {
        None
    };
    if let Some(plan) =
        engine.plan_transition_details(current, next, sample_rate.max(1), channels.max(1))?
    {
        let planned_template = plan.program.template.clone();
        let render_timing_unstable =
            matches!(planned_template.as_str(), "FilterSweep" | "BassSwap16")
                && engine.db().with_conn(render_timing_unstable)?;
        let (renderer_program, renderer_fallback_reason) = v1_renderable_program(
            &plan.program,
            sample_rate.max(1),
            channels.max(1),
            render_timing_unstable,
        );
        let renderer_plan = DjTransitionPlan {
            program: renderer_program,
            rejected_alternatives: plan.rejected_alternatives,
            planner_version: plan.planner_version,
            fallback_reason: renderer_fallback_reason.or(plan.fallback_reason),
        };
        let timing_plan =
            dj_gapless_plan_for_pair(engine, current, current_duration_ms, &renderer_plan.program);
        job.gapless = timing_plan.gapless;
        let transition_event_id = log_dj_transition_event(
            engine,
            replace_armed_event_id,
            current,
            next,
            &planned_template,
            &renderer_plan,
            &timing_plan,
        )?;
        let fire_ahead_ms = engine.db().with_conn(dj_transition_fire_ahead_ms)?;
        job = job.with_prepared_transition(PreparedTransitionProgram {
            program: renderer_plan.program,
            transition_event_id: Some(transition_event_id),
            fire_ahead_ms,
            queue_generation: pair.queue_generation,
            current_queue_item_id: pair.current_queue_item_id,
            next_queue_item_id: Some(next_queue_item_id),
            anchor_start_ms: timing_plan.anchor_start_ms,
        });
    }
    Ok(job)
}

#[derive(Debug, Clone)]
struct ArmedDjTransitionEvent {
    id: i64,
    program: noor_mix::TransitionProgram,
    fallback_reason: Option<String>,
    planned_start_ms: Option<i64>,
    timing_source: Option<String>,
}

impl ArmedDjTransitionEvent {
    /// The planned start doubles as the decoded-audio-time fire anchor, but
    /// only when it was derived from an analysis grid; a fallback overlap's
    /// planned start is metadata arithmetic and must not be fired against.
    fn anchor_start_ms(&self) -> Option<i64> {
        match self.timing_source.as_deref() {
            Some("downbeat_sync") | Some("beat_sync") => self.planned_start_ms,
            _ => None,
        }
    }
}

fn latest_armed_dj_transition_event_for_pair(
    conn: &Connection,
    current: &DjMediaRef,
    next: &DjMediaRef,
) -> Result<Option<ArmedDjTransitionEvent>> {
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let row = conn
        .query_row(
            "SELECT id, program_json, fallback_reason, planned_start_ms, timing_source
         FROM dj_transition_events
         WHERE from_media_ref_kind = ?1
           AND from_media_ref_id = ?2
           AND to_media_ref_kind = ?3
           AND to_media_ref_id = ?4
           AND timing_status = 'armed'
           AND actual_start_ms IS NULL
           AND outcome IS NULL
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
            params![
                current_key.media_ref_kind,
                current_key.media_ref_id,
                next_key.media_ref_kind,
                next_key.media_ref_id,
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((id, program_json, fallback_reason, planned_start_ms, timing_source)) = row else {
        return Ok(None);
    };
    let program = serde_json::from_str(&program_json)?;
    Ok(Some(ArmedDjTransitionEvent {
        id,
        program,
        fallback_reason,
        planned_start_ms,
        timing_source,
    }))
}

fn missing_profile_fallback_resolved(
    conn: &Connection,
    current: &DjMediaRef,
    next: &DjMediaRef,
    fallback_reason: Option<&str>,
) -> Result<bool> {
    let Some(media_ref) = (match fallback_reason {
        Some("current_profile_missing") => Some(current),
        Some("next_profile_missing") => Some(next),
        _ => None,
    }) else {
        return Ok(false);
    };
    Ok(queries::get_audio_dj_profile(conn, &media_ref.profile_key())?.is_some())
}

const DJ_FIRE_AHEAD_WINDOW: i64 = 20;
const DJ_FIRE_AHEAD_POSITIVE_PERCENT: usize = 70;
const DJ_FIRE_AHEAD_MEDIAN_FLOOR_MS: i64 = 150;
const DJ_FIRE_AHEAD_MAX_MS: i64 = 150;
const DJ_FILTER_SWEEP_TIMING_WINDOW: i64 = 20;
const DJ_FILTER_SWEEP_TIMING_MIN_ROWS: usize = 4;
const DJ_FILTER_SWEEP_MEDIAN_ABS_MAX_MS: i64 = 300;
const DJ_FILTER_SWEEP_WORST_ABS_MAX_MS: i64 = 750;
const DJ_FILTER_SWEEP_RENDER_MS: u32 = 18_000;
const DJ_BASS_SWAP_16_RENDER_MS: u32 = 24_000;
const DJ_BASS_SWAP_32_RENDER_MS: u32 = 28_000;
const DJ_SLAM_CUT_RENDER_MS: u32 = 200;
const DJ_LONG_HARMONIC_BLEND_RENDER_MS: u32 = 24_000;

fn dj_transition_fire_ahead_ms(conn: &Connection) -> Result<u32> {
    let deltas = dj_timing_calibration_deltas(conn, DJ_FIRE_AHEAD_WINDOW)?;
    Ok(fire_ahead_ms_from_deltas(&deltas))
}

fn render_timing_unstable(conn: &Connection) -> Result<bool> {
    let deltas = dj_timing_calibration_deltas(conn, DJ_FILTER_SWEEP_TIMING_WINDOW)?;
    Ok(render_timing_unstable_from_deltas(&deltas))
}

fn dj_timing_calibration_deltas(conn: &Connection, limit: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT timing_delta_ms
         FROM dj_transition_events
         WHERE timing_status = 'fired'
           AND timing_delta_ms IS NOT NULL
           AND runtime_rendered_dj_mixer = 1
           AND runtime_renderer_status IN ('rendered_handoff', 'rendered_overlay')
           AND COALESCE(runtime_renderer_reason, 'none') = 'none'
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], |row| row.get::<_, i64>(0))?;
    let mut deltas = Vec::new();
    for row in rows {
        deltas.push(row?);
    }
    Ok(deltas)
}

fn render_timing_unstable_from_deltas(deltas: &[i64]) -> bool {
    if deltas.len() < DJ_FILTER_SWEEP_TIMING_MIN_ROWS {
        return false;
    }
    let Some(median_abs) = median_abs_delta(deltas) else {
        return false;
    };
    let worst_abs = deltas.iter().map(|delta| delta.abs()).max().unwrap_or(0);
    median_abs > DJ_FILTER_SWEEP_MEDIAN_ABS_MAX_MS || worst_abs > DJ_FILTER_SWEEP_WORST_ABS_MAX_MS
}

fn median_abs_delta(deltas: &[i64]) -> Option<i64> {
    if deltas.is_empty() {
        return None;
    }
    let mut values = deltas.iter().map(|delta| delta.abs()).collect::<Vec<_>>();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2)
    } else {
        Some(values[middle])
    }
}

fn fire_ahead_ms_from_deltas(deltas: &[i64]) -> u32 {
    if deltas.len() < DJ_FIRE_AHEAD_WINDOW as usize {
        return 0;
    }
    let positive_count = deltas.iter().filter(|delta| **delta > 0).count();
    if positive_count * 100 < deltas.len() * DJ_FIRE_AHEAD_POSITIVE_PERCENT {
        return 0;
    }
    median_delta(deltas)
        .filter(|delta| *delta > DJ_FIRE_AHEAD_MEDIAN_FLOOR_MS)
        .map(|delta| (delta / 2).clamp(0, DJ_FIRE_AHEAD_MAX_MS) as u32)
        .unwrap_or(0)
}

fn median_delta(deltas: &[i64]) -> Option<i64> {
    if deltas.is_empty() {
        return None;
    }
    let mut values = deltas.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        Some((values[middle - 1] + values[middle]) / 2)
    } else {
        Some(values[middle])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DjTransitionTimingPlan {
    gapless: GaplessPlan,
    planned_start_ms: Option<i64>,
    anchor_start_ms: Option<i64>,
    timing_source: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncedDjOverlap {
    overlap_ms: i32,
    timing_source: &'static str,
}

fn dj_gapless_plan_from_program(program: &noor_mix::TransitionProgram) -> GaplessPlan {
    let sample_rate = program.sample_rate.max(1);
    let overlap_ms = ((program.resolve_at.saturating_mul(1000)) / u64::from(sample_rate))
        .clamp(250, i32::MAX as u64) as i32;
    GaplessPlan {
        enabled: true,
        overlap_ms,
        prebuffer_ms: 500,
        requires_stream_metadata: false,
    }
}

fn dj_gapless_plan_for_pair(
    engine: &DjEngine,
    current: &DjMediaRef,
    current_duration_ms: Option<i64>,
    program: &noor_mix::TransitionProgram,
) -> DjTransitionTimingPlan {
    let mut plan = dj_gapless_plan_from_program(program);
    let mut timing_source = "fallback_overlap";
    let mut grid_synced = false;
    let mut duration_ms = current_duration_ms;
    if let Ok(Some(synced)) = engine.db().with_conn(|conn| {
        synced_dj_overlap_ms(conn, current, current_duration_ms, program, plan.overlap_ms)
    }) {
        plan.overlap_ms = synced.overlap_ms;
        timing_source = synced.timing_source;
        grid_synced = true;
    }
    if duration_ms.is_none() {
        duration_ms = engine
            .db()
            .with_conn(|conn| current_track_duration_ms(conn, current))
            .ok()
            .flatten();
    }
    let planned_start_ms =
        duration_ms.map(|duration| duration.saturating_sub(i64::from(plan.overlap_ms)).max(0));
    DjTransitionTimingPlan {
        gapless: plan,
        planned_start_ms,
        // duration - overlap cancels back to the grid marker the sync pass
        // picked, so this is an exact decoded-audio-time anchor. A plain
        // fallback overlap has no grid behind it and must keep firing from
        // the track end.
        anchor_start_ms: if grid_synced { planned_start_ms } else { None },
        timing_source,
    }
}

fn synced_dj_overlap_ms(
    conn: &Connection,
    current: &DjMediaRef,
    current_duration_ms: Option<i64>,
    program: &noor_mix::TransitionProgram,
    preferred_overlap_ms: i32,
) -> Result<Option<SyncedDjOverlap>> {
    let Some(duration_ms) = current_duration_ms.or(current_track_duration_ms(conn, current)?)
    else {
        return Ok(None);
    };
    let key = current.profile_key();
    let Some(profile) = queries::get_audio_dj_profile(conn, &key)? else {
        return Ok(None);
    };
    if profile.profile_confidence < 0.65 {
        return Ok(None);
    }

    let downbeats = decode_f32_blob(&profile.downbeats_blob).unwrap_or_default();
    if let Some(overlap_ms) = synced_overlap_from_grid_ms(
        duration_ms,
        &downbeats,
        preferred_overlap_ms,
        Some(program.resolve_at),
        program.sample_rate,
    ) {
        return Ok(Some(SyncedDjOverlap {
            overlap_ms,
            timing_source: "downbeat_sync",
        }));
    }

    let beats = decode_f32_blob(&profile.beat_grid_blob).unwrap_or_default();
    Ok(synced_overlap_from_grid_ms(
        duration_ms,
        &beats,
        preferred_overlap_ms,
        Some(program.resolve_at),
        program.sample_rate,
    )
    .map(|overlap_ms| SyncedDjOverlap {
        overlap_ms,
        timing_source: "beat_sync",
    }))
}

fn current_track_duration_ms(conn: &Connection, current: &DjMediaRef) -> Result<Option<i64>> {
    match current {
        DjMediaRef::LibraryTrack { track_id }
        | DjMediaRef::TidalTrack {
            track_id: Some(track_id),
            ..
        } => conn
            .query_row(
                "SELECT duration_ms FROM tracks WHERE id = ?1",
                params![track_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(Into::into),
        DjMediaRef::TidalTrack { tidal_id, .. } => conn
            .query_row(
                "SELECT duration_ms FROM tracks WHERE tidal_id = ?1 LIMIT 1",
                params![tidal_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(Into::into),
        DjMediaRef::PendingQueueItem { .. } => Ok(None),
    }
}

fn synced_overlap_from_grid_ms(
    duration_ms: i64,
    grid_seconds: &[f32],
    preferred_overlap_ms: i32,
    program_samples: Option<u64>,
    sample_rate: u32,
) -> Option<i32> {
    const MIN_SYNC_OVERLAP_MS: i64 = 8_000;
    const MAX_SYNC_OVERLAP_MS: i64 = 28_000;
    let preferred_ms = preferred_overlap_ms.max(250) as i64;
    let program_ms = program_samples
        .map(|samples| {
            ((samples.saturating_mul(1000)) / u64::from(sample_rate.max(1)))
                .clamp(250, i64::MAX as u64) as i64
        })
        .unwrap_or(preferred_ms);
    let min_overlap_ms = preferred_ms
        .max(program_ms)
        .max(MIN_SYNC_OVERLAP_MS)
        .min(MAX_SYNC_OVERLAP_MS);
    let grid = extrapolated_grid_ms(grid_seconds, duration_ms)?;

    grid.into_iter()
        .filter_map(|start_ms| {
            let overlap_ms = duration_ms.saturating_sub(start_ms);
            (overlap_ms >= min_overlap_ms && overlap_ms <= MAX_SYNC_OVERLAP_MS)
                .then_some(overlap_ms as i32)
        })
        .min()
}

fn extrapolated_grid_ms(grid_seconds: &[f32], duration_ms: i64) -> Option<Vec<i64>> {
    let mut grid = grid_seconds
        .iter()
        .filter_map(|seconds| {
            seconds
                .is_finite()
                .then_some((*seconds * 1000.0).round() as i64)
        })
        .filter(|ms| *ms >= 0 && *ms < duration_ms)
        .collect::<Vec<_>>();
    grid.sort_unstable();
    grid.dedup();
    if grid.len() < 2 {
        return (!grid.is_empty()).then_some(grid);
    }

    // Extrapolate past the last detected marker using the MEDIAN spacing; the
    // previous minimum let one noisy near-duplicate pair flood the tail with
    // arbitrarily dense fake markers, so the "synced" overlap could start
    // anywhere instead of on a real beat.
    let deltas = grid
        .windows(2)
        .filter_map(|pair| {
            let delta = pair[1].saturating_sub(pair[0]);
            (delta > 0).then_some(delta)
        })
        .collect::<Vec<_>>();
    let interval_ms = median_delta(&deltas).filter(|delta| *delta > 0)?;
    let mut next = grid.last().copied()?.saturating_add(interval_ms);
    while next < duration_ms {
        grid.push(next);
        next = next.saturating_add(interval_ms);
    }
    Some(grid)
}

fn v1_renderable_program(
    program: &noor_mix::TransitionProgram,
    sample_rate: u32,
    channels: u16,
    render_timing_unstable: bool,
) -> (noor_mix::TransitionProgram, Option<&'static str>) {
    if program.template == "SafeCrossfade" {
        return (program.clone(), None);
    }
    if program.template == "BassSwap16" {
        if render_timing_unstable {
            let mut renderer_program = crate::playback::dj_engine::safe_crossfade_program(
                sample_rate,
                channels,
                noor_mix::Policy::default(),
            );
            preserve_planner_sync_fields(&mut renderer_program, program);
            return (renderer_program, Some("timing_unstable"));
        }
        let mut renderer_program = noor_mix::planner::bass_swap_16_program(
            sample_rate,
            channels,
            DJ_BASS_SWAP_16_RENDER_MS,
        );
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    if program.template == "BassSwap32" {
        if render_timing_unstable {
            let mut renderer_program = crate::playback::dj_engine::safe_crossfade_program(
                sample_rate,
                channels,
                noor_mix::Policy::default(),
            );
            preserve_planner_sync_fields(&mut renderer_program, program);
            return (renderer_program, Some("timing_unstable"));
        }
        let mut renderer_program = noor_mix::planner::bass_swap_32_program(
            sample_rate,
            channels,
            DJ_BASS_SWAP_32_RENDER_MS,
        );
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    if program.template == "SlamCut" {
        let mut renderer_program =
            noor_mix::planner::slam_cut_program(sample_rate, channels, DJ_SLAM_CUT_RENDER_MS);
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    if program.template == "LongHarmonicBlend" {
        let rate = program
            .automation
            .iter()
            .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
            .map(|event| event.to)
            .unwrap_or(1.0);
        let mut renderer_program = noor_mix::planner::long_harmonic_blend_program(
            sample_rate,
            channels,
            DJ_LONG_HARMONIC_BLEND_RENDER_MS,
            rate,
        );
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    if program.template == "DropTease16" {
        let mut renderer_program =
            noor_mix::planner::drop_tease_16_program(sample_rate, channels, 16_000);
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    if program.template == "FilterSweep" {
        if render_timing_unstable {
            let mut renderer_program = crate::playback::dj_engine::safe_crossfade_program(
                sample_rate,
                channels,
                noor_mix::Policy::default(),
            );
            preserve_planner_sync_fields(&mut renderer_program, program);
            return (renderer_program, Some("timing_unstable"));
        }
        let mut renderer_program = noor_mix::planner::filter_sweep_eq_wash_program(
            sample_rate,
            channels,
            DJ_FILTER_SWEEP_RENDER_MS,
        );
        preserve_planner_sync_fields(&mut renderer_program, program);
        return (renderer_program, None);
    }
    (
        crate::playback::dj_engine::safe_crossfade_program(
            sample_rate,
            channels,
            noor_mix::Policy::default(),
        ),
        Some("template_not_renderable"),
    )
}

fn preserve_planner_sync_fields(
    renderer_program: &mut noor_mix::TransitionProgram,
    planner_program: &noor_mix::TransitionProgram,
) {
    renderer_program.deck_a_start_frame = planner_program.deck_a_start_frame;
    renderer_program.deck_b_start_frame = planner_program.deck_b_start_frame;
    if let Some(rate) = planner_program
        .automation
        .iter()
        .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
        .map(|event| event.to)
    {
        renderer_program
            .automation
            .retain(|event| event.param != noor_mix::Param::PlaybackRate(noor_mix::DeckId::B));
        renderer_program.automation.push(noor_mix::AutomationEvent {
            param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
            start_sample: 0,
            end_sample: renderer_program.resolve_at,
            from: rate,
            to: rate,
            curve: noor_mix::Curve::Linear,
        });
    }
}

fn log_dj_transition_event(
    engine: &DjEngine,
    replace_armed_event_id: Option<i64>,
    current: &DjMediaRef,
    next: &DjMediaRef,
    planned_template: &str,
    plan: &DjTransitionPlan,
    timing_plan: &DjTransitionTimingPlan,
) -> Result<i64> {
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    let program_json = serde_json::to_string(&plan.program)?;
    let rejected_json = serde_json::to_string(&plan.rejected_alternatives)?;
    engine.db().with_conn(|conn| {
        if let Some(id) = replace_armed_event_id {
            queries::replace_armed_dj_transition_event(
                conn,
                id,
                planned_template,
                program_json.as_str(),
                Some(rejected_json.as_str()),
                plan.planner_version,
                plan.fallback_reason,
                timing_plan.planned_start_ms,
                Some(timing_plan.timing_source),
            )?;
            Ok(id)
        } else {
            queries::insert_dj_transition_event(
                conn,
                current.track_id(),
                next.track_id(),
                Some(current_key.media_ref_kind.as_str()),
                Some(current_key.media_ref_id.as_str()),
                Some(next_key.media_ref_kind.as_str()),
                Some(next_key.media_ref_id.as_str()),
                planned_template,
                program_json.as_str(),
                Some(rejected_json.as_str()),
                plan.planner_version,
                plan.fallback_reason,
                timing_plan.planned_start_ms,
                Some(timing_plan.timing_source),
                Some("armed"),
            )
        }
    })
}

impl ActiveListenSession {
    pub fn start(
        track_id: i64,
        now: DateTime<Utc>,
        source: crate::db::models::ListenSource,
        prior: Option<&LiveListenSession>,
    ) -> Self {
        let (session_id, position, transition_from) = match prior {
            Some(ls) if (now - ls.last_finished_at).num_minutes() < SESSION_GAP_MINUTES => (
                ls.session_id.clone(),
                ls.position + 1,
                Some(ls.last_track_id),
            ),
            _ => (uuid::Uuid::new_v4().to_string(), 0, None),
        };
        Self {
            track_id,
            started_at: now,
            accumulated_ms: 0,
            resumed_at: Some(now),
            session_id,
            source,
            position_in_session: position,
            transition_from_track_id: transition_from,
            dj_transition_event_id: None,
        }
    }

    pub fn with_dj_transition_event_id(mut self, event_id: Option<i64>) -> Self {
        self.dj_transition_event_id = event_id;
        self
    }

    pub fn to_live_session(&self, finished_at: DateTime<Utc>) -> LiveListenSession {
        LiveListenSession {
            session_id: self.session_id.clone(),
            last_track_id: self.track_id,
            last_finished_at: finished_at,
            position: self.position_in_session,
        }
    }
}

pub fn latest_open_dj_transition_event_for_pair(
    conn: &Connection,
    from_track_id: Option<i64>,
    to_track_id: i64,
) -> Result<Option<i64>> {
    let Some(from_track_id) = from_track_id else {
        return Ok(None);
    };
    conn.query_row(
        "SELECT id
         FROM dj_transition_events
         WHERE from_track_id = ?1
           AND to_track_id = ?2
           AND outcome IS NULL
         ORDER BY CASE timing_status
             WHEN 'fired' THEN 0
             WHEN 'late' THEN 1
             WHEN 'armed' THEN 2
             ELSE 3
         END,
         started_at DESC,
         id DESC
         LIMIT 1",
        params![from_track_id, to_track_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn record_dj_transition_listen_outcome(
    conn: &Connection,
    transition_event_id: Option<i64>,
    listened_ms: i64,
    completed: bool,
) -> Result<()> {
    let Some(id) = transition_event_id else {
        return Ok(());
    };
    if completed {
        queries::update_dj_transition_outcome(conn, id, "finished", false)?;
    } else if listened_ms < 30_000 {
        queries::update_dj_transition_outcome(conn, id, "skip_within_30s", true)?;
    }
    Ok(())
}

// Reads the queue.source string of the currently-playing queue item and maps it
// to a ListenSource. Returns Unknown if no current queue item or the source
// label isn't one we recognize - those rows still get written, they just count
// at half confidence in the trainer.
pub fn lookup_current_listen_source(conn: &Connection) -> crate::db::models::ListenSource {
    use crate::db::models::ListenSource;
    let raw: Option<String> = conn
        .query_row(
            "SELECT q.source FROM playback_state ps
             JOIN queue q ON q.id = ps.current_queue_item_id
             WHERE ps.id = 1",
            [],
            |row| row.get(0),
        )
        .ok();
    match raw.as_deref() {
        Some("user") | Some("library") | Some("playback") => ListenSource::Manual,
        Some("radio") | Some("radio_pending") => ListenSource::Radio,
        Some("playlist") => ListenSource::Playlist,
        Some("album") => ListenSource::Album,
        Some("artist") => ListenSource::Artist,
        Some("search") => ListenSource::Search,
        Some("automix") | Some("automix-new") => ListenSource::Automix,
        _ => ListenSource::Unknown,
    }
}

impl ActiveListenSession {
    pub fn pause(&mut self, now: DateTime<Utc>) {
        if let Some(resumed_at) = self.resumed_at.take() {
            self.accumulated_ms += (now - resumed_at).num_milliseconds().max(0);
        }
    }

    pub fn resume(&mut self, now: DateTime<Utc>) {
        if self.resumed_at.is_none() {
            self.resumed_at = Some(now);
        }
    }

    pub fn listened_ms_at(&self, now: DateTime<Utc>) -> i64 {
        let live_ms = self
            .resumed_at
            .map(|resumed_at| (now - resumed_at).num_milliseconds().max(0))
            .unwrap_or(0);
        self.accumulated_ms + live_ms
    }
}

/// Canonical "give me everything the UI needs to render the player" loader.
/// Endpoints that mutate queue or playback_state should return this snapshot
/// (or call back into it via `get_playback_state`) so the UI never has to
/// stitch together partial responses. Returns `{state, queue}` together.
pub fn load_snapshot(conn: &Connection) -> Result<PlaybackSnapshot> {
    let state = load_state(conn)?;
    let queue = queue::load_queue(conn)?;
    Ok(PlaybackSnapshot { state, queue })
}

pub fn load_state(conn: &Connection) -> Result<PlaybackState> {
    let row = conn
        .query_row(
            "SELECT current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms, automix_discover_new, automix_use_learning, automix_allow_external, current_queue_item_id
             FROM playback_state
             WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, f64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, i32>(7)?,
                    row.get::<_, bool>(8)?,
                    row.get::<_, bool>(9)?,
                    row.get::<_, bool>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("playback_state row missing"))?;

    let current_track = match row.0 {
        Some(track_id) => queue::get_track_by_id(conn, track_id)?,
        None => None,
    };

    Ok(PlaybackState {
        current_track,
        current_queue_item_id: row.11,
        position_ms: row.1,
        is_playing: row.2,
        volume: row.3,
        shuffle_mode: row.4,
        repeat_mode: row.5,
        automix_enabled: row.6,
        crossfade_ms: row.7,
        automix_discover_new: row.8,
        automix_use_learning: row.9,
        automix_allow_external: row.10,
        // buffered_ms and buffered_start_ms are runtime-only fields overlaid
        // by the live snapshot helper in routes.rs; the DB has no column for
        // either of them.
        buffered_ms: 0,
        buffered_start_ms: 0,
    })
}

pub fn enqueue_track(conn: &Connection, track_id: i64, source: &str) -> Result<Vec<QueueItem>> {
    let track = queue::get_track_by_id(conn, track_id)?
        .ok_or_else(|| anyhow!("track {track_id} not found"))?;
    queue::append_tracks(conn, &[track], source)
}

pub fn replace_queue_with_tracks(
    conn: &Connection,
    track_ids: &[i64],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let mut tracks = Vec::new();
    for track_id in track_ids {
        if let Some(track) = queue::get_track_by_id(conn, *track_id)? {
            tracks.push(track);
        }
    }
    let queue = queue::replace_queue(conn, &tracks, source)?;
    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
        [],
    )?;
    Ok(queue)
}

/// Replace the queue with tracks plus optional per-row reasons.
///
/// `reasons` is index-aligned with `track_ids`. Missing or shorter
/// reasons lists default to `None` for the absent indices. Tracks that
/// fail to load are skipped (consistent with `replace_queue_with_tracks`).
pub fn replace_queue_with_reasons(
    conn: &Connection,
    track_ids: &[i64],
    reasons: &[Option<String>],
    source: &str,
) -> Result<Vec<QueueItem>> {
    let mut paired: Vec<(Track, Option<String>)> = Vec::with_capacity(track_ids.len());
    for (idx, track_id) in track_ids.iter().enumerate() {
        if let Some(track) = queue::get_track_by_id(conn, *track_id)? {
            let reason = reasons.get(idx).cloned().unwrap_or(None);
            paired.push((track, reason));
        }
    }
    let queue = queue::replace_queue_with_reasons(conn, &paired, source)?;
    conn.execute(
        "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
        [],
    )?;
    Ok(queue)
}

pub fn play_track_now(conn: &Connection, track_id: i64) -> Result<PlaybackSnapshot> {
    let track = queue::get_track_by_id(conn, track_id)?
        .ok_or_else(|| anyhow!("track {track_id} not found"))?;

    let current_ids = queue::queue_track_ids(conn)?;
    if !current_ids.contains(&track_id) {
        queue::append_tracks(conn, std::slice::from_ref(&track), "playback")?;
    }

    // Resolve to the actual queue row so the UI's "now playing" highlight
    // points at the right row (not just any row sharing the same track_id).
    let queue_item_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM queue WHERE track_id = ?1 ORDER BY position ASC, id ASC LIMIT 1",
            params![track_id],
            |row| row.get(0),
        )
        .optional()?;

    conn.execute(
        "UPDATE playback_state
         SET current_track_id = ?1,
             current_queue_item_id = ?2,
             position_ms = 0,
             is_playing = 1
         WHERE id = 1",
        params![track_id, queue_item_id],
    )?;

    load_snapshot(conn)
}

/// What changed during reconciliation. Callers use this to decide which
/// `AppEvent`s to broadcast and whether to stop the audio runtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileOutcome {
    /// At least one queue row was deleted.
    pub queue_changed: bool,
    /// `playback_state.current_track_id` was updated (advanced or cleared).
    pub current_changed: bool,
    /// `is_playing` was set to 0 because no surviving track exists. Caller
    /// should also stop the audio runtime to release the output device.
    pub stopped_playback: bool,
    /// The new current track ID. `None` means playback was cleared.
    pub new_current_track_id: Option<i64>,
}

/// Reconcile the queue and playback state with a set of just-deleted tracks.
///
/// Run inside a single transaction so the queue and playback_state never
/// drift. Behavior:
/// 1. Delete queue rows pointing at any of `deleted_track_ids`. Pending rows
///    (track_id IS NULL) are unaffected - Last.fm radio neighbors don't
///    reference local track IDs.
/// 2. If the current track is in the deleted set, advance `current_track_id`
///    and `current_queue_item_id` to the next surviving queue row. The next
///    survivor is preferred from "after the current position"; if none
///    exists there, fall back to the first surviving row globally.
/// 3. If no survivor exists, clear current_*, set is_playing = 0, and signal
///    `stopped_playback` so the caller can also stop the audio runtime.
/// 4. Renormalise positions to be contiguous starting at 0.
pub fn reconcile_after_track_delete(
    conn: &Connection,
    deleted_track_ids: &[i64],
) -> Result<ReconcileOutcome> {
    if deleted_track_ids.is_empty() {
        return Ok(ReconcileOutcome::default());
    }

    let deleted_set: HashSet<i64> = deleted_track_ids.iter().copied().collect();
    let tx = conn.unchecked_transaction()?;

    // Snapshot the queue before deletion so we can pick the next survivor by
    // position - `current_queue_item_id` would be invalid after deletion.
    let rows: Vec<(i64, Option<i64>, i32)> = {
        let mut stmt =
            tx.prepare("SELECT id, track_id, position FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };

    let (current_track_id, current_qid): (Option<i64>, Option<i64>) = tx.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let current_pos = current_qid.and_then(|cqid| {
        rows.iter()
            .find(|(id, _, _)| *id == cqid)
            .map(|(_, _, p)| *p)
    });

    let is_survivor = |tid: &Option<i64>| -> bool {
        match tid {
            None => true,
            Some(t) => !deleted_set.contains(t),
        }
    };

    let new_current_row: Option<&(i64, Option<i64>, i32)> = if let Some(cp) = current_pos {
        rows.iter()
            .find(|(id, tid, p)| *p > cp && is_survivor(tid) && Some(*id) != current_qid)
            .or_else(|| {
                rows.iter()
                    .find(|(id, tid, _)| is_survivor(tid) && Some(*id) != current_qid)
            })
    } else {
        None
    };

    // Apply deletion now that survivor selection is decided.
    let placeholders = (1..=deleted_track_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let delete_sql = format!("DELETE FROM queue WHERE track_id IN ({})", placeholders);
    let deleted = tx.execute(
        &delete_sql,
        rusqlite::params_from_iter(deleted_track_ids.iter()),
    )?;
    let queue_changed = deleted > 0;

    let mut current_changed = false;
    let mut stopped_playback = false;
    let mut new_current_track_id = current_track_id;

    if let Some(ctid) = current_track_id
        && deleted_set.contains(&ctid)
    {
        current_changed = true;
        if let Some((qid, tid, _)) = new_current_row.copied() {
            tx.execute(
                "UPDATE playback_state
                     SET current_track_id = ?1, current_queue_item_id = ?2, position_ms = 0
                     WHERE id = 1",
                params![tid, qid],
            )?;
            new_current_track_id = tid;
        } else {
            tx.execute(
                "UPDATE playback_state
                     SET current_track_id = NULL,
                         current_queue_item_id = NULL,
                         position_ms = 0,
                         is_playing = 0
                     WHERE id = 1",
                [],
            )?;
            stopped_playback = true;
            new_current_track_id = None;
        }
    }

    if queue_changed {
        let surviving_ids: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT id FROM queue ORDER BY position ASC, id ASC")?;
            stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (idx, qid) in surviving_ids.iter().enumerate() {
            tx.execute(
                "UPDATE queue SET position = ?1 WHERE id = ?2",
                params![idx as i32, qid],
            )?;
        }
    }

    tx.commit()?;
    Ok(ReconcileOutcome {
        queue_changed,
        current_changed,
        stopped_playback,
        new_current_track_id,
    })
}

pub fn pause(conn: &Connection) -> Result<PlaybackSnapshot> {
    conn.execute("UPDATE playback_state SET is_playing = 0 WHERE id = 1", [])?;
    load_snapshot(conn)
}

pub fn resume(conn: &Connection) -> Result<PlaybackSnapshot> {
    conn.execute("UPDATE playback_state SET is_playing = 1 WHERE id = 1", [])?;
    load_snapshot(conn)
}

pub fn set_volume(conn: &Connection, volume: f64) -> Result<PlaybackSnapshot> {
    let clamped = volume.clamp(0.0, 1.0);
    conn.execute(
        "UPDATE playback_state SET volume = ?1 WHERE id = 1",
        params![clamped],
    )?;
    load_snapshot(conn)
}

fn current_shuffle_anchor_queue_item_id(conn: &Connection) -> Result<Option<i64>> {
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if let Some(queue_item_id) = current_queue_item_id {
        let queue_track_id: Option<Option<i64>> = conn
            .query_row(
                "SELECT track_id FROM queue WHERE id = ?1",
                params![queue_item_id],
                |row| row.get(0),
            )
            .optional()?;
        if queue_track_id
            .map(|track_id| track_id == current_track_id)
            .unwrap_or(false)
        {
            return Ok(Some(queue_item_id));
        }
    }

    if let Some(track_id) = current_track_id {
        let repaired_queue_item_id: Option<i64> = conn
            .query_row(
                "SELECT id
                 FROM queue
                 WHERE track_id = ?1
                 ORDER BY position ASC, id ASC
                 LIMIT 1",
                params![track_id],
                |row| row.get(0),
            )
            .optional()?;
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
            params![repaired_queue_item_id],
        )?;
        return Ok(repaired_queue_item_id);
    }

    if current_queue_item_id.is_some() {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = NULL WHERE id = 1",
            [],
        )?;
    }
    Ok(None)
}

pub fn set_shuffle_mode(conn: &Connection, mode: ShuffleMode) -> Result<ShuffleModeUpdate> {
    let current_queue_item_id = current_shuffle_anchor_queue_item_id(conn)?;
    let seed = (mode != ShuffleMode::Off).then(generate_shuffle_seed);
    conn.execute(
        "UPDATE playback_state SET shuffle_mode = ?1, shuffle_seed = ?2 WHERE id = 1",
        params![mode.as_str(), seed],
    )?;
    let debug = match seed {
        Some(seed) => {
            queue::apply_shuffle_with_seed(
                conn,
                mode,
                current_queue_item_id,
                seed,
                "playback_state",
            )?
            .debug
        }
        None => None,
    };
    Ok(ShuffleModeUpdate {
        snapshot: load_snapshot(conn)?,
        debug,
    })
}

pub fn set_repeat_mode(conn: &Connection, mode: &str) -> Result<PlaybackSnapshot> {
    let mode = match mode {
        "all" | "one" => mode,
        _ => "off",
    };
    conn.execute(
        "UPDATE playback_state SET repeat_mode = ?1 WHERE id = 1",
        params![mode],
    )?;
    load_snapshot(conn)
}

pub fn set_crossfade_ms(conn: &Connection, crossfade_ms: i32) -> Result<()> {
    conn.execute(
        "UPDATE playback_state SET crossfade_ms = ?1 WHERE id = 1",
        params![crossfade_ms.max(0)],
    )?;
    Ok(())
}

pub fn remove_queue_item_and_reconcile(
    conn: &Connection,
    item_id: i64,
) -> Result<RemoveQueueItemOutcome> {
    let rows: Vec<(i64, Option<i64>, i32)> = {
        let mut stmt =
            conn.prepare("SELECT id, track_id, position FROM queue ORDER BY position ASC, id ASC")?;
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let target = rows.iter().find(|(id, _, _)| *id == item_id).copied();
    let (current_queue_item_id, current_track_id, was_playing): (Option<i64>, Option<i64>, bool) =
        conn.query_row(
            "SELECT current_queue_item_id, current_track_id, is_playing FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? != 0)),
        )?;
    let resolved_current_queue_item_id = match current_queue_item_id {
        Some(queue_item_id)
            if rows
                .iter()
                .any(|(id, track_id, _)| *id == queue_item_id && *track_id == current_track_id) =>
        {
            Some(queue_item_id)
        }
        _ => current_track_id.and_then(|track_id| {
            rows.iter()
                .find(|(_, row_track_id, _)| *row_track_id == Some(track_id))
                .map(|(id, _, _)| *id)
        }),
    };
    if resolved_current_queue_item_id != current_queue_item_id {
        conn.execute(
            "UPDATE playback_state SET current_queue_item_id = ?1 WHERE id = 1",
            params![resolved_current_queue_item_id],
        )?;
    }

    let removed_current = target.is_some() && resolved_current_queue_item_id == Some(item_id);
    let next_current = target.and_then(|(_, _, target_pos)| {
        rows.iter()
            .find(|(id, _, position)| *id != item_id && *position > target_pos)
            .or_else(|| rows.iter().find(|(id, _, _)| *id != item_id))
            .copied()
    });

    queue::remove_queue_item(conn, item_id)?;

    if removed_current {
        match next_current {
            Some((queue_item_id, track_id, _)) => {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = ?1,
                         current_queue_item_id = ?2,
                         position_ms = 0,
                         is_playing = ?3
                     WHERE id = 1",
                    params![track_id, queue_item_id, was_playing],
                )?;
            }
            None => {
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = NULL,
                         current_queue_item_id = NULL,
                         position_ms = 0,
                         is_playing = 0
                     WHERE id = 1",
                    [],
                )?;
            }
        }
    }

    Ok(RemoveQueueItemOutcome {
        snapshot: load_snapshot(conn)?,
        removed_current,
        was_playing,
    })
}

pub fn next_track(conn: &Connection, recently_cleared: bool) -> Result<PlaybackSnapshot> {
    let repeat_mode: String = conn.query_row(
        "SELECT repeat_mode FROM playback_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let queue_items = ensure_automix_queue_depth(conn, AUTOMIX_MIN_UPCOMING, recently_cleared)?;
    if queue_items.is_empty() {
        conn.execute(
            "UPDATE playback_state SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 0, position_ms = 0 WHERE id = 1",
            [],
        )?;
        return load_snapshot(conn);
    }

    let current_index =
        playback_anchor_index(&queue_items, current_track_id, current_queue_item_id);
    let has_no_anchor = current_track_id.is_none() && current_queue_item_id.is_none();

    let next_track = match repeat_mode.as_str() {
        "one" => current_index
            .and_then(|idx| queue_items.get(idx))
            .or_else(|| has_no_anchor.then(|| queue_items.first()).flatten()),
        _ => current_index
            .and_then(|idx| queue_items.get(idx + 1))
            .or_else(|| {
                if has_no_anchor || repeat_mode == "all" {
                    queue_items.first()
                } else {
                    None
                }
            }),
    };

    if let Some(item) = next_track {
        // Pending rows have track.id == 0 (COALESCE sentinel); write NULL so the FK is not
        // violated. current_queue_item_id tracks position for the next advance.
        let new_track_id: Option<i64> = if item.track.id != 0 {
            Some(item.track.id)
        } else {
            None
        };
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, current_queue_item_id = ?2, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![new_track_id, item.id],
        )?;
    } else {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL, position_ms = 0, is_playing = 0
             WHERE id = 1",
            [],
        )?;
    }

    load_snapshot(conn)
}

pub fn start_queue_from_beginning(
    conn: &Connection,
    recently_cleared: bool,
) -> Result<PlaybackSnapshot> {
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = NULL,
             current_queue_item_id = NULL,
             position_ms = 0
         WHERE id = 1",
        [],
    )?;
    next_track(conn, recently_cleared)
}

/// Elapsed playback below which "previous" navigates back; at or above it,
/// "previous" restarts the current track. Shared by the player logic and the
/// route-level restart short-circuit so the two can never disagree.
pub const PREVIOUS_RESTART_THRESHOLD_MS: i64 = 3_000;

/// A play-history target for `previous_track`: the queue row that actually
/// played before the current one. `track_id` is `None` for rows that were
/// pending when they played. Validated against the live queue before use;
/// a stale anchor (row removed, row re-resolved to a different track) falls
/// back to queue-order stepping.
#[derive(Debug, Clone, Copy)]
pub struct HistoryAnchor {
    pub queue_item_id: i64,
    pub track_id: Option<i64>,
}

pub struct PreviousTrackOutcome {
    pub snapshot: PlaybackSnapshot,
    /// True when the decision was "restart what is already playing" (elapsed
    /// past the threshold, or already at the head of the queue with no
    /// history). The route turns this into a runtime seek-to-0 when the
    /// track is audibly active instead of a full stream re-resolve + switch.
    pub restart_in_place: bool,
}

/// Move to the previous track.
///
/// `live_position_ms` is the AUDIBLE playhead read from the runtime by the
/// caller. The DB's own `position_ms` is not consulted: nothing persists the
/// live playhead into it during playback, so it reads 0 mid-track (that
/// stale read is what kept the restart branch from ever firing).
///
/// `history_anchor` is the most recent play-history entry that still
/// resolves against the queue, if the caller has one; it wins over
/// queue-order stepping so "previous" follows what actually played across
/// shuffle, manual jumps, and automix insertions.
pub fn previous_track(
    conn: &Connection,
    live_position_ms: i64,
    history_anchor: Option<&HistoryAnchor>,
) -> Result<PreviousTrackOutcome> {
    let (current_track_id, current_queue_item_id): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT current_track_id, current_queue_item_id FROM playback_state WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let queue_items = queue::load_queue(conn)?;
    if queue_items.is_empty() {
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL,
                 position_ms = 0, is_playing = 0
             WHERE id = 1",
            [],
        )?;
        return Ok(PreviousTrackOutcome {
            snapshot: load_snapshot(conn)?,
            restart_in_place: false,
        });
    }

    // Restart in place when past the threshold.
    if (current_track_id.is_some() || current_queue_item_id.is_some())
        && live_position_ms >= PREVIOUS_RESTART_THRESHOLD_MS
    {
        conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
        return Ok(PreviousTrackOutcome {
            snapshot: load_snapshot(conn)?,
            restart_in_place: true,
        });
    }

    let anchor_to = |item: &QueueItem| -> Result<PlaybackSnapshot> {
        let new_track_id: Option<i64> = if item.track.id != 0 {
            Some(item.track.id)
        } else {
            None
        };
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = ?1, current_queue_item_id = ?2,
                 position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![new_track_id, item.id],
        )?;
        load_snapshot(conn)
    };

    // Play history wins over queue-order stepping when its row still
    // resolves. Re-validated here even though the route pre-checks, so a
    // queue edit between the two reads cannot anchor onto the wrong row.
    if let Some(anchor) = history_anchor
        && let Some(item) = queue_items
            .iter()
            .find(|item| item.id == anchor.queue_item_id)
    {
        let item_track_id = (item.track.id != 0).then_some(item.track.id);
        if item_track_id == anchor.track_id {
            return Ok(PreviousTrackOutcome {
                snapshot: anchor_to(item)?,
                restart_in_place: false,
            });
        }
    }

    let current_index =
        playback_anchor_index(&queue_items, current_track_id, current_queue_item_id);

    if let Some(previous_item) = current_index
        .and_then(|idx| idx.checked_sub(1))
        .and_then(|idx| queue_items.get(idx))
    {
        return Ok(PreviousTrackOutcome {
            snapshot: anchor_to(previous_item)?,
            restart_in_place: false,
        });
    }

    // Nothing was playing - jump to the first item rather than doing nothing.
    if current_index.is_none()
        && let Some(first_item) = queue_items.first()
    {
        return Ok(PreviousTrackOutcome {
            snapshot: anchor_to(first_item)?,
            restart_in_place: false,
        });
    }

    // Already at the start of the queue with no history - restart current.
    conn.execute("UPDATE playback_state SET position_ms = 0 WHERE id = 1", [])?;
    Ok(PreviousTrackOutcome {
        snapshot: load_snapshot(conn)?,
        restart_in_place: true,
    })
}

pub fn current_track_id(conn: &Connection) -> Result<Option<i64>> {
    let current_track_id = conn.query_row(
        "SELECT current_track_id FROM playback_state WHERE id = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(current_track_id)
}

/// Returns the track that would play next **without** advancing the queue or
/// mutating any playback state. Used for gapless pre-buffering.
pub fn peek_next_track(conn: &Connection, recently_cleared: bool) -> Result<Option<Track>> {
    let (current_track_id, current_queue_item_id, repeat_mode): (Option<i64>, Option<i64>, String) =
        conn.query_row(
            "SELECT current_track_id, current_queue_item_id, repeat_mode FROM playback_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

    let queue_items = ensure_automix_queue_depth(conn, AUTOMIX_MIN_UPCOMING, recently_cleared)?;
    if queue_items.is_empty() {
        return Ok(None);
    }

    let current_index =
        playback_anchor_index(&queue_items, current_track_id, current_queue_item_id);
    let has_no_anchor = current_track_id.is_none() && current_queue_item_id.is_none();

    let next = match repeat_mode.as_str() {
        "one" => current_index
            .and_then(|idx| queue_items.get(idx))
            .or_else(|| has_no_anchor.then(|| queue_items.first()).flatten()),
        _ => current_index
            .and_then(|idx| queue_items.get(idx + 1))
            .or_else(|| {
                if has_no_anchor || repeat_mode == "all" {
                    queue_items.first()
                } else {
                    None
                }
            }),
    };

    Ok(next.map(|item| item.track.clone()))
}

pub(crate) fn preferred_tidal_quality(track: &Track, user_pref: Option<AudioQuality>) -> String {
    if let Some(q) = user_pref {
        return q.as_tidal_str().to_string();
    }
    track
        .best_quality
        .clone()
        .unwrap_or_else(|| stream::DEFAULT_AUDIO_QUALITY.to_string())
}

pub fn build_tidal_stream_request(
    track: &Track,
    user_pref: Option<AudioQuality>,
) -> Option<StreamRequest> {
    track
        .tidal_id
        .map(|track_id| StreamRequest::new(track_id, preferred_tidal_quality(track, user_pref)))
}

pub fn build_playback_preparation(
    track: &Track,
    stream_info: Option<&StreamInfo>,
    crossfade_ms: i32,
    user_pref: Option<AudioQuality>,
) -> PlaybackPreparation {
    let source = build_tidal_stream_request(track, user_pref)
        .map(PlaybackSourceRequest::TidalStream)
        .unwrap_or(PlaybackSourceRequest::LocalLibrary);
    let gapless = gapless::plan_from_stream(stream_info, GaplessSettings::new(true, crossfade_ms));

    let output_sample_rate = stream_info.and_then(StreamInfo::sample_rate_hz);

    let mut job = PreparedPlaybackJob {
        output_sample_rate,
        ..PreparedPlaybackJob::new(track.clone(), source, gapless)
    };
    if let Some(media_ref) = crate::playback::dj_lookahead::tidal_media_ref_for_track(track) {
        job = job.with_dj_media_ref(media_ref);
    }
    job
}

pub fn playback_source_kind(track: &Track) -> &'static str {
    if track.tidal_id.is_some() {
        "tidal"
    } else {
        "local"
    }
}

pub fn listen_completion_threshold_ms(track: &Track) -> Option<i64> {
    track
        .duration_ms
        .map(|duration_ms| ((duration_ms as f64 * 0.9) as i64).min(240_000))
}

pub fn is_completed_listen(track: &Track, listened_ms: i64) -> bool {
    listen_completion_threshold_ms(track)
        .map(|threshold_ms| listened_ms >= threshold_ms)
        .unwrap_or(listened_ms >= 240_000)
}

/// Cap a flushed listen-session duration at the track's length when known.
///
/// The session timer accrues wall-clock time while the player is nominally
/// playing, so a stalled stream or a player stuck at end-of-queue can record
/// arbitrarily long listens (observed: 2795 s on a 334 s track). Capping at
/// the track length bounds the damage. Trade-off: a repeat-one loop flushes
/// as a single session and loses its extra loop time; position-based
/// accounting is the proper fix (see FOLLOWUPS).
pub fn clamp_listened_ms(listened_ms: i64, track_duration_ms: Option<i64>) -> i64 {
    match track_duration_ms {
        Some(duration_ms) if duration_ms > 0 => listened_ms.min(duration_ms),
        _ => listened_ms,
    }
}

pub(super) fn playback_anchor_index(
    queue_items: &[QueueItem],
    current_track_id: Option<i64>,
    current_queue_item_id: Option<i64>,
) -> Option<usize> {
    if let Some(qid) = current_queue_item_id
        && let Some(idx) = queue_items.iter().position(|item| item.id == qid)
    {
        let queue_track_id = (queue_items[idx].track.id != 0).then_some(queue_items[idx].track.id);
        if queue_track_id == current_track_id {
            return Some(idx);
        }
    }

    current_track_id.and_then(|track_id| {
        queue_items
            .iter()
            .position(|item| item.track.id == track_id)
    })
}

pub(crate) fn build_session_taste_profile(
    conn: &Connection,
    current_track: &Track,
) -> Result<SessionTasteProfile> {
    let mut profile = SessionTasteProfile {
        current_artist_id: Some(current_track.artist_id),
        current_album_id: current_track.album_id,
        current_source: Some(current_track.source.clone()),
        ..SessionTasteProfile::default()
    };

    if current_track.artist_id != 0 {
        *profile
            .positive_artists
            .entry(current_track.artist_id)
            .or_insert(0.0) += 3.0;
    }

    let current_track_genres = queue::get_track_genres(conn, std::slice::from_ref(current_track))?;
    for genre in current_track_genres
        .get(&current_track.id)
        .into_iter()
        .flat_map(|genres| genres.iter())
    {
        let normalized = normalize_genre_key(genre);
        profile.current_genres.insert(normalized.clone());
        *profile.positive_genres.entry(normalized).or_insert(0.0) += 2.2;
    }

    let mut stmt = conn.prepare(
        "SELECT track_id, completed
         FROM listen_history
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let feedback_rows = stmt
        .query_map(params![SESSION_FEEDBACK_LIMIT], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut feedback_tracks = Vec::new();
    let mut feedback_entries = Vec::new();

    // Batch load all feedback tracks in a single query instead of N individual SELECTs.
    let feedback_track_ids: Vec<i64> = feedback_rows.iter().map(|(id, _)| *id).collect();
    let found_tracks = queue::get_tracks_by_ids(conn, &feedback_track_ids)?;
    let track_map: HashMap<i64, &Track> = found_tracks.iter().map(|t| (t.id, t)).collect();

    for (track_id, completed) in feedback_rows {
        profile.recent_track_ids.insert(track_id);
        if !completed {
            profile.skipped_track_ids.insert(track_id);
        }

        if let Some(track) = track_map.get(&track_id) {
            feedback_tracks.push((**track).clone());
            feedback_entries.push(((**track).clone(), completed));
        }
    }

    let feedback_genres = queue::get_track_genres(conn, &feedback_tracks)?;
    for (index, (track, completed)) in feedback_entries.iter().enumerate() {
        let recency = (SESSION_FEEDBACK_LIMIT - index as i64).max(1) as f64 / 6.0;
        let artist_weight = if *completed {
            0.8 + recency
        } else {
            1.1 + recency
        };
        let genre_weight = if *completed {
            0.7 + recency
        } else {
            1.0 + recency
        };

        let artist_buckets = if *completed {
            &mut profile.positive_artists
        } else {
            &mut profile.negative_artists
        };
        if track.artist_id != 0 {
            *artist_buckets.entry(track.artist_id).or_insert(0.0) += artist_weight;
        }

        let genre_buckets = if *completed {
            &mut profile.positive_genres
        } else {
            &mut profile.negative_genres
        };
        for genre in feedback_genres
            .get(&track.id)
            .into_iter()
            .flat_map(|genres| genres.iter())
        {
            let normalized = normalize_genre_key(genre);
            *genre_buckets.entry(normalized).or_insert(0.0) += genre_weight;
        }
    }

    Ok(profile)
}

pub(super) fn normalize_genre_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playback::automix::{
        automix_score, automix_scored_reason, build_automix_extension_with_reasons,
        evaluate_automix_for_seed,
    };

    #[test]
    fn clamp_listened_ms_caps_runaway_sessions_at_track_length() {
        // Wall-clock accrual during a stalled stream must not outlive the
        // track (observed: 2795 s recorded on a 334 s track).
        assert_eq!(clamp_listened_ms(2_795_000, Some(334_000)), 334_000);
        assert_eq!(clamp_listened_ms(200_000, Some(334_000)), 200_000);
    }

    #[test]
    fn clamp_listened_ms_passes_through_unknown_durations() {
        assert_eq!(clamp_listened_ms(2_795_000, None), 2_795_000);
        assert_eq!(clamp_listened_ms(2_795_000, Some(0)), 2_795_000);
        assert_eq!(clamp_listened_ms(2_795_000, Some(-1)), 2_795_000);
    }

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, year INTEGER, artwork_url TEXT);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT DEFAULT 'tidal'
            );
            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER,
                ephemeral_album_title TEXT,
                ephemeral_artwork_url TEXT,
                ephemeral_duration_ms INTEGER,
                ephemeral_artist_tidal_id INTEGER,
                ephemeral_album_tidal_id INTEGER
            );
            CREATE TABLE genres (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                parent_id INTEGER
            );
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );
            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );
            CREATE TABLE embedding_models (
                id INTEGER PRIMARY KEY,
                model_key TEXT NOT NULL UNIQUE,
                family TEXT NOT NULL,
                dimension INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                is_active INTEGER NOT NULL DEFAULT 0,
                trained_at TEXT,
                config_json TEXT,
                metrics_json TEXT,
                created_at TEXT DEFAULT (datetime('now'))
            );
            CREATE TABLE server_config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE external_track_candidates (
                id INTEGER PRIMARY KEY,
                tidal_id INTEGER,
                mbid TEXT,
                dedupe_key TEXT NOT NULL UNIQUE,
                normalized_artist_name TEXT NOT NULL DEFAULT '',
                normalized_title TEXT NOT NULL DEFAULT '',
                duration_bucket INTEGER NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                artist_name TEXT NOT NULL,
                genre_tags_json TEXT,
                duration_ms INTEGER,
                expires_at TEXT NOT NULL,
                resolved_track_id INTEGER,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );
            CREATE TABLE external_track_candidate_neighbors (
                library_track_id INTEGER NOT NULL,
                candidate_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                rank INTEGER NOT NULL,
                score REAL NOT NULL DEFAULT 0,
                audio_score REAL NOT NULL DEFAULT 0,
                metadata_score REAL NOT NULL DEFAULT 0,
                reason_json TEXT,
                computed_at TEXT DEFAULT (datetime('now')),
                PRIMARY KEY (library_track_id, candidate_id, model_id)
            );
            CREATE TABLE track_similarity (
                track_a INTEGER NOT NULL,
                track_b INTEGER NOT NULL,
                similarity_score REAL NOT NULL DEFAULT 0,
                co_listen_score REAL DEFAULT 0,
                co_album_score REAL DEFAULT 0,
                co_artist_score REAL DEFAULT 0,
                genre_proximity REAL DEFAULT 0,
                duration_proximity REAL DEFAULT 0,
                era_proximity REAL DEFAULT 0,
                computed_at TEXT,
                PRIMARY KEY (track_a, track_b)
            );
            CREATE TABLE playback_state (
                id INTEGER PRIMARY KEY,
                current_track_id INTEGER,
                current_queue_item_id INTEGER,
                shuffle_seed INTEGER,
                position_ms INTEGER NOT NULL DEFAULT 0,
                is_playing INTEGER NOT NULL DEFAULT 0,
                volume REAL NOT NULL DEFAULT 1.0,
                shuffle_mode TEXT NOT NULL DEFAULT 'off',
                repeat_mode TEXT NOT NULL DEFAULT 'off',
                automix_enabled INTEGER NOT NULL DEFAULT 0,
                crossfade_ms INTEGER NOT NULL DEFAULT 0,
                automix_discover_new INTEGER NOT NULL DEFAULT 0,
                automix_use_learning INTEGER NOT NULL DEFAULT 1,
                automix_allow_external INTEGER NOT NULL DEFAULT 0
            );
            ",
        )
        .unwrap();

        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])
            .unwrap();
        for id in 1..=6 {
            conn.execute(
                "INSERT INTO tracks (
                    id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc,
                    tidal_id, ytmusic_id, soundcloud_id, best_quality, best_source, fidelity_score,
                    is_favorite, play_count, last_played_at, date_added, source
                ) VALUES (?1, ?2, 1, NULL, 1, ?1, 180000, NULL, ?1, NULL, NULL, 'LOSSLESS', 'tidal', 10, 0, 0, NULL, '2025-01-01', 'tidal')",
                params![id, format!("Track {id}")],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO playback_state (
                id, current_track_id, position_ms, is_playing, volume, shuffle_mode, repeat_mode, automix_enabled, crossfade_ms
            ) VALUES (1, NULL, 0, 0, 1.0, 'off', 'off', 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score, co_artist_score)
             VALUES (2, 3, 0.95, 1.0)",
            [],
        )
        .unwrap();

        conn
    }

    fn load_tracks(conn: &Connection, ids: &[i64]) -> Vec<Track> {
        ids.iter()
            .map(|id| queue::get_track_by_id(conn, *id).unwrap().unwrap())
            .collect()
    }

    fn attach_test_dj_transition_plan(
        db: &Database,
        job: PlaybackPreparation,
        sample_rate: u32,
        channels: u16,
    ) -> Result<PlaybackPreparation> {
        let pair = db.with_conn(load_dj_lookahead_pair)?;
        attach_dj_transition_plan_for_pair(
            &DjEngine::new(db.clone()),
            job,
            pair,
            sample_rate,
            channels,
        )
    }

    mod dj_lookahead {
        use super::*;

        const DEADLINE: u64 = 48_000 * 30;

        fn enable(conn: &Connection) {
            queries::set_dj_engine_enabled(conn, true).unwrap();
        }

        fn start(conn: &Connection) -> Option<DjLookaheadStart> {
            if !queries::is_dj_engine_enabled(conn).unwrap() {
                return None;
            }
            let pair = load_dj_lookahead_pair(conn).unwrap();
            dj_lookahead_start_from_pair(pair, DEADLINE)
        }

        fn seed_queue(conn: &Connection, ids: &[i64]) -> Vec<QueueItem> {
            let tracks = load_tracks(conn, ids);
            queue::replace_queue(conn, &tracks, "test").unwrap()
        }

        #[test]
        fn dj_disabled_queue_events_do_not_start_dj_lookahead() {
            let conn = conn();
            seed_queue(&conn, &[1, 2]);
            play_track_now(&conn, 1).unwrap();

            assert!(start(&conn).is_none());
        }

        #[test]
        fn dj_enable_starts_dj_lookahead_for_active_pair() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1, 2]);
            conn.execute(
                "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
                params![queued[0].id],
            )
            .unwrap();

            let start = start(&conn).expect("lookahead");
            assert_eq!(start.current_queue_item_id, Some(queued[0].id));
            assert_eq!(start.next_queue_item_id, Some(queued[1].id));
            assert_eq!(start.deadline_samples, DEADLINE);
        }

        #[test]
        fn manual_play_starts_dj_lookahead() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1, 2]);
            play_track_now(&conn, 1).unwrap();

            let start = start(&conn).expect("lookahead");
            assert_eq!(start.current_queue_item_id, Some(queued[0].id));
            assert_eq!(start.next_queue_item_id, Some(queued[1].id));
        }

        #[test]
        fn manual_queue_append_starts_dj_lookahead() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1]);
            conn.execute(
                "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
                params![queued[0].id],
            )
            .unwrap();
            enqueue_track(&conn, 2, "user").unwrap();

            let start = start(&conn).expect("lookahead");
            assert_eq!(start.current_queue_item_id, Some(queued[0].id));
            assert!(matches!(
                start.next,
                Some(DjMediaRef::TidalTrack { tidal_id: 2, .. })
            ));
        }

        #[test]
        fn play_next_starts_dj_lookahead() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1, 3]);
            conn.execute(
                "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
                params![queued[0].id],
            )
            .unwrap();
            let play_next = load_tracks(&conn, &[2]).remove(0);
            queue::append_tracks(&conn, &[play_next], "user_play_next").unwrap();
            let inserted = queue::load_queue(&conn).unwrap();
            let inserted_id = inserted.iter().find(|item| item.track.id == 2).unwrap().id;
            queue::move_queue_item(&conn, inserted_id, 1).unwrap();

            let start = start(&conn).expect("lookahead");
            assert_eq!(start.next_queue_item_id, Some(inserted_id));
        }

        #[test]
        fn queue_reorder_restarts_dj_lookahead() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1, 2, 3]);
            conn.execute(
                "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
                params![queued[0].id],
            )
            .unwrap();
            let before = start(&conn).expect("before");
            queue::move_queue_item(&conn, queued[2].id, 1).unwrap();

            let after = start(&conn).expect("after");
            assert_ne!(before.next_queue_item_id, after.next_queue_item_id);
            assert_eq!(after.next_queue_item_id, Some(queued[2].id));
        }

        #[test]
        fn pending_resolution_restarts_dj_lookahead_with_tidal_ref() {
            let conn = conn();
            enable(&conn);
            let queued = seed_queue(&conn, &[1]);
            conn.execute(
                "UPDATE playback_state SET current_track_id = 1, current_queue_item_id = ?1 WHERE id = 1",
                params![queued[0].id],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
                 VALUES (NULL, 1, 'radio_pending', 'Artist', 'Title')",
                [],
            )
            .unwrap();
            let pending_id = conn.last_insert_rowid();
            let before = start(&conn).expect("before");
            conn.execute(
                "UPDATE queue SET tidal_id_hint = 99 WHERE id = ?1",
                params![pending_id],
            )
            .unwrap();

            let after = start(&conn).expect("after");
            assert_ne!(before.queue_generation, after.queue_generation);
            assert!(matches!(
                after.next,
                Some(DjMediaRef::PendingQueueItem {
                    tidal_id_hint: Some(99),
                    ..
                })
            ));
        }
    }

    mod dj_prepare_next {
        use super::*;
        use crate::db::schema;

        fn db_with_pair(next_source: &str) -> Database {
            let db = Database::open_in_memory().expect("db");
            db.with_conn(|conn| {
                schema::run_migrations(conn)?;
                conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
                for id in 1..=4 {
                    conn.execute(
                        "INSERT INTO tracks (
                            id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                        ) VALUES (?1, ?2, 1, ?1, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                        params![id, format!("Track {id}")],
                    )?;
                }
                conn.execute(
                    "INSERT INTO queue (id, track_id, position, source)
                     VALUES (11, 1, 0, 'manual'), (12, 2, 1, ?1)",
                    params![next_source],
                )?;
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                     WHERE id = 1",
                    [],
                )?;
                Ok(())
            })
            .expect("seed db");
            db
        }

        fn enable(db: &Database) {
            db.with_conn(|conn| queries::set_dj_engine_enabled(conn, true))
                .expect("enable");
        }

        fn next_job(db: &Database) -> PlaybackPreparation {
            db.with_conn(|conn| {
                let track = queue::get_track_by_id(conn, 2)?.expect("track");
                Ok(build_playback_preparation(&track, None, 0, None))
            })
            .expect("next job")
        }

        fn planned_job_for_source(source: &str) -> PlaybackPreparation {
            let db = db_with_pair(source);
            enable(&db);
            attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan")
        }

        #[test]
        fn prepare_next_attaches_program_when_enabled() {
            let job = planned_job_for_source("manual");

            assert!(job.prepared_transition.is_some());
        }

        #[test]
        fn prepared_dj_program_supplies_runtime_overlap() {
            let job = planned_job_for_source("manual");

            assert!(job.prepared_transition.is_some());
            assert!(job.gapless.enabled);
            assert!(job.gapless.overlap_ms > 0);
        }

        #[test]
        fn prepare_next_omits_program_when_disabled() {
            let db = db_with_pair("manual");
            let job = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");

            assert!(job.prepared_transition.is_none());
        }

        #[test]
        fn dj_planning_does_not_reorder_queue() {
            let db = db_with_pair("manual");
            enable(&db);
            let before = db
                .with_conn(|conn| {
                    let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position, id")?;
                    let rows = stmt
                        .query_map([], |row| row.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows)
                })
                .expect("before");

            let _ = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");
            let after = db
                .with_conn(|conn| {
                    let mut stmt = conn.prepare("SELECT id FROM queue ORDER BY position, id")?;
                    let rows = stmt
                        .query_map([], |row| row.get::<_, i64>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(rows)
                })
                .expect("after");

            assert_eq!(after, before);
        }

        #[test]
        fn dj_planning_does_not_replace_next_queue_item() {
            let db = db_with_pair("manual");
            enable(&db);

            let _ = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");
            let next_queue_track = db
                .with_conn(|conn| {
                    conn.query_row("SELECT track_id FROM queue WHERE id = 12", [], |row| {
                        row.get::<_, Option<i64>>(0)
                    })
                    .map_err(anyhow::Error::from)
                })
                .expect("next queue item");

            assert_eq!(next_queue_track, Some(2));
        }

        #[test]
        fn manual_queue_next_uses_plan_transition_path() {
            assert!(
                planned_job_for_source("manual")
                    .prepared_transition
                    .is_some()
            );
        }

        #[test]
        fn radio_queue_next_uses_plan_transition_path() {
            assert!(
                planned_job_for_source("radio")
                    .prepared_transition
                    .is_some()
            );
        }

        #[test]
        fn automix_queue_next_uses_plan_transition_path() {
            assert!(
                planned_job_for_source("automix-new")
                    .prepared_transition
                    .is_some()
            );
        }

        #[test]
        fn external_next_track_uses_same_plan_transition_path() {
            assert!(
                planned_job_for_source("radio_pending")
                    .prepared_transition
                    .is_some()
            );
        }

        #[test]
        fn pending_next_without_profile_falls_back_to_safe_crossfade() {
            let db = Database::open_in_memory().expect("db");
            db.with_conn(|conn| {
                schema::run_migrations(conn)?;
                conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
                conn.execute(
                    "INSERT INTO tracks (
                        id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                     ) VALUES (1, 'Track 1', 1, 1, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title)
                     VALUES (11, 1, 0, 'manual', NULL, NULL),
                            (12, NULL, 1, 'radio_pending', 'External A', 'External B')",
                    [],
                )?;
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                     WHERE id = 1",
                    [],
                )?;
                queries::set_dj_engine_enabled(conn, true)?;
                Ok(())
            })
            .expect("seed pending");
            let pair = db.with_conn(load_dj_lookahead_pair).expect("pair");
            let engine = DjEngine::new(db.clone());
            let job = attach_dj_transition_plan_for_pair(
                &engine,
                PreparedPlaybackJob::test_fixture(99, 1),
                pair,
                48_000,
                2,
            )
            .expect("plan");

            let program = job.prepared_transition.expect("transition").program;
            assert_eq!(program.template, "SafeCrossfade");
        }
    }

    mod dj_transition_logging {
        use super::*;
        use crate::db::models::{
            AudioDjProfileCorrectionRow, AudioDjProfileKey, AudioDjProfileRow, AudioDspFeatures,
        };
        use crate::db::schema;
        use crate::services::audio_analysis::dj_profile::{
            DJ_PROFILE_VERSION, encode_f32_blob, encode_u32_blob,
        };
        use serde_json::Value;

        fn db_with_pair() -> Database {
            let db = Database::open_in_memory().expect("db");
            db.with_conn(|conn| {
                schema::run_migrations(conn)?;
                conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
                conn.execute(
                    "INSERT INTO tracks (
                        id, title, artist_id, tidal_id, source, best_quality, best_source, duration_ms
                    ) VALUES (1, 'Track 1', 1, 1, 'tidal', 'LOSSLESS', 'tidal', 180000),
                             (2, 'Track 2', 1, 2, 'tidal', 'LOSSLESS', 'tidal', 180000)",
                    [],
                )?;
                conn.execute(
                    "INSERT INTO queue (id, track_id, position, source)
                     VALUES (11, 1, 0, 'manual'), (12, 2, 1, 'manual')",
                    [],
                )?;
                conn.execute(
                    "UPDATE playback_state
                     SET current_track_id = 1, current_queue_item_id = 11, is_playing = 1
                     WHERE id = 1",
                    [],
                )?;
                queries::set_dj_engine_enabled(conn, true)?;
                seed_dsp(conn, 1, "8A")?;
                seed_dsp(conn, 2, "8A")?;
                seed_profile(conn, "tidal_track", "1", Some(1))?;
                seed_profile(conn, "tidal_track", "2", Some(2))?;
                Ok(())
            })
            .expect("seed");
            db
        }

        fn seed_profile(
            conn: &Connection,
            kind: &str,
            id: &str,
            track_id: Option<i64>,
        ) -> Result<()> {
            let row = AudioDjProfileRow {
                media_ref_kind: kind.to_string(),
                media_ref_id: id.to_string(),
                track_id,
                queue_item_id: None,
                tidal_id: id.parse().ok(),
                profile_version: DJ_PROFILE_VERSION.to_string(),
                beat_grid_blob: encode_f32_blob(
                    &(0..64).map(|i| i as f32 * 0.5).collect::<Vec<_>>(),
                ),
                downbeats_blob: encode_f32_blob(
                    &(0..16).map(|i| i as f32 * 2.0).collect::<Vec<_>>(),
                ),
                phrase_boundaries_blob: encode_u32_blob(&(0..2).collect::<Vec<_>>()),
                mix_in_blob: encode_f32_blob(&[0.0]),
                mix_out_blob: encode_f32_blob(&[90.0]),
                intro_end_seconds: Some(16.0),
                outro_start_seconds: Some(120.0),
                breakdown_blob: encode_f32_blob(&[]),
                drop_blob: encode_f32_blob(&[]),
                safe_transition_windows_blob: encode_f32_blob(&[0.0, 8.0, 1.0]),
                energy_contour_blob: encode_f32_blob(&[]),
                vocal_presence_blob: encode_f32_blob(&[0.0; 2]),
                vocal_density_blob: encode_f32_blob(&[0.0; 2]),
                waveform_peaks_blob: encode_f32_blob(&[0.0, 0.5, 1.0, 0.5]),
                lufs_loud_body: Some(-12.0),
                true_peak_dbtp: Some(-1.0),
                beat_confidence: Some(0.9),
                profile_confidence: 0.9,
                analysis_scope_ms: 90_000,
                is_temporary: false,
                source: "test".to_string(),
                computed_at: "now".to_string(),
            };
            queries::upsert_audio_dj_profile(conn, &row)
        }

        fn seed_dsp(conn: &Connection, track_id: i64, camelot_key: &str) -> Result<()> {
            queries::upsert_audio_dsp_features(
                conn,
                &AudioDspFeatures {
                    track_id,
                    bpm: Some(120.0),
                    key_signature: None,
                    camelot_key: Some(camelot_key.to_string()),
                    loudness_lufs: Some(-12.0),
                    energy: Some(0.5),
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

        fn next_job(db: &Database) -> PlaybackPreparation {
            db.with_conn(|conn| {
                let track = queue::get_track_by_id(conn, 2)?.expect("track");
                Ok(build_playback_preparation(&track, None, 0, None))
            })
            .expect("job")
        }

        fn plan(db: &Database) -> PreparedTransitionProgram {
            attach_test_dj_transition_plan(db, next_job(db), 48_000, 2)
                .expect("plan")
                .prepared_transition
                .expect("transition")
        }

        fn event_count(db: &Database) -> i64 {
            db.with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                    row.get(0)
                })
                .map_err(Into::into)
            })
            .expect("count")
        }

        fn insert_timing_sample(
            conn: &Connection,
            delta_ms: i64,
            runtime_rendered_dj_mixer: bool,
            runtime_renderer_status: &str,
            runtime_renderer_reason: &str,
        ) -> Result<()> {
            conn.execute(
                "INSERT INTO dj_transition_events (
                    from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                    template, program_json, planner_version, timing_delta_ms, timing_status,
                    runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
                 ) VALUES (
                    'tidal_track', '1', 'tidal_track', '2',
                    'SafeCrossfade', '{\"template\":\"SafeCrossfade\"}', 'dj-v1',
                    ?1, 'fired', ?2, ?3, ?4
                 )",
                params![
                    delta_ms,
                    if runtime_rendered_dj_mixer { 1 } else { 0 },
                    runtime_renderer_status,
                    runtime_renderer_reason,
                ],
            )?;
            Ok(())
        }

        fn make_pair_drop_tease_ready(db: &Database) {
            db.with_conn(|conn| {
                queries::set_dj_global_policy(conn, "bold", "neutral")?;
                let phrase_blob = encode_u32_blob(&(0..4).collect::<Vec<_>>());
                let drop_blob = encode_f32_blob(&[32.0]);
                let vocal_blob = encode_f32_blob(&[0.0; 4]);
                conn.execute(
                    "UPDATE audio_dj_profiles
                     SET phrase_boundaries_blob = ?1,
                         vocal_presence_blob = ?2,
                         vocal_density_blob = ?2
                     WHERE media_ref_id = '1'",
                    params![&phrase_blob, &vocal_blob],
                )?;
                conn.execute(
                    "UPDATE audio_dj_profiles
                     SET phrase_boundaries_blob = ?1,
                         drop_blob = ?2,
                         vocal_presence_blob = ?3,
                         vocal_density_blob = ?3
                     WHERE media_ref_id = '2'",
                    params![&phrase_blob, &drop_blob, &vocal_blob],
                )?;
                Ok(())
            })
            .expect("drop tease profile fixture");
        }

        #[test]
        fn latest_open_dj_transition_event_for_pair_prefers_fired_event() {
            let db = db_with_pair();
            let fired = plan(&db);
            let fired_id = fired.transition_event_id.expect("fired event");
            db.with_conn(|conn| {
                queries::update_dj_transition_fire_timing(
                    conn,
                    fired_id,
                    172_040,
                    "fired",
                    true,
                    "rendered_handoff",
                    "none",
                )
            })
            .expect("mark fired");
            let duplicate = plan(&db);
            assert_ne!(duplicate.transition_event_id, Some(fired_id));

            let selected = db
                .with_conn(|conn| latest_open_dj_transition_event_for_pair(conn, Some(1), 2))
                .expect("selected");

            assert_eq!(selected, Some(fired_id));
        }

        #[test]
        fn repeated_planning_reuses_existing_armed_event_for_pair() {
            let db = db_with_pair();
            let first = plan(&db);
            let second = plan(&db);

            assert_eq!(second.transition_event_id, first.transition_event_id);
            assert_eq!(event_count(&db), 1);
        }

        #[test]
        fn missing_profile_armed_event_replans_when_profile_arrives() {
            let db = db_with_pair();
            db.with_conn(|conn| {
                conn.execute(
                    "DELETE FROM audio_dj_profiles
                     WHERE media_ref_kind = 'tidal_track' AND media_ref_id = '2'",
                    [],
                )?;
                Ok(())
            })
            .expect("remove next profile");

            let fallback = plan(&db);
            let fallback_id = fallback.transition_event_id.expect("fallback event");
            assert_eq!(fallback.program.template, "SafeCrossfade");
            let fallback_reason: Option<String> = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT fallback_reason FROM dj_transition_events WHERE id = ?1",
                        params![fallback_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
                })
                .expect("fallback reason");
            assert_eq!(fallback_reason.as_deref(), Some("next_profile_missing"));

            db.with_conn(|conn| seed_profile(conn, "tidal_track", "2", Some(2)))
                .expect("seed next profile");
            let replanned = plan(&db);

            assert_eq!(replanned.transition_event_id, Some(fallback_id));
            assert_eq!(replanned.program.template, "BassSwap16");
            assert_eq!(event_count(&db), 1);
            let row: (String, Option<String>) = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT template, fallback_reason
                         FROM dj_transition_events WHERE id = ?1",
                        params![fallback_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("replanned row");
            assert_eq!(row.0, "BassSwap16");
            assert_eq!(row.1, None);
        }

        #[test]
        fn dj_disabled_does_not_write_dj_transition_events() {
            let db = db_with_pair();
            db.with_conn(|conn| queries::set_dj_engine_enabled(conn, false))
                .expect("disable");

            let job = attach_test_dj_transition_plan(&db, next_job(&db), 48_000, 2).expect("plan");

            assert!(job.prepared_transition.is_none());
            assert_eq!(event_count(&db), 0);
        }

        #[test]
        fn dj_transition_logging_does_not_replace_playback_transitions() {
            let db = db_with_pair();
            let _ = plan(&db);
            db.with_conn(|conn| {
                queries::record_playback_transition(conn, 1, 2, "queue", true, 8000)
            })
            .expect("legacy transition");

            let counts = db
                .with_conn(|conn| {
                    let dj: i64 =
                        conn.query_row("SELECT COUNT(*) FROM dj_transition_events", [], |row| {
                            row.get(0)
                        })?;
                    let legacy: i64 =
                        conn.query_row("SELECT COUNT(*) FROM playback_transitions", [], |row| {
                            row.get(0)
                        })?;
                    Ok((dj, legacy))
                })
                .expect("counts");

            assert_eq!(counts, (1, 1));
        }

        #[test]
        fn dj_transition_logging_stores_no_fabricated_rejected_alternatives() {
            let db = db_with_pair();
            let transition = plan(&db);
            let rejected: String = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT rejected_alternatives_json FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
                })
                .expect("rejected");
            let parsed: Vec<Value> = serde_json::from_str(&rejected).expect("json");

            // The planner is a decision tree, not a scorer, so it logs an empty
            // list rather than a fabricated per-alternative ranking.
            assert!(parsed.is_empty());
        }

        #[test]
        fn dj_transition_logging_uses_planner_version_not_profile_version() {
            let db = db_with_pair();
            let transition = plan(&db);
            let planner_version: String = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT planner_version FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
                })
                .expect("planner version");

            assert_eq!(planner_version, noor_mix::planner::DJ_PLANNER_VERSION);
            assert_ne!(planner_version, DJ_PROFILE_VERSION);
        }

        #[test]
        fn v1_planner_logs_and_prepares_bass_swap_16_when_renderable() {
            let db = db_with_pair();
            let transition = plan(&db);

            assert_eq!(transition.program.template, "BassSwap16");
            let row: (String, String, Option<String>) = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT template, program_json, fallback_reason
                         FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("event");
            let renderer_program: noor_mix::TransitionProgram =
                serde_json::from_str(&row.1).expect("program");

            assert_eq!(row.0, "BassSwap16");
            assert_eq!(renderer_program.template, "BassSwap16");
            assert_eq!(renderer_program.resolve_at, 1_152_000);
            assert_eq!(row.2, None);
        }

        #[test]
        fn v1_planner_keeps_drop_tease_overlay_out_of_end_transition() {
            let db = db_with_pair();
            make_pair_drop_tease_ready(&db);
            let transition = plan(&db);

            assert_eq!(transition.program.template, "BassSwap32");
            let row: (String, String, Option<String>) = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT template, program_json, fallback_reason
                         FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("event");
            let renderer_program: noor_mix::TransitionProgram =
                serde_json::from_str(&row.1).expect("program");

            assert_eq!(row.0, "BassSwap32");
            assert_eq!(renderer_program.template, "BassSwap32");
            assert_eq!(row.2, None);
        }

        #[test]
        fn v1_planner_logs_and_prepares_filter_sweep_when_renderable() {
            let db = db_with_pair();
            // An unsyncable 5% delta only stays a FilterSweep under bold
            // intent; balanced intent now degrades to SafeCrossfade.
            db.with_conn(|conn| {
                queries::set_dj_global_policy(conn, "bold", "neutral")?;
                queries::upsert_audio_dj_profile_correction(
                    conn,
                    &AudioDjProfileCorrectionRow {
                        media_ref_kind: "tidal_track".to_string(),
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
            .expect("seed correction");
            let transition = plan(&db);

            assert_eq!(transition.program.template, "FilterSweep");
            let row: (String, String, Option<String>) = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT template, program_json, fallback_reason
                         FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("event");
            let renderer_program: noor_mix::TransitionProgram =
                serde_json::from_str(&row.1).expect("program");

            assert_eq!(row.0, "FilterSweep");
            assert_eq!(renderer_program.template, "FilterSweep");
            assert_eq!(renderer_program.resolve_at, 864_000);
            assert_eq!(row.2, None);
        }

        #[test]
        fn v1_renderable_program_passes_filter_sweep_with_wider_duration() {
            let input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 32_000);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "FilterSweep");
            assert_eq!(program.resolve_at, 864_000);
            assert_eq!(reason, None);
            assert!(
                program
                    .automation
                    .iter()
                    .any(|event| event.param == noor_mix::Param::HighGain(noor_mix::DeckId::A))
            );
        }

        #[test]
        fn v1_renderable_program_passes_bass_swap_16_with_low_handoff() {
            let mut input = noor_mix::planner::bass_swap_16_program(48_000, 2, 32_000);
            input.deck_b_start_frame = 384_000;
            input.automation.push(noor_mix::AutomationEvent {
                param: noor_mix::Param::PlaybackRate(noor_mix::DeckId::B),
                start_sample: 0,
                end_sample: input.resolve_at,
                from: 0.985,
                to: 0.985,
                curve: noor_mix::Curve::Linear,
            });

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "BassSwap16");
            assert_eq!(program.resolve_at, 1_152_000);
            assert_eq!(program.deck_b_start_frame, 384_000);
            assert_eq!(reason, None);
            let rate = program
                .automation
                .iter()
                .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
                .expect("rate automation");
            assert_eq!(rate.from, 0.985);
            assert_eq!(rate.to, 0.985);
            assert!(program.automation.iter().any(|event| event.param
                == noor_mix::Param::LowGain(noor_mix::DeckId::B)
                && event.start_sample == program.swap_start
                && event.to == 1.0));
        }

        #[test]
        fn v1_renderable_program_passes_bass_swap_32_with_low_handoff() {
            let input = noor_mix::planner::bass_swap_32_program(48_000, 2, 16_000);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "BassSwap32");
            assert_eq!(program.resolve_at, 1_344_000);
            assert_eq!(reason, None);
            assert!(program.automation.iter().any(|event| event.param
                == noor_mix::Param::LowGain(noor_mix::DeckId::B)
                && event.start_sample == program.swap_start
                && event.to == 1.0));
        }

        #[test]
        fn v1_renderable_program_passes_slam_cut_as_short_gain_cut() {
            let mut input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 10_000);
            input.template = "SlamCut".to_string();

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "SlamCut");
            assert_eq!(program.resolve_at, 9_600);
            assert_eq!(reason, None);
            assert!(program.automation.iter().all(|event| !matches!(
                event.param,
                noor_mix::Param::LowGain(_)
                    | noor_mix::Param::MidGain(_)
                    | noor_mix::Param::HighGain(_)
            )));
        }

        #[test]
        fn v1_renderable_program_passes_long_harmonic_blend_rate() {
            let input = noor_mix::planner::long_harmonic_blend_program(48_000, 2, 32_000, 0.985);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "LongHarmonicBlend");
            assert_eq!(program.resolve_at, 1_152_000);
            assert_eq!(reason, None);
            let rate = program
                .automation
                .iter()
                .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
                .expect("rate automation")
                .to;
            assert_eq!(rate, 0.985);
        }

        #[test]
        fn v1_renderable_program_defaults_long_harmonic_blend_rate() {
            let mut input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 10_000);
            input.template = "LongHarmonicBlend".to_string();

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "LongHarmonicBlend");
            assert_eq!(reason, None);
            let rate = program
                .automation
                .iter()
                .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
                .expect("rate automation")
                .to;
            assert_eq!(rate, 1.0);
        }

        #[test]
        fn v1_renderable_program_preserves_drop_tease_as_overlay() {
            let mut input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 10_000);
            input.template = "DropTease16".to_string();
            input.deck_b_start_frame = 384_000;

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, false);

            assert_eq!(program.template, "DropTease16");
            assert_eq!(program.deck_b_start_frame, 384_000);
            assert_eq!(reason, None);
            assert!(program.automation.iter().any(|event| event.param
                == noor_mix::Param::DeckGain(noor_mix::DeckId::A)
                && event.from == 0.0
                && event.to == 0.0));
        }

        #[test]
        fn unstable_timing_downgrades_filter_sweep_to_safe_crossfade() {
            let input = noor_mix::planner::filter_sweep_eq_wash_program(48_000, 2, 10_000);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

            assert_eq!(program.template, "SafeCrossfade");
            assert_eq!(reason, Some("timing_unstable"));
        }

        #[test]
        fn unstable_timing_downgrades_bass_swap_16_to_safe_crossfade() {
            let input = noor_mix::planner::bass_swap_16_program(48_000, 2, 16_000);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

            assert_eq!(program.template, "SafeCrossfade");
            assert_eq!(reason, Some("timing_unstable"));
        }

        #[test]
        fn unstable_timing_downgrades_bass_swap_32_to_safe_crossfade() {
            let input = noor_mix::planner::bass_swap_32_program(48_000, 2, 32_000);

            let (program, reason) = v1_renderable_program(&input, 48_000, 2, true);

            assert_eq!(program.template, "SafeCrossfade");
            assert_eq!(reason, Some("timing_unstable"));
        }

        #[test]
        fn dj_renderers_use_wider_overlap_than_safe_crossfade() {
            let safe = crate::playback::dj_engine::safe_crossfade_program(
                48_000,
                2,
                noor_mix::Policy::default(),
            );
            let filter = noor_mix::planner::filter_sweep_eq_wash_program(
                48_000,
                2,
                DJ_FILTER_SWEEP_RENDER_MS,
            );
            let bass_swap =
                noor_mix::planner::bass_swap_16_program(48_000, 2, DJ_BASS_SWAP_16_RENDER_MS);
            let bass_swap_32 =
                noor_mix::planner::bass_swap_32_program(48_000, 2, DJ_BASS_SWAP_32_RENDER_MS);

            // SafeCrossfade dropped from 12s to 6s: two full-spectrum tracks
            // fighting for 12 seconds read as mud next to the beat-matched
            // FullBlend renders, which keep their longer windows.
            assert_eq!(dj_gapless_plan_from_program(&safe).overlap_ms, 6_000);
            assert_eq!(dj_gapless_plan_from_program(&filter).overlap_ms, 18_000);
            assert_eq!(dj_gapless_plan_from_program(&bass_swap).overlap_ms, 24_000);
            assert_eq!(
                dj_gapless_plan_from_program(&bass_swap_32).overlap_ms,
                28_000
            );
        }

        #[test]
        fn fire_ahead_requires_latest_twenty_positive_evidence() {
            let passing = vec![
                412, 709, 270, 475, 8, 827, 258, 738, 8, 35, 252, 141, -375, 210, 73, 529, -53, 48,
                481, 300,
            ];
            let mixed = vec![
                412, 709, 270, 475, -8, -827, -258, -738, -8, -35, 252, 141, -375, 210, 73, 529,
                -53, 48, 481, 300,
            ];
            let low_median = vec![
                151, 150, 149, 148, 147, 146, 145, 144, 143, 142, 141, 140, 139, 138, 137, 136,
                135, 134, -20, -40,
            ];

            assert_eq!(fire_ahead_ms_from_deltas(&passing), 127);
            assert_eq!(fire_ahead_ms_from_deltas(&mixed), 0);
            assert_eq!(fire_ahead_ms_from_deltas(&low_median), 0);
            assert_eq!(fire_ahead_ms_from_deltas(&passing[..19]), 0);
        }

        #[test]
        fn renderer_timing_gate_uses_abs_error_not_signed_bias() {
            assert!(render_timing_unstable_from_deltas(&[549, -399, 303, -375]));
            assert!(!render_timing_unstable_from_deltas(&[140, -130, 75, -90]));
            assert!(!render_timing_unstable_from_deltas(&[549, -399, 303]));
        }

        #[test]
        fn timing_calibration_ignores_decode_fallback_rows() {
            let db = db_with_pair();
            db.with_conn(|conn| {
                conn.execute("DELETE FROM dj_transition_events", [])?;
                for delta_ms in [549, -399, 303, -375] {
                    insert_timing_sample(
                        conn,
                        delta_ms,
                        false,
                        "legacy_overlap",
                        "active_deck_not_decoded",
                    )?;
                }
                Ok(())
            })
            .expect("seed fallback timing");

            let unstable = db.with_conn(render_timing_unstable).expect("gate");

            assert!(!unstable);
        }

        #[test]
        fn timing_calibration_uses_successful_dj_mixer_rows() {
            let db = db_with_pair();
            db.with_conn(|conn| {
                conn.execute("DELETE FROM dj_transition_events", [])?;
                for delta_ms in [549, -399, 303, -375] {
                    insert_timing_sample(conn, delta_ms, true, "rendered_handoff", "none")?;
                }
                Ok(())
            })
            .expect("seed rendered timing");

            let unstable = db.with_conn(render_timing_unstable).expect("gate");

            assert!(unstable);
        }

        #[test]
        fn fire_ahead_ignores_fallback_timing_rows() {
            let db = db_with_pair();
            db.with_conn(|conn| {
                conn.execute("DELETE FROM dj_transition_events", [])?;
                for _ in 0..20 {
                    insert_timing_sample(
                        conn,
                        500,
                        false,
                        "legacy_overlap",
                        "next_deck_not_decoded",
                    )?;
                }
                Ok(())
            })
            .expect("seed fallback timing");

            let fire_ahead_ms = db
                .with_conn(dj_transition_fire_ahead_ms)
                .expect("fire ahead");

            assert_eq!(fire_ahead_ms, 0);
        }

        #[test]
        fn fire_ahead_caps_large_median() {
            assert_eq!(fire_ahead_ms_from_deltas(&[500; 20]), 150);
        }

        #[test]
        fn external_dj_transition_logging_does_not_require_library_track_id() {
            let db = db_with_pair();
            db.with_conn(|conn| {
                conn.execute("DELETE FROM queue WHERE id = 12", [])?;
                conn.execute(
                    "INSERT INTO queue (id, track_id, position, source, pending_artist, pending_title)
                     VALUES (12, NULL, 1, 'radio_pending', 'External A', 'External B')",
                    [],
                )?;
                seed_profile(conn, "queue_item", "12", None)?;
                Ok(())
            })
            .expect("external");
            let pair = db.with_conn(load_dj_lookahead_pair).expect("pair");
            let engine = DjEngine::new(db.clone());
            let job = attach_dj_transition_plan_for_pair(
                &engine,
                PreparedPlaybackJob::test_fixture(99, 1),
                pair,
                48_000,
                2,
            )
            .expect("plan");

            let event_id = job
                .prepared_transition
                .and_then(|transition| transition.transition_event_id)
                .expect("event");
            let row = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT to_track_id, to_media_ref_kind, to_media_ref_id
                         FROM dj_transition_events WHERE id = ?1",
                        params![event_id],
                        |row| {
                            Ok((
                                row.get::<_, Option<i64>>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .map_err(Into::into)
                })
                .expect("event");

            assert_eq!(row.0, None);
            assert_eq!(row.1, "queue_item");
            assert_eq!(row.2, "12");
        }

        #[test]
        fn skip_mid_transition_updates_dj_event() {
            let db = db_with_pair();
            let transition = plan(&db);

            db.with_conn(|conn| {
                record_dj_transition_listen_outcome(
                    conn,
                    transition.transition_event_id,
                    12_000,
                    false,
                )
            })
            .expect("outcome");

            let outcome: String = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT outcome FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
                })
                .expect("outcome");
            assert_eq!(outcome, "skip_within_30s");
        }

        #[test]
        fn skip_within_30s_counts_as_negative_transition_outcome() {
            let db = db_with_pair();
            let transition = plan(&db);

            db.with_conn(|conn| {
                record_dj_transition_listen_outcome(
                    conn,
                    transition.transition_event_id,
                    29_999,
                    false,
                )
            })
            .expect("outcome");

            let skip_flag: i64 = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT skip_within_30s FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
                })
                .expect("flag");
            assert_eq!(skip_flag, 1);
        }

        #[test]
        fn skip_after_30s_does_not_count_as_bad_transition_feedback() {
            let db = db_with_pair();
            let transition = plan(&db);

            db.with_conn(|conn| {
                record_dj_transition_listen_outcome(
                    conn,
                    transition.transition_event_id,
                    30_000,
                    false,
                )
            })
            .expect("outcome");

            let row = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT outcome, skip_within_30s FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("row");
            assert_eq!(row, (None, 0));
        }

        #[test]
        fn manual_bad_feedback_counts_stronger_than_skip() {
            let db = db_with_pair();
            let transition = plan(&db);

            db.with_conn(|conn| {
                record_dj_transition_listen_outcome(
                    conn,
                    transition.transition_event_id,
                    12_000,
                    false,
                )?;
                conn.execute(
                    "UPDATE dj_transition_events SET user_rating = -1 WHERE id = ?1",
                    params![transition.transition_event_id],
                )?;
                queries::count_recent_bad_dj_feedback_for_ref(
                    conn,
                    &AudioDjProfileKey {
                        media_ref_kind: "tidal_track".to_string(),
                        media_ref_id: "2".to_string(),
                    },
                    3,
                )
            })
            .map(|count| assert_eq!(count, 1))
            .expect("feedback count");
        }

        #[test]
        fn finished_transition_updates_dj_event() {
            let db = db_with_pair();
            let transition = plan(&db);

            db.with_conn(|conn| {
                record_dj_transition_listen_outcome(
                    conn,
                    transition.transition_event_id,
                    170_000,
                    true,
                )
            })
            .expect("outcome");

            let row = db
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT outcome, skip_within_30s FROM dj_transition_events WHERE id = ?1",
                        params![transition.transition_event_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .map_err(Into::into)
                })
                .expect("row");
            assert_eq!(row, ("finished".to_string(), 0));
        }
    }

    fn track_with_tidal_id(id: i64, tidal_id: Option<i64>, quality: Option<&str>) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist_id: 1,
            artist_name: Some("A".to_string()),
            album_id: None,
            album_title: None,
            disc_number: Some(1),
            track_number: Some(id as i32),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id,
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: quality.map(|s| s.to_string()),
            best_source: tidal_id.map(|_| "tidal".to_string()),
            fidelity_score: 10,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: Some("2025-01-01".to_string()),
            source: if tidal_id.is_some() {
                "tidal".to_string()
            } else {
                "local".to_string()
            },
            artwork_url: None,
        }
    }

    fn blank_dsp_features() -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 0,
            bpm: None,
            key_signature: None,
            camelot_key: None,
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

    #[test]
    fn automix_reason_does_not_claim_harmonic_match_without_key_or_bpm() {
        let profile = SessionTasteProfile {
            current_source: Some("tidal".to_string()),
            ..SessionTasteProfile::default()
        };
        let (taste, seed) = from_session_profile(&profile);
        let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
        let seed_features = blank_dsp_features();
        let candidate_features = blank_dsp_features();

        let score = automix_score(
            &track,
            &[],
            &taste,
            &seed,
            Some(&seed_features),
            Some(&candidate_features),
        );
        let reason = automix_scored_reason(&score);

        assert!(reason.contains("same source"), "got: {reason}");
        assert!(
            !reason.contains("harmonic")
                && !reason.contains("key clash")
                && !reason.contains("adjacent key"),
            "missing key and BPM must produce no harmonic signal at all: {reason}"
        );
    }

    #[test]
    fn automix_reason_marks_recent_skip_as_penalty_not_cause() {
        // The candidate's artist carries negative session affinity, so
        // automix_score penalizes it. The reason must render that as a
        // "despite" clause, never as a cause the track was picked.
        let profile = SessionTasteProfile {
            current_source: Some("tidal".to_string()),
            negative_artists: HashMap::from([(1, 1.0)]),
            ..SessionTasteProfile::default()
        };
        let (taste, seed) = from_session_profile(&profile);
        let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));

        let score = automix_score(&track, &[], &taste, &seed, None, None);
        let reason = automix_scored_reason(&score);

        assert!(
            reason.contains("despite") && reason.contains("recent skip penalty"),
            "a penalizing signal must be rendered as a penalty: {reason}"
        );
        let lead = reason.split(" despite ").next().unwrap_or("");
        assert!(
            !lead.contains("recent skip penalty"),
            "a penalty must never appear in the selection-cause lead: {reason}"
        );
    }

    #[test]
    fn automix_reason_marks_energy_whiplash_as_penalty() {
        // A large energy jump multiplies the score *down*; the old reason
        // builder mislabeled it "energy contrast" as if it were a cause.
        let profile = SessionTasteProfile {
            current_source: Some("tidal".to_string()),
            ..SessionTasteProfile::default()
        };
        let (taste, seed) = from_session_profile(&profile);
        let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
        let seed_features = AudioDspFeatures {
            energy: Some(0.2),
            ..blank_dsp_features()
        };
        let candidate_features = AudioDspFeatures {
            energy: Some(0.9),
            ..blank_dsp_features()
        };

        let score = automix_score(
            &track,
            &[],
            &taste,
            &seed,
            Some(&seed_features),
            Some(&candidate_features),
        );
        let reason = automix_scored_reason(&score);

        assert!(
            reason.contains("despite") && reason.contains("energy whiplash"),
            "a large energy jump is a penalty, not a selection cause: {reason}"
        );
    }

    #[test]
    fn automix_reason_does_not_claim_harmonic_match_on_key_clash_with_close_bpm() {
        // 8A vs 10A is a Camelot clash, but a near-identical BPM pushes the
        // *combined* harmonic multiplier above 1.0. The reason must still call
        // it a key clash - deriving the signal from the Camelot relationship,
        // not the blended multiplier.
        let profile = SessionTasteProfile {
            current_source: Some("tidal".to_string()),
            ..SessionTasteProfile::default()
        };
        let (taste, seed) = from_session_profile(&profile);
        let track = track_with_tidal_id(42, Some(42), Some("LOSSLESS"));
        let seed_features = AudioDspFeatures {
            camelot_key: Some("8A".to_string()),
            bpm: Some(120.0),
            ..blank_dsp_features()
        };
        let candidate_features = AudioDspFeatures {
            camelot_key: Some("10A".to_string()),
            bpm: Some(122.0),
            ..blank_dsp_features()
        };

        let score = automix_score(
            &track,
            &[],
            &taste,
            &seed,
            Some(&seed_features),
            Some(&candidate_features),
        );
        let reason = automix_scored_reason(&score);

        assert!(
            reason.contains("key clash"),
            "a Camelot clash must be labeled a key clash: {reason}"
        );
        assert!(
            !reason.contains("harmonic match") && !reason.contains("adjacent key"),
            "a key clash must never be shown as a harmonic match: {reason}"
        );
    }

    #[test]
    fn previous_track_moves_back_when_under_threshold() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 2, position_ms = 0, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = previous_track(&conn, 2_500, None).unwrap();

        assert!(!outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(outcome.snapshot.state.position_ms, 0);
        assert!(outcome.snapshot.state.is_playing);
    }

    #[test]
    fn previous_track_restarts_current_track_when_over_threshold() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        // DB position_ms stays 0 during playback (nothing persists the live
        // playhead); the threshold must key on the caller-provided live
        // position, never this column.
        conn.execute(
            "UPDATE playback_state SET current_track_id = 2, position_ms = 0, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = previous_track(&conn, PREVIOUS_RESTART_THRESHOLD_MS, None).unwrap();

        assert!(outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 2);
        assert_eq!(outcome.snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_restarts_first_track_when_no_previous_exists() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, position_ms = 0, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = previous_track(&conn, 1_000, None).unwrap();

        assert!(outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(outcome.snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_prefers_valid_history_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        // Simulate a shuffled/jumped session: current is the FIRST row, but
        // history says the third row actually played before it. Queue-order
        // stepping would restart-in-place; history must win.
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let anchor = HistoryAnchor {
            queue_item_id: queue_items[2].id,
            track_id: Some(queue_items[2].track.id),
        };
        let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

        assert!(!outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 3);
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[2].id)
        );
    }

    #[test]
    fn previous_track_falls_back_when_history_anchor_row_is_gone() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![queue_items[1].id],
        )
        .unwrap();

        let anchor = HistoryAnchor {
            queue_item_id: 999_999,
            track_id: Some(3),
        };
        let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

        // Stale anchor: fall back to queue-order stepping (row above).
        assert!(!outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    }

    #[test]
    fn previous_track_rejects_history_anchor_with_changed_track() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![queue_items[1].id],
        )
        .unwrap();

        // Anchor row exists but now holds a different track (re-resolved /
        // edited): must not navigate onto the wrong track.
        let anchor = HistoryAnchor {
            queue_item_id: queue_items[2].id,
            track_id: Some(999),
        };
        let outcome = previous_track(&conn, 500, Some(&anchor)).unwrap();

        assert!(!outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
    }

    #[test]
    fn previous_track_ignores_mismatched_current_queue_item_id() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let outcome = previous_track(&conn, 1_000, None).unwrap();

        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[0].id)
        );
        assert_eq!(outcome.snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_accepts_pending_current_queue_item_id() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title)
             VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title')",
            [],
        )
        .unwrap();
        let pending_qid = conn.last_insert_rowid();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = ?1, position_ms = 0, is_playing = 1
             WHERE id = 1",
            params![pending_qid],
        )
        .unwrap();

        let outcome = previous_track(&conn, 1_000, None).unwrap();

        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[0].id)
        );
        assert_eq!(outcome.snapshot.state.position_ms, 0);
    }

    #[test]
    fn previous_track_selects_first_queue_item_when_nothing_is_playing() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        let outcome = previous_track(&conn, 0, None).unwrap();

        assert!(!outcome.restart_in_place);
        assert_eq!(outcome.snapshot.state.current_track.unwrap().id, 1);
        assert_eq!(outcome.snapshot.state.position_ms, 0);
        assert!(outcome.snapshot.state.is_playing);
    }

    #[test]
    fn previous_track_clears_state_when_queue_is_empty() {
        let conn = conn();
        conn.execute(
            "UPDATE playback_state SET current_track_id = 1, position_ms = 1500, is_playing = 1 WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = previous_track(&conn, 5_000, None).unwrap();

        assert!(!outcome.restart_in_place);
        assert!(outcome.snapshot.state.current_track.is_none());
        assert_eq!(outcome.snapshot.state.position_ms, 0);
        assert!(!outcome.snapshot.state.is_playing);
        assert!(outcome.snapshot.queue.is_empty());
    }

    #[test]
    fn build_tidal_stream_request_uses_track_quality_or_defaults() {
        let track = track_with_tidal_id(42, Some(88), Some("HI_RES_LOSSLESS"));

        let request = build_tidal_stream_request(&track, None).unwrap();

        assert_eq!(request.track_id, 88);
        assert_eq!(request.audio_quality, "HI_RES_LOSSLESS");
        assert_eq!(request.playback_mode, "STREAM");
        assert_eq!(request.asset_presentation, "FULL");
    }

    #[test]
    fn build_playback_preparation_marks_local_tracks_as_local_library() {
        let track = track_with_tidal_id(7, None, None);

        let prep = build_playback_preparation(&track, None, 1500, None);

        assert!(prep.is_local());
        assert_eq!(prep.source_kind(), PlaybackSourceKind::LocalLibrary);
        assert!(prep.stream_request().is_none());
        assert!(prep.dj_media_ref.is_none());
        assert!(!prep.gapless.enabled);
        assert_eq!(prep.track.id, 7);
    }

    #[test]
    fn build_playback_preparation_includes_tidal_stream_request() {
        let track = track_with_tidal_id(7, Some(77), Some("LOSSLESS"));
        let stream = StreamInfo {
            url: "https://example.com/stream.flac".to_string(),
            segment_urls: vec![],
            segment_offsets_ms: vec![],
            track_id: 77,
            audio_quality: "LOSSLESS".to_string(),
            codec: "audio/flac".to_string(),
            sample_rate: Some(44_100),
            bit_depth: Some(16),
        };

        let prep = build_playback_preparation(&track, Some(&stream), 1500, None);

        assert!(prep.is_tidal());
        assert_eq!(prep.source_kind(), PlaybackSourceKind::TidalStream);
        let request = prep.stream_request().expect("expected a tidal request");
        assert_eq!(request.track_id, 77);
        assert_eq!(request.audio_quality, "LOSSLESS");
        assert!(prep.gapless.enabled);
        assert_eq!(prep.gapless.overlap_ms, 1500);
        assert_eq!(prep.output_sample_rate, Some(44_100));
        assert_eq!(
            prep.dj_media_ref
                .as_ref()
                .map(|media_ref| media_ref.profile_key()),
            Some(crate::db::models::AudioDjProfileKey {
                media_ref_kind: "tidal_track".to_string(),
                media_ref_id: "77".to_string(),
            })
        );
    }

    #[test]
    fn synced_overlap_uses_projected_downbeat_before_track_end() {
        let overlap_ms =
            synced_overlap_from_grid_ms(180_000, &[0.0, 2.0, 4.0], 8_000, Some(8_000 * 48), 48_000)
                .expect("overlap");

        assert_eq!(overlap_ms, 8_000);
    }

    #[test]
    fn synced_overlap_keeps_preferred_minimum_when_last_grid_is_too_close() {
        let overlap_ms =
            synced_overlap_from_grid_ms(181_000, &[0.0, 2.0, 4.0], 8_000, Some(8_000 * 48), 48_000)
                .expect("overlap");

        assert_eq!(overlap_ms, 9_000);
    }

    #[test]
    fn synced_overlap_allows_longer_dj_handoff_windows() {
        let overlap_ms = synced_overlap_from_grid_ms(
            180_000,
            &[0.0, 2.0, 4.0],
            24_000,
            Some(24_000 * 48),
            48_000,
        )
        .expect("overlap");

        assert_eq!(overlap_ms, 24_000);
    }

    #[test]
    fn synced_overlap_extrapolation_ignores_near_duplicate_grid_noise() {
        // A 10 ms near-duplicate pair must not become the extrapolation
        // interval; the median keeps the projected grid on the real 2 s bar
        // spacing. With a min-interval flood every 10 ms this would return
        // exactly 8_000 from a fake marker instead of 9_000 from a real one.
        let overlap_ms = synced_overlap_from_grid_ms(
            181_000,
            &[0.0, 2.0, 2.01, 4.0, 6.0, 8.0],
            8_000,
            Some(8_000 * 48),
            48_000,
        )
        .expect("overlap");

        assert_eq!(overlap_ms, 9_000);
    }

    #[test]
    fn armed_event_anchor_requires_grid_timing_source() {
        let event = |timing_source: Option<&str>| ArmedDjTransitionEvent {
            id: 1,
            program: noor_mix::TransitionProgram {
                tier: noor_mix::program::Tier::SafeCrossfade,
                template: "SafeCrossfade".to_string(),
                drop_source: None,
                sample_rate: 48_000,
                channels: 2,
                deck_a_start_frame: 0,
                deck_b_start_frame: 0,
                sync_start: 0,
                intro_start: 0,
                swap_start: 1,
                fade_start: 1,
                resolve_at: 2,
                loops: vec![],
                automation: vec![],
            },
            fallback_reason: None,
            planned_start_ms: Some(200_000),
            timing_source: timing_source.map(str::to_string),
        };

        // Grid-derived plans fire against the planned start directly.
        assert_eq!(
            event(Some("downbeat_sync")).anchor_start_ms(),
            Some(200_000)
        );
        assert_eq!(event(Some("beat_sync")).anchor_start_ms(), Some(200_000));
        // A fallback overlap's planned start is metadata arithmetic; firing
        // against it would reintroduce the duration-mismatch error.
        assert_eq!(event(Some("fallback_overlap")).anchor_start_ms(), None);
        assert_eq!(event(None).anchor_start_ms(), None);
    }

    #[test]
    fn completed_listen_uses_ninety_percent_or_four_minute_cap() {
        let short_track = track_with_tidal_id(1, Some(42), Some("LOSSLESS"));
        assert!(is_completed_listen(&short_track, 162_000));
        assert!(!is_completed_listen(&short_track, 161_999));

        let long_track = Track {
            duration_ms: Some(600_000),
            ..track_with_tidal_id(2, Some(99), Some("LOSSLESS"))
        };
        assert!(is_completed_listen(&long_track, 240_000));
        assert!(!is_completed_listen(&long_track, 239_999));
    }

    #[test]
    fn next_track_extends_queue_when_automix_is_enabled() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.unwrap().id, 3);
        assert!(snapshot.queue.len() > 2);
        assert!(snapshot.queue.iter().any(|item| item.source == "automix"));
    }

    #[test]
    fn ensure_automix_queue_depth_records_reasons_for_generated_rows() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        let queue = ensure_automix_queue_depth(&conn, AUTOMIX_MIN_UPCOMING, false).unwrap();
        let generated = queue
            .iter()
            .filter(|item| item.source == "automix")
            .collect::<Vec<_>>();

        assert!(!generated.is_empty(), "expected generated automix rows");
        assert!(
            generated.iter().all(|item| item
                .reason
                .as_deref()
                .is_some_and(|reason| !reason.trim().is_empty())),
            "generated automix rows should persist selection reasons: {generated:?}"
        );
    }

    #[test]
    fn peek_next_track_can_see_generated_automix_track() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        let next = peek_next_track(&conn, false)
            .unwrap()
            .expect("generated automix track");

        assert_eq!(next.id, 3);
        let queue_items = queue::load_queue(&conn).unwrap();
        assert!(queue_items.len() > 2);
    }

    #[test]
    fn peek_next_track_uses_current_queue_item_id_for_duplicate_tracks() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 1, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        let second_track_one = queue_items
            .iter()
            .find(|item| item.position == 2 && item.track.id == 1)
            .expect("second copy of track 1");
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![second_track_one.id],
        )
        .unwrap();

        let next = peek_next_track(&conn, false).unwrap().expect("next track");

        assert_eq!(next.id, 3);
    }

    #[test]
    fn peek_next_track_ignores_mismatched_current_queue_item_id() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let next = peek_next_track(&conn, false).unwrap().expect("next track");

        assert_eq!(next.id, 3);
    }

    #[test]
    fn peek_next_track_returns_first_queue_item_when_unanchored() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let next = peek_next_track(&conn, false).unwrap().expect("first track");

        assert_eq!(next.id, 1);
    }

    #[test]
    fn ensure_automix_queue_depth_suppresses_refill_when_recently_cleared() {
        // Same setup as `next_track_extends_queue_when_automix_is_enabled`:
        // two tracks queued, automix on, current = 2. The non-suppressed
        // refill path is already covered by that sibling test; here we
        // verify the new gate alone - that with `recently_cleared = true`
        // the helper short-circuits before any extension work and returns
        // the existing queue unmodified.
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, position_ms = 0, is_playing = 1, automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            [],
        )
        .unwrap();

        // Suppression on - must return the 2 existing items, never call the
        // extension path (which would otherwise touch tables not present in
        // this minimal test schema and panic).
        let suppressed = ensure_automix_queue_depth(&conn, AUTOMIX_MIN_UPCOMING, true).unwrap();
        assert_eq!(
            suppressed.len(),
            2,
            "suppressed call must not extend the queue"
        );
        assert!(
            !suppressed.iter().any(|item| item.source == "automix"),
            "suppressed call must not append automix rows"
        );
        let stored = queue::load_queue(&conn).unwrap();
        assert_eq!(
            stored.len(),
            2,
            "DB queue must be untouched while suppressed"
        );
    }

    #[test]
    fn ensure_automix_external_enabled_appends_pending_sidecar_rows() {
        let conn = conn();
        let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
        queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, position_ms = 0, is_playing = 1,
                 automix_enabled = 1, automix_allow_external = 1, automix_use_learning = 0",
            [],
        )
        .unwrap();
        let model = queries::create_embedding_model(
            &conn,
            "discovery-fusion-v2:test-external",
            "discovery-fusion-v2",
            32,
            "ready",
            None,
        )
        .unwrap();
        queries::activate_embedding_model(&conn, model.id).unwrap();
        let candidate = queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(99001),
                mbid: None,
                dedupe_key: "tidal:99001".to_string(),
                title: "Outside Track".to_string(),
                artist_name: "Outside Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .unwrap();
        queries::replace_external_candidate_neighbors(
            &conn,
            model.id,
            1,
            &[queries::ExternalCandidateNeighborWriteRow {
                candidate_id: candidate.id,
                rank: 1,
                score: 0.9,
                audio_score: 0.9,
                metadata_score: 0.0,
                reason_json: None,
            }],
        )
        .unwrap();

        let queue = ensure_automix_queue_depth(&conn, 1, false).unwrap();

        let pending = queue
            .iter()
            .find(|item| item.source == "automix-new")
            .expect("pending external automix row");
        assert!(pending.is_pending);
        assert_eq!(pending.track.title, "Outside Track");
        let tidal_hint: Option<i64> = conn
            .query_row(
                "SELECT tidal_id_hint FROM queue WHERE id = ?1",
                params![pending.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tidal_hint, Some(99001));
    }

    #[test]
    fn ensure_automix_external_overfetches_past_already_queued_candidates() {
        let conn = conn();
        let current = queue::get_tracks_by_ids(&conn, &[1]).unwrap().remove(0);
        queue::append_tracks(&conn, std::slice::from_ref(&current), "user").unwrap();
        queue::append_external_track(
            &conn,
            &queue::ExternalTrackInsert {
                artist: "Outside Artist",
                title: "Already Queued",
                source: "automix-new",
                reason: Some("external similarity"),
                tidal_id_hint: Some(99001),
                local_track_id: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, position_ms = 0, is_playing = 1,
                 automix_enabled = 1, automix_allow_external = 1, automix_use_learning = 0",
            [],
        )
        .unwrap();
        let model = queries::create_embedding_model(
            &conn,
            "discovery-fusion-v2:test-external-overfetch",
            "discovery-fusion-v2",
            32,
            "ready",
            None,
        )
        .unwrap();
        queries::activate_embedding_model(&conn, model.id).unwrap();
        let first = queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(99001),
                mbid: None,
                dedupe_key: "tidal:99001".to_string(),
                title: "Already Queued".to_string(),
                artist_name: "Outside Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(180_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .unwrap();
        let second = queries::upsert_external_track_candidate(
            &conn,
            &queries::ExternalTrackCandidateUpsert {
                tidal_id: Some(99002),
                mbid: None,
                dedupe_key: "tidal:99002".to_string(),
                title: "Fresh External".to_string(),
                artist_name: "Outside Artist".to_string(),
                genre_tags_json: None,
                duration_ms: Some(181_000),
                expires_at: "2026-03-01 00:00:00".to_string(),
            },
        )
        .unwrap();
        queries::replace_external_candidate_neighbors(
            &conn,
            model.id,
            1,
            &[
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: first.id,
                    rank: 1,
                    score: 0.95,
                    audio_score: 0.95,
                    metadata_score: 0.0,
                    reason_json: None,
                },
                queries::ExternalCandidateNeighborWriteRow {
                    candidate_id: second.id,
                    rank: 2,
                    score: 0.9,
                    audio_score: 0.9,
                    metadata_score: 0.0,
                    reason_json: None,
                },
            ],
        )
        .unwrap();

        ensure_automix_queue_depth(&conn, 2, false).unwrap();

        let hints = conn
            .prepare(
                "SELECT tidal_id_hint
                 FROM queue
                 WHERE source = 'automix-new'
                 ORDER BY position",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, Option<i64>>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(hints.iter().filter(|hint| **hint == Some(99001)).count(), 1);
        assert!(hints.contains(&Some(99002)));
    }

    #[test]
    fn ensure_automix_queue_depth_anchors_to_duplicate_current_queue_item() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 1]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        let second_track_one = queue_items
            .iter()
            .find(|item| item.position == 2 && item.track.id == 1)
            .expect("second copy of track 1");
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1,
                 automix_enabled = 1, shuffle_mode = 'off'
             WHERE id = 1",
            params![second_track_one.id],
        )
        .unwrap();

        let queue = ensure_automix_queue_depth(&conn, 1, false).unwrap();

        assert!(
            queue.len() > queue_items.len(),
            "automix should refill from the active duplicate row"
        );
        assert!(queue.iter().any(|item| item.source == "automix"));
    }

    #[test]
    fn play_track_now_sets_current_queue_item_id() {
        let conn = conn();
        // Seed two queue rows pointing at the same track so the "lowest
        // position" tiebreak is testable.
        let tracks = load_tracks(&conn, &[1, 1, 2]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();

        play_track_now(&conn, 1).unwrap();
        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
        // Of the two rows pointing at track 1, the lowest-position one wins.
        let expected_qid = q.iter().find(|i| i.track.id == 1).unwrap().id;
        assert_eq!(state.current_queue_item_id, Some(expected_qid));
        assert!(state.is_playing);
    }

    #[test]
    fn remove_current_queue_item_advances_to_next_survivor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[1].id],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[1].id).unwrap();

        assert!(outcome.removed_current);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(3)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[2].id)
        );
        assert!(outcome.snapshot.state.is_playing);
        assert_eq!(outcome.snapshot.queue.len(), 2);
    }

    #[test]
    fn remove_current_queue_item_repairs_stale_anchor_before_reconcile() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = 999999, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(outcome.removed_current);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(2)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[1].id)
        );
        assert!(outcome.snapshot.state.is_playing);
    }

    #[test]
    fn remove_current_queue_item_repairs_mismatched_anchor_before_reconcile() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[1].id],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(outcome.removed_current);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(2)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[1].id)
        );
        assert!(outcome.snapshot.state.is_playing);
    }

    #[test]
    fn remove_current_queue_item_repairs_missing_anchor_before_reconcile() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = NULL, is_playing = 0
             WHERE id = 1",
            [],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(outcome.removed_current);
        assert!(!outcome.was_playing);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(2)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[1].id)
        );
        assert!(!outcome.snapshot.state.is_playing);
    }

    #[test]
    fn remove_current_queue_item_stops_when_no_survivor_exists() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(outcome.removed_current);
        assert!(outcome.snapshot.state.current_track.is_none());
        assert_eq!(outcome.snapshot.state.current_queue_item_id, None);
        assert!(!outcome.snapshot.state.is_playing);
        assert!(outcome.snapshot.queue.is_empty());
    }

    #[test]
    fn remove_paused_current_queue_item_preserves_paused_state() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 0
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(outcome.removed_current);
        assert!(!outcome.was_playing);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(2)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[1].id)
        );
        assert!(!outcome.snapshot.state.is_playing);
    }

    #[test]
    fn remove_previous_queue_item_preserves_current_queue_item_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 3, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[2].id],
        )
        .unwrap();

        let outcome = remove_queue_item_and_reconcile(&conn, queue_items[0].id).unwrap();

        assert!(!outcome.removed_current);
        assert_eq!(
            outcome
                .snapshot
                .state
                .current_track
                .as_ref()
                .map(|track| track.id),
            Some(3)
        );
        assert_eq!(
            outcome.snapshot.state.current_queue_item_id,
            Some(queue_items[2].id)
        );
        assert_eq!(outcome.snapshot.queue.len(), 2);
        assert_eq!(outcome.snapshot.queue[0].position, 0);
        assert_eq!(outcome.snapshot.queue[1].position, 1);
    }

    #[test]
    fn setting_shuffle_off_clears_shuffle_seed() {
        let conn = conn();
        conn.execute(
            "UPDATE playback_state SET shuffle_mode = 'genre', shuffle_seed = 12345 WHERE id = 1",
            [],
        )
        .unwrap();

        let update = set_shuffle_mode(&conn, ShuffleMode::Off).unwrap();
        let stored_seed: Option<i64> = conn
            .query_row(
                "SELECT shuffle_seed FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(update.snapshot.state.shuffle_mode, "off");
        assert!(update.debug.is_none());
        assert_eq!(stored_seed, None);
    }

    #[test]
    fn setting_shuffle_repairs_stale_current_queue_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3, 4]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        let current_qid = queue_items[1].id;
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2,
                 current_queue_item_id = 999999,
                 is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
        let debug = update.debug.expect("shuffle debug");
        let stored_current_qid: Option<i64> = conn
            .query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(stored_current_qid, Some(current_qid));
        assert_eq!(
            update.snapshot.state.current_queue_item_id,
            Some(current_qid)
        );
        assert_eq!(debug.locked_count, 2);
        assert_eq!(debug.candidate_count, 2);
        assert_eq!(
            update
                .snapshot
                .queue
                .iter()
                .position(|item| item.id == current_qid),
            Some(1)
        );
    }

    #[test]
    fn setting_shuffle_repairs_mismatched_current_queue_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3, 4]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        let mismatched_qid = queue_items[0].id;
        let current_qid = queue_items[1].id;
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2,
                 current_queue_item_id = ?1,
                 is_playing = 1
             WHERE id = 1",
            params![mismatched_qid],
        )
        .unwrap();

        let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
        let debug = update.debug.expect("shuffle debug");
        let stored_current_qid: Option<i64> = conn
            .query_row(
                "SELECT current_queue_item_id FROM playback_state WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(stored_current_qid, Some(current_qid));
        assert_eq!(
            update.snapshot.state.current_queue_item_id,
            Some(current_qid)
        );
        assert_eq!(debug.locked_count, 2);
        assert_eq!(
            update
                .snapshot
                .queue
                .iter()
                .position(|item| item.id == current_qid),
            Some(1)
        );
    }

    #[test]
    fn setting_shuffle_repairs_missing_anchor_for_duplicate_current_track() {
        let conn = conn();
        let repeated = queue::get_track_by_id(&conn, 1).unwrap().unwrap();
        let other_tracks = load_tracks(&conn, &[2, 3]);
        let queue_items = queue::replace_queue(
            &conn,
            &[
                repeated.clone(),
                repeated,
                other_tracks[0].clone(),
                other_tracks[1].clone(),
            ],
            "test",
        )
        .unwrap();
        let repaired_qid = queue_items[0].id;
        let duplicate_qid = queue_items[1].id;
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1,
                 current_queue_item_id = NULL,
                 is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let update = set_shuffle_mode(&conn, ShuffleMode::True).unwrap();
        let debug = update.debug.expect("shuffle debug");

        assert_eq!(
            update.snapshot.state.current_queue_item_id,
            Some(repaired_qid)
        );
        assert_eq!(debug.locked_count, 1);
        assert_eq!(debug.candidate_count, 3);
        assert_eq!(update.snapshot.queue[0].id, repaired_qid);
        assert!(
            update
                .snapshot
                .queue
                .iter()
                .any(|item| item.id == duplicate_qid)
        );
    }

    #[test]
    fn lookup_listen_source_maps_radio_pending_and_automix_new() {
        let conn = conn();
        for (source, expected) in [
            ("radio_pending", crate::db::models::ListenSource::Radio),
            ("automix-new", crate::db::models::ListenSource::Automix),
        ] {
            conn.execute("DELETE FROM queue", []).unwrap();
            conn.execute(
                "INSERT INTO queue (track_id, position, source) VALUES (1, 0, ?1)",
                params![source],
            )
            .unwrap();
            let qid = conn.last_insert_rowid();
            conn.execute(
                "UPDATE playback_state
                 SET current_track_id = 1, current_queue_item_id = ?1
                 WHERE id = 1",
                params![qid],
            )
            .unwrap();

            assert_eq!(lookup_current_listen_source(&conn), expected);
        }
    }

    #[test]
    fn next_track_starts_first_queue_item_when_no_current_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        conn.execute(
            "UPDATE playback_state
             SET current_track_id = NULL, current_queue_item_id = NULL, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(2));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            snapshot.queue.first().map(|item| item.id)
        );
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn next_track_stops_when_current_anchor_is_stale() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 9999, current_queue_item_id = NULL, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), None);
        assert_eq!(snapshot.state.current_queue_item_id, None);
        assert!(!snapshot.state.is_playing);
    }

    #[test]
    fn peek_next_track_returns_none_when_current_anchor_is_stale() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();

        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 9999, current_queue_item_id = NULL, is_playing = 1
             WHERE id = 1",
            [],
        )
        .unwrap();

        let next = peek_next_track(&conn, false).unwrap();

        assert!(next.is_none());
    }

    #[test]
    fn next_track_uses_current_queue_item_id_for_duplicate_tracks() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 1, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        let second_track_one = queue_items
            .iter()
            .find(|item| item.position == 2 && item.track.id == 1)
            .expect("second copy of track 1");
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![second_track_one.id],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(3));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            queue_items
                .iter()
                .find(|item| item.track.id == 3)
                .map(|item| item.id)
        );
    }

    #[test]
    fn next_track_ignores_mismatched_current_queue_item_id() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 2, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let snapshot = next_track(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(3));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            queue_items
                .iter()
                .find(|item| item.track.id == 3)
                .map(|item| item.id)
        );
    }

    #[test]
    fn start_queue_from_beginning_ignores_previous_current_anchor() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2]);
        let queue_items = queue::replace_queue(&conn, &tracks, "test").unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_items[0].id],
        )
        .unwrap();

        let snapshot = start_queue_from_beginning(&conn, false).unwrap();

        assert_eq!(snapshot.state.current_track.as_ref().map(|t| t.id), Some(1));
        assert_eq!(
            snapshot.state.current_queue_item_id,
            Some(queue_items[0].id)
        );
        assert!(snapshot.state.is_playing);
    }

    #[test]
    fn reconcile_advances_current_when_deleted() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let queue_after_seed = queue::load_queue(&conn).unwrap();
        // Mark t1 as current.
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![queue_after_seed[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.queue_changed);
        assert!(outcome.current_changed);
        assert!(!outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, Some(2));

        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(2));
        assert!(state.is_playing);

        let remaining = queue::load_queue(&conn).unwrap();
        assert_eq!(remaining.len(), 2);
        assert_eq!(remaining[0].position, 0);
        assert_eq!(remaining[1].position, 1);
    }

    #[test]
    fn reconcile_stops_playback_when_no_survivors() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.queue_changed);
        assert!(outcome.current_changed);
        assert!(outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, None);

        let state = load_state(&conn).unwrap();
        assert!(state.current_track.is_none());
        assert!(!state.is_playing);
    }

    #[test]
    fn reconcile_skips_pending_rows() {
        let conn = conn();
        // Seed: one library row (track 1, current) followed by a pending row.
        conn.execute(
            "INSERT INTO queue (track_id, position, source) VALUES (1, 0, 'test')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO queue (track_id, position, source, pending_artist, pending_title, pending_at)
             VALUES (NULL, 1, 'radio_pending', 'Pending Artist', 'Pending Title', datetime('now'))",
            [],
        )
        .unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        let outcome = reconcile_after_track_delete(&conn, &[1]).unwrap();
        assert!(outcome.current_changed);
        assert!(!outcome.stopped_playback);
        // Pending rows always survive - they don't reference local track IDs.
        assert_eq!(outcome.new_current_track_id, None);

        let remaining = queue::load_queue(&conn).unwrap();
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].is_pending);
    }

    #[test]
    fn reconcile_noop_when_current_not_in_deleted_set() {
        let conn = conn();
        let tracks = load_tracks(&conn, &[1, 2, 3]);
        queue::replace_queue(&conn, &tracks, "test").unwrap();
        let q = queue::load_queue(&conn).unwrap();
        conn.execute(
            "UPDATE playback_state
             SET current_track_id = 1, current_queue_item_id = ?1, is_playing = 1
             WHERE id = 1",
            params![q[0].id],
        )
        .unwrap();

        // Delete track 3 - current (track 1) is unaffected.
        let outcome = reconcile_after_track_delete(&conn, &[3]).unwrap();
        assert!(outcome.queue_changed);
        assert!(!outcome.current_changed);
        assert!(!outcome.stopped_playback);
        assert_eq!(outcome.new_current_track_id, Some(1));

        let state = load_state(&conn).unwrap();
        assert_eq!(state.current_track.as_ref().map(|t| t.id), Some(1));
    }

    /// Test-only convenience: run the automix extension builder and discard
    /// the per-row reasons, returning just the tracks. Keeps the extension
    /// tests focused on selection behaviour without threading through
    /// `AutomixSelection` at every call site.
    fn extension_tracks(
        conn: &Connection,
        current_track: &Track,
        queue_items: &[QueueItem],
        mode: ShuffleMode,
        needed: usize,
        use_learning: bool,
    ) -> Result<Vec<Track>> {
        Ok(build_automix_extension_with_reasons(
            conn,
            current_track,
            queue_items,
            mode,
            None,
            needed,
            use_learning,
        )?
        .into_iter()
        .map(|selection| selection.track)
        .collect())
    }

    /// Build an isolated DB fixture with the full surface
    /// `build_automix_extension_with_reasons` needs (the standard `conn()`
    /// helper above lacks `embedding_models`, `track_embeddings`, and
    /// `track_similarity`). Returns a connection with one seed track
    /// inserted but **no** embedding row and **no** similarity rows -
    /// the "no recommendation signal" case the guard targets.
    fn empty_signal_conn() -> (Connection, Track) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, artwork_url TEXT, year INTEGER);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                disc_number INTEGER,
                track_number INTEGER,
                duration_ms INTEGER,
                isrc TEXT,
                tidal_id INTEGER,
                ytmusic_id TEXT,
                soundcloud_id INTEGER,
                best_quality TEXT,
                best_source TEXT,
                fidelity_score INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                play_count INTEGER DEFAULT 0,
                last_played_at TEXT,
                date_added TEXT,
                source TEXT DEFAULT 'tidal'
            );
            CREATE TABLE queue (
                id               INTEGER PRIMARY KEY,
                track_id         INTEGER,
                position         INTEGER NOT NULL,
                source           TEXT    DEFAULT 'user',
                reason           TEXT,
                pending_artist   TEXT,
                pending_title    TEXT,
                pending_at       TIMESTAMP,
                resolving_at     TIMESTAMP,
                resolved_at      TIMESTAMP,
                tidal_match_score REAL,
                tidal_id_hint    INTEGER,
                ephemeral_album_title TEXT,
                ephemeral_artwork_url TEXT,
                ephemeral_duration_ms INTEGER,
                ephemeral_artist_tidal_id INTEGER,
                ephemeral_album_tidal_id INTEGER
            );
            CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL, parent_id INTEGER);
            CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT,
                confidence REAL DEFAULT 1.0
            );
            CREATE TABLE listen_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                duration_listened_ms INTEGER DEFAULT 0,
                completed INTEGER DEFAULT 0
            );
            CREATE TABLE embedding_models (
                id INTEGER PRIMARY KEY,
                model_key TEXT NOT NULL UNIQUE,
                family TEXT NOT NULL,
                dimension INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                is_active INTEGER NOT NULL DEFAULT 0,
                trained_at TEXT,
                config_json TEXT,
                metrics_json TEXT,
                created_at TEXT
            );
            CREATE TABLE track_embeddings (
                model_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                vector_blob BLOB NOT NULL,
                PRIMARY KEY (model_id, track_id)
            );
            CREATE TABLE track_similarity (
                track_a INTEGER NOT NULL,
                track_b INTEGER NOT NULL,
                similarity_score REAL NOT NULL DEFAULT 0,
                co_listen_score REAL DEFAULT 0,
                co_album_score REAL DEFAULT 0,
                co_artist_score REAL DEFAULT 0,
                genre_proximity REAL DEFAULT 0,
                duration_proximity REAL DEFAULT 0,
                era_proximity REAL DEFAULT 0,
                computed_at TEXT,
                PRIMARY KEY (track_a, track_b)
            );
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO artists (id, name) VALUES (1, 'Unenriched Artist')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
             VALUES (1, 'Fresh Tidal Import', 1, 'tidal_stream', 0)",
            [],
        )
        .unwrap();

        let track = queue::get_track_by_id(&conn, 1).unwrap().unwrap();
        (conn, track)
    }

    /// Phase 2c hotfix: when both the embedding fast-path AND the
    /// precomputed similarity table are empty for a seed,
    /// `build_automix_extension` must return `Vec::new()` rather than
    /// falling back to a 500-track random library pool.
    ///
    /// Reproduces the Amy Shark "I Said Hi" symptom from the
    /// diagnostic: fresh `tidal_stream` import, no enrichment, no
    /// similarity rows, no embedding. Pre-fix behaviour was a queue
    /// full of unrelated library tracks (Mac Miller, Bob Marley,
    /// James Brown, etc.); post-fix the queue ends gracefully.
    #[test]
    fn build_automix_extension_returns_empty_when_seed_has_no_signal() {
        let (conn, seed) = empty_signal_conn();
        let extension = extension_tracks(
            &conn,
            &seed,
            &[], // empty queue
            ShuffleMode::Off,
            12,   // typical needed
            true, // use_learning enabled - fast-path will run, find no model, fall through
        )
        .expect("extension call");
        assert!(
            extension.is_empty(),
            "expected empty extension for seed with no signal, got {} tracks",
            extension.len()
        );
    }

    /// Same fixture but with one similarity row for the seed → the
    /// guard should NOT fire. We deliberately don't assert on the
    /// extension's exact contents (that depends on scoring), only on
    /// the fact that the guard's early-return path didn't engage.
    /// Sparse-but-non-empty signal is the documented "still falls
    /// through to random pool below" case.
    #[test]
    fn build_automix_extension_does_not_skip_when_seed_has_some_signal() {
        let (conn, seed) = empty_signal_conn();
        // Add a second track + one similarity row from seed (id=1) to it.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, source, fidelity_score)
             VALUES (2, 'Other Track', 1, 'tidal_stream', 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_similarity (track_a, track_b, similarity_score)
             VALUES (1, 2, 0.5)",
            [],
        )
        .unwrap();

        let extension = extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true)
            .expect("extension call");

        // We don't pin contents - only that the empty-signal guard
        // didn't bail. Sparse-signal seeds still walk through the
        // random-pool path which is intended legacy behaviour.
        assert!(
            !extension.is_empty(),
            "expected non-empty extension once a similarity row exists"
        );
    }

    #[test]
    fn learned_automix_builds_chain_aware_order_from_overfetched_neighbors() {
        let conn = conn();
        conn.execute_batch(
            "
            CREATE TABLE track_neighbors (
                track_id INTEGER NOT NULL,
                neighbor_track_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                rank INTEGER NOT NULL,
                score REAL NOT NULL DEFAULT 0,
                behavioral_score REAL DEFAULT 0,
                audio_score REAL DEFAULT 0,
                metadata_score REAL DEFAULT 0,
                reason_json TEXT,
                computed_at TEXT DEFAULT (datetime('now')),
                primary_reason TEXT,
                confidence REAL NOT NULL DEFAULT 0,
                support_count INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
                play_count_seed INTEGER NOT NULL DEFAULT 0,
                play_count_candidate INTEGER NOT NULL DEFAULT 0,
                support_transition REAL NOT NULL DEFAULT 0,
                support_colisten REAL NOT NULL DEFAULT 0,
                support_structure REAL NOT NULL DEFAULT 0,
                support_metadata REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (track_id, neighbor_track_id, model_id)
            );
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
            INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
            INSERT INTO embedding_models (
                id, model_key, family, dimension, status, is_active, trained_at, created_at
            ) VALUES (
                1, 'test-chain', 'discovery-fusion-v2', 3, 'ready', 1,
                '2026-01-01 00:00:00', '2026-01-01 00:00:00'
            );
            INSERT INTO track_neighbors (
                track_id, neighbor_track_id, model_id, rank, score, audio_score, primary_reason
            ) VALUES
                (1, 2, 1, 1, 0.90, 0.90, 'audio_texture'),
                (1, 3, 1, 2, 0.89, 0.89, 'audio_texture'),
                (1, 4, 1, 3, 0.88, 0.88, 'audio_texture');
            INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
                (1, 120.0, '1A'),
                (2, 120.0, '2A'),
                (3, 120.0, '12A'),
                (4, 120.0, '3A');
            ",
        )
        .expect("schema");
        let seed = queue::get_track_by_id(&conn, 1).unwrap().unwrap();

        let extension =
            extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 3, true).expect("extension call");

        assert_eq!(
            extension.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![2, 4, 3]
        );
    }

    #[test]
    fn learned_automix_smoke_prefers_next_track_fit_over_vague_similarity() {
        let conn = conn();
        conn.execute_batch(
            "
            CREATE TABLE track_neighbors (
                track_id INTEGER NOT NULL,
                neighbor_track_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                rank INTEGER NOT NULL,
                score REAL NOT NULL DEFAULT 0,
                behavioral_score REAL DEFAULT 0,
                audio_score REAL DEFAULT 0,
                metadata_score REAL DEFAULT 0,
                reason_json TEXT,
                computed_at TEXT DEFAULT (datetime('now')),
                primary_reason TEXT,
                confidence REAL NOT NULL DEFAULT 0,
                support_count INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
                play_count_seed INTEGER NOT NULL DEFAULT 0,
                play_count_candidate INTEGER NOT NULL DEFAULT 0,
                support_transition REAL NOT NULL DEFAULT 0,
                support_colisten REAL NOT NULL DEFAULT 0,
                support_structure REAL NOT NULL DEFAULT 0,
                support_metadata REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (track_id, neighbor_track_id, model_id)
            );
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
            INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
            INSERT INTO embedding_models (
                id, model_key, family, dimension, status, is_active, trained_at, created_at
            ) VALUES (
                1, 'test-smoke', 'discovery-fusion-v2', 3, 'ready', 1,
                '2026-01-01 00:00:00', '2026-01-01 00:00:00'
            );
            INSERT INTO track_neighbors (
                track_id, neighbor_track_id, model_id, rank, score, behavioral_score,
                audio_score, metadata_score, reason_json, primary_reason, support_colisten
            ) VALUES
                (1, 2, 1, 1, 0.99, 0.80, 0.10, 0.10, '[{\"key\":\"behavioral\"}]', 'behavioral', 1.0),
                (1, 3, 1, 2, 0.98, 0.00, 0.98, 0.00, '[{\"key\":\"audio_texture\"}]', 'audio_texture', 0.0),
                (1, 4, 1, 3, 0.97, 0.00, 0.10, 0.00, '[{\"key\":\"lastfm_direct\"}]', 'lastfm_direct', 0.0),
                (1, 5, 1, 4, 0.96, 0.00, 0.10, 0.00, '[{\"key\":\"lastfm_branch\"}]', 'lastfm_branch', 0.0),
                (1, 6, 1, 5, 0.70, 0.00, 0.20, 0.30, '[{\"key\":\"bpm_match\"},{\"key\":\"harmonic_match\"}]', 'bpm_match', 0.0);
            INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
                (1, 120.0, '1A'),
                (2, 150.0, '6B'),
                (3, 120.0, '1A'),
                (4, 145.0, '6B'),
                (5, 120.0, '1A'),
                (6, 120.0, '1A');
            ",
        )
        .expect("schema");
        let seed = queue::get_track_by_id(&conn, 1).unwrap().unwrap();

        let extension =
            extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 5, true).expect("extension call");

        assert_eq!(extension.first().map(|track| track.id), Some(6));
        assert_eq!(
            extension.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![6, 5, 3, 4, 2]
        );
    }

    #[test]
    fn automix_evaluator_reports_before_after_without_queue_insert() {
        let conn = conn();
        conn.execute_batch(
            "
            CREATE TABLE track_neighbors (
                track_id INTEGER NOT NULL,
                neighbor_track_id INTEGER NOT NULL,
                model_id INTEGER NOT NULL,
                rank INTEGER NOT NULL,
                score REAL NOT NULL DEFAULT 0,
                behavioral_score REAL DEFAULT 0,
                audio_score REAL DEFAULT 0,
                metadata_score REAL DEFAULT 0,
                reason_json TEXT,
                computed_at TEXT DEFAULT (datetime('now')),
                primary_reason TEXT,
                confidence REAL NOT NULL DEFAULT 0,
                support_count INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree INTEGER NOT NULL DEFAULT 0,
                candidate_in_degree_percentile REAL NOT NULL DEFAULT 0,
                play_count_seed INTEGER NOT NULL DEFAULT 0,
                play_count_candidate INTEGER NOT NULL DEFAULT 0,
                support_transition REAL NOT NULL DEFAULT 0,
                support_colisten REAL NOT NULL DEFAULT 0,
                support_structure REAL NOT NULL DEFAULT 0,
                support_metadata REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (track_id, neighbor_track_id, model_id)
            );
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
            INSERT INTO server_config (key, value) VALUES ('discovery_engine', 'v2');
            INSERT INTO embedding_models (
                id, model_key, family, dimension, status, is_active, trained_at, created_at
            ) VALUES (
                1, 'test-evaluator', 'discovery-fusion-v2', 3, 'ready', 1,
                '2026-01-01 00:00:00', '2026-01-01 00:00:00'
            );
            INSERT INTO track_neighbors (
                track_id, neighbor_track_id, model_id, rank, score, audio_score,
                metadata_score, reason_json, primary_reason
            ) VALUES
                (1, 2, 1, 1, 0.90, 0.40, 0.20, '[{\"key\":\"bpm_match\"}]', 'bpm_match');
            INSERT INTO audio_dsp_features (track_id, bpm, camelot_key) VALUES
                (1, 120.0, '1A'),
                (2, 120.5, '1A');
            ",
        )
        .expect("schema");

        let report = evaluate_automix_for_seed(&conn, 1, 1).expect("evaluate");

        assert_eq!(report.queue_len_before, report.queue_len_after);
        assert_eq!(report.before.len(), 1);
        assert_eq!(report.after.len(), 1);
        assert_eq!(report.before[0].track_id, 2);
        assert_eq!(report.after[0].track_id, 2);
        assert_eq!(report.after[0].bpm, Some(120.5));
        assert_eq!(report.after[0].camelot_key.as_deref(), Some("1A"));
        assert!(report.after[0].final_score.is_some());
    }

    /// Metadata fallback: seed has no embedding/similarity signal but the
    /// artist has other tracks in the library. The cascade should return
    /// same-artist tracks rather than ending the queue.
    #[test]
    fn build_automix_extension_falls_back_to_same_artist_when_no_signal() {
        let (conn, seed) = empty_signal_conn();
        // Add four more tracks by the same artist (id=1).
        for i in 2..=5 {
            conn.execute(
                &format!(
                    "INSERT INTO tracks (id, title, artist_id, source, fidelity_score) \
                     VALUES ({i}, 'Track {i}', 1, 'tidal_stream', 0)"
                ),
                [],
            )
            .unwrap();
        }

        let extension = extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true)
            .expect("extension call");

        assert!(
            !extension.is_empty(),
            "expected same-artist tracks in fallback, got empty extension"
        );
        assert!(
            extension.iter().all(|t| t.artist_id == seed.artist_id),
            "expected all fallback tracks to share the seed's artist_id"
        );
        // Seed itself must not appear.
        assert!(
            !extension.iter().any(|t| t.id == seed.id),
            "seed track must not appear in its own extension"
        );
    }

    /// Metadata fallback: seed has no signal AND is the only track by its
    /// artist/album, with no genre tags - the truly-orphan path. The
    /// extension must stay empty (no random kitchen-sink fill).
    #[test]
    fn build_automix_extension_returns_empty_when_no_artist_album_or_genre() {
        let (conn, seed) = empty_signal_conn();
        // No additional tracks, no genre rows - seed is completely isolated.
        let extension = extension_tracks(&conn, &seed, &[], ShuffleMode::Off, 12, true)
            .expect("extension call");

        assert!(
            extension.is_empty(),
            "expected empty extension for a fully isolated seed, got {} tracks",
            extension.len()
        );
    }
}

#[cfg(test)]
mod quality_precedence_tests {
    use super::*;
    use crate::db::audio_settings::AudioQuality;

    fn track_with_best(best: Option<&str>) -> Track {
        Track {
            id: 1,
            title: "T".to_string(),
            artist_id: 1,
            artist_name: None,
            album_id: None,
            album_title: None,
            disc_number: None,
            track_number: None,
            duration_ms: None,
            isrc: None,
            tidal_id: Some(1),
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: best.map(String::from),
            best_source: None,
            fidelity_score: 0,
            is_favorite: false,
            play_count: 0,
            last_played_at: None,
            date_added: None,
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn user_pref_overrides_track_best_quality() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, Some(AudioQuality::Lossless));
        assert_eq!(got, "LOSSLESS");
    }

    #[test]
    fn falls_back_to_track_when_no_user_pref() {
        let t = track_with_best(Some("HI_RES_LOSSLESS"));
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, "HI_RES_LOSSLESS");
    }

    #[test]
    fn falls_back_to_default_when_neither_set() {
        let t = track_with_best(None);
        let got = preferred_tidal_quality(&t, None);
        assert_eq!(got, crate::services::tidal::stream::DEFAULT_AUDIO_QUALITY);
    }
}

#[cfg(test)]
mod parity_tests {
    //! Parity gate for the TasteVector migration (Phase 1).
    //!
    //! Runs the live `automix_score` against a frozen snapshot
    //! (`automix_score_old`) on a fixed synthetic fixture and asserts the
    //! top-30 candidate ordering matches exactly. In the scaffolding commit
    //! the snapshot is byte-identical to the live function so the gate
    //! passes trivially; in the migration commit the live function is
    //! rewritten to consume `TasteVector`/`SeedContext` while the snapshot
    //! stays frozen, so divergence indicates a migration bug rather than a
    //! tuning miss.
    //!
    //! Fixture is built in-memory with no database - `automix_score` only
    //! consumes plain data, so a DB round-trip would add noise without
    //! adding signal.
    use super::*;
    use crate::playback::automix::{automix_score, parse_days_since_last_played};
    use std::collections::{HashMap, HashSet};

    /// Frozen snapshot of `automix_score` taken at Phase 1 start. The body
    /// is a verbatim copy of `super::automix_score` and must not be
    /// modified by the migration commit - the whole point is that this
    /// path keeps producing the original numbers while the live function
    /// changes shape underneath it.
    fn automix_score_old(
        track: &Track,
        genres: &[String],
        taste: &SessionTasteProfile,
        seed_features: Option<&AudioDspFeatures>,
        candidate_features: Option<&AudioDspFeatures>,
    ) -> f64 {
        let mut score = 1.0;

        if taste.skipped_track_ids.contains(&track.id) {
            score *= 0.1;
        }

        if Some(track.artist_id) == taste.current_artist_id && track.artist_id != 0 {
            score *= 1.1;
        }

        if taste.current_source.as_deref() == Some(track.source.as_str()) {
            score *= 1.05;
        }

        if track.is_favorite {
            score *= 1.2;
        }

        if track.play_count == 0 {
            score *= 1.35;
        } else if let Some(last_played) = track.last_played_at.as_deref() {
            let days_since = parse_days_since_last_played(last_played);
            if days_since < 14.0 {
                let penalty = 0.5 + 0.5 * (days_since / 14.0);
                score *= penalty;
            }
        }

        if track.artist_id != 0 {
            score += taste
                .positive_artists
                .get(&track.artist_id)
                .copied()
                .unwrap_or(0.0)
                * 0.5;
            score -= taste
                .negative_artists
                .get(&track.artist_id)
                .copied()
                .unwrap_or(0.0)
                * 0.65;
        }

        let normalized_genres = genres.iter().map(|genre| normalize_genre_key(genre));
        for genre in normalized_genres {
            if taste.current_genres.contains(&genre) {
                score += 1.8;
            }
            score += taste.positive_genres.get(&genre).copied().unwrap_or(0.0) * 0.4;
            score -= taste.negative_genres.get(&genre).copied().unwrap_or(0.0) * 0.5;
        }

        score += (track.fidelity_score.max(0) as f64) * 0.003;

        if let (Some(seed), Some(cand)) = (seed_features, candidate_features) {
            score *= compute_harmonic_multiplier(
                seed.camelot_key.as_deref(),
                cand.camelot_key.as_deref(),
                seed.bpm,
                cand.bpm,
            );

            if let (Some(seed_energy), Some(cand_energy)) = (seed.energy, cand.energy)
                && (seed_energy - cand_energy).abs() > 0.5
            {
                score *= 0.7;
            }
        }

        score.max(0.05)
    }

    struct CandidateInput {
        track: Track,
        genres: Vec<String>,
        features: Option<AudioDspFeatures>,
    }

    struct Fixture {
        profile: SessionTasteProfile,
        seed_features: Option<AudioDspFeatures>,
        candidates: Vec<CandidateInput>,
    }

    fn make_track(
        id: i64,
        artist_id: i64,
        is_favorite: bool,
        play_count: i32,
        fidelity_score: i32,
        source: &str,
    ) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist_id,
            artist_name: Some(format!("Artist {artist_id}")),
            album_id: Some(artist_id * 10),
            album_title: Some(format!("Album {artist_id}")),
            disc_number: Some(1),
            track_number: Some(1),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(id),
            artist_tidal_id: None,
            album_tidal_id: None,
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some(source.to_string()),
            fidelity_score,
            is_favorite,
            play_count,
            last_played_at: None,
            date_added: Some("2025-01-01T00:00:00Z".to_string()),
            source: source.to_string(),
            artwork_url: None,
        }
    }

    fn make_features(camelot: &str, bpm: f64, energy: f64) -> AudioDspFeatures {
        AudioDspFeatures {
            track_id: 0,
            bpm: Some(bpm),
            key_signature: None,
            camelot_key: Some(camelot.to_string()),
            loudness_lufs: None,
            energy: Some(energy),
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

    /// Synthetic fixture exercising every branch in `automix_score`:
    /// hard suppression (skipped track), same-artist boost, same-source
    /// boost, favourite multiplier, unplayed boost, positive/negative
    /// artist signal, positive/negative genre signal, current-genre
    /// proximity bonus, fidelity tilt, harmonic multiplier, and energy
    /// whiplash penalty.
    fn build_fixture() -> Fixture {
        let mut profile = SessionTasteProfile {
            current_artist_id: Some(1),
            current_album_id: Some(10),
            current_source: Some("tidal".to_string()),
            ..SessionTasteProfile::default()
        };
        profile.current_genres.insert("house".to_string());
        for (id, weight) in [(1, 8.5_f64), (2, 5.2), (3, 3.0), (4, 1.5), (5, 0.8)] {
            profile.positive_artists.insert(id, weight);
        }
        for (id, weight) in [(2, 1.1_f64), (3, 2.4)] {
            profile.negative_artists.insert(id, weight);
        }
        for (genre, weight) in [("house", 6.4_f64), ("techno", 4.2), ("ambient", 2.0)] {
            profile.positive_genres.insert(genre.to_string(), weight);
        }
        for (genre, weight) in [("jazz", 1.8_f64), ("ambient", 0.4)] {
            profile.negative_genres.insert(genre.to_string(), weight);
        }
        for id in [300, 301, 302, 305] {
            profile.recent_track_ids.insert(id);
        }
        profile.skipped_track_ids.insert(305);

        let seed_features = Some(make_features("8A", 124.0, 0.7));

        let candidates = vec![
            CandidateInput {
                track: make_track(100, 1, true, 0, 80, "tidal"),
                genres: vec!["House".to_string()],
                features: Some(make_features("8A", 125.0, 0.65)),
            },
            CandidateInput {
                track: make_track(101, 1, false, 2, 60, "tidal"),
                genres: vec!["Techno".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(102, 2, false, 0, 70, "tidal"),
                genres: vec!["House".to_string(), "Techno".to_string()],
                features: Some(make_features("9A", 128.0, 0.75)),
            },
            CandidateInput {
                track: make_track(103, 3, false, 5, 40, "tidal"),
                genres: vec!["Jazz".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(104, 4, true, 0, 50, "tidal"),
                genres: vec!["Ambient".to_string()],
                features: Some(make_features("3B", 110.0, 0.2)),
            },
            CandidateInput {
                track: make_track(105, 5, false, 1, 30, "tidal"),
                genres: vec!["House".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(106, 6, false, 0, 70, "tidal"),
                genres: vec!["Techno".to_string()],
                features: Some(make_features("8A", 124.0, 0.7)),
            },
            CandidateInput {
                track: make_track(107, 7, false, 3, 20, "local"),
                genres: vec!["Jazz".to_string(), "Ambient".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(108, 2, false, 0, 10, "tidal"),
                genres: vec![],
                features: None,
            },
            CandidateInput {
                track: make_track(109, 1, false, 0, 90, "tidal"),
                genres: vec!["House".to_string(), "Ambient".to_string()],
                features: Some(make_features("2A", 130.0, 0.85)),
            },
            CandidateInput {
                track: make_track(110, 3, true, 4, 85, "tidal"),
                genres: vec!["Techno".to_string()],
                features: None,
            },
            CandidateInput {
                track: make_track(305, 4, false, 0, 60, "tidal"),
                genres: vec!["House".to_string()],
                features: None,
            },
        ];

        Fixture {
            profile,
            seed_features,
            candidates,
        }
    }

    /// Builds a `TasteVector` + `SeedContext` from the fixture's
    /// `SessionTasteProfile` via `from_session_profile`, then calls the
    /// migrated `automix_score`. Conversion happens once per call (not per
    /// candidate) to mirror the production call site.
    fn score_with_new_path(fixture: &Fixture) -> Vec<(i64, f64)> {
        let (taste, seed) =
            crate::smart::taste_vector::adapters::from_session_profile(&fixture.profile);
        fixture
            .candidates
            .iter()
            .map(|cand| {
                let score = automix_score(
                    &cand.track,
                    &cand.genres,
                    &taste,
                    &seed,
                    fixture.seed_features.as_ref(),
                    cand.features.as_ref(),
                )
                .value;
                (cand.track.id, score)
            })
            .collect()
    }

    fn score_with_old_path(fixture: &Fixture) -> Vec<(i64, f64)> {
        fixture
            .candidates
            .iter()
            .map(|cand| {
                let score = automix_score_old(
                    &cand.track,
                    &cand.genres,
                    &fixture.profile,
                    fixture.seed_features.as_ref(),
                    cand.features.as_ref(),
                );
                (cand.track.id, score)
            })
            .collect()
    }

    fn top_n_ids(scores: &[(i64, f64)], n: usize) -> Vec<i64> {
        let mut sorted: Vec<(i64, f64)> = scores.to_vec();
        sorted.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        sorted.into_iter().take(n).map(|(id, _)| id).collect()
    }

    fn kendall_tau_top_n(new_scores: &[(i64, f64)], old_scores: &[(i64, f64)], n: usize) -> f64 {
        let new_top = top_n_ids(new_scores, n);
        let old_top = top_n_ids(old_scores, n);
        let new_rank: HashMap<i64, usize> =
            new_top.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let old_rank: HashMap<i64, usize> =
            old_top.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let union: HashSet<i64> = new_top.iter().chain(old_top.iter()).copied().collect();
        let items: Vec<i64> = union.into_iter().collect();

        let mut concordant = 0i64;
        let mut discordant = 0i64;
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let (a, b) = (items[i], items[j]);
                if let (Some(&na), Some(&nb), Some(&oa), Some(&ob)) = (
                    new_rank.get(&a),
                    new_rank.get(&b),
                    old_rank.get(&a),
                    old_rank.get(&b),
                ) {
                    let new_order = na.cmp(&nb);
                    let old_order = oa.cmp(&ob);
                    if new_order == old_order {
                        concordant += 1;
                    } else {
                        discordant += 1;
                    }
                }
            }
        }
        let total = concordant + discordant;
        if total == 0 {
            1.0
        } else {
            (concordant - discordant) as f64 / total as f64
        }
    }

    /// Per-track structured table emitted on parity failure. Sorted by
    /// `|new_score - old_score|` descending so tiny diffs at the top
    /// suggest float precision while large diffs at the top point to a
    /// logic bug. Free triage signal.
    fn emit_divergence_table(new_scores: &[(i64, f64)], old_scores: &[(i64, f64)]) {
        let new_lookup: HashMap<i64, f64> = new_scores.iter().copied().collect();
        let old_lookup: HashMap<i64, f64> = old_scores.iter().copied().collect();
        let new_full = top_n_ids(new_scores, new_scores.len());
        let old_full = top_n_ids(old_scores, old_scores.len());
        let new_rank: HashMap<i64, usize> = new_full
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let old_rank: HashMap<i64, usize> = old_full
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();

        let mut rows: Vec<(i64, usize, usize, f64, f64)> = new_scores
            .iter()
            .map(|(id, new_s)| {
                let old_s = old_lookup.get(id).copied().unwrap_or(f64::NAN);
                let nr = *new_rank.get(id).unwrap_or(&usize::MAX);
                let or = *old_rank.get(id).unwrap_or(&usize::MAX);
                (*id, or, nr, old_s, *new_s)
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.4 - b.3)
                .abs()
                .partial_cmp(&(a.4 - a.3).abs())
                .unwrap_or(Ordering::Equal)
        });

        eprintln!(
            "{:>8}  {:>8}  {:>8}  {:>10}  {:>11}  {:>11}  {:>13}",
            "track_id",
            "old_rank",
            "new_rank",
            "rank_delta",
            "old_score",
            "new_score",
            "score_delta"
        );
        for (id, or, nr, old_s, new_s) in rows {
            let rank_delta = (nr as i64) - (or as i64);
            eprintln!(
                "{:>8}  {:>8}  {:>8}  {:>10}  {:>11.6}  {:>11.6}  {:>13.9}",
                id,
                or,
                nr,
                rank_delta,
                old_s,
                new_s,
                new_s - old_s
            );
        }
        // Note: per-signal contribution breakdown is intentionally not
        // emitted here. If parity fails, the score-delta column above plus
        // a quick read of `automix_score` against the snapshot is faster
        // than maintaining a parallel breakdown that itself can drift.
        let _ = new_lookup;
    }

    #[test]
    fn automix_score_parity_top_30() {
        let fixture = build_fixture();
        let new_scores = score_with_new_path(&fixture);
        let old_scores = score_with_old_path(&fixture);

        // Pre-assertion guards: same length, no NaN/Inf on either side.
        // Catches NaN-from-zero-division or shape mismatches before we
        // attempt to interpret rankings.
        assert_eq!(
            old_scores.len(),
            new_scores.len(),
            "score vector lengths differ"
        );
        assert!(
            old_scores.iter().all(|(_, s)| s.is_finite()),
            "old path produced NaN/Inf"
        );
        assert!(
            new_scores.iter().all(|(_, s)| s.is_finite()),
            "new path produced NaN/Inf"
        );

        let n = 30.min(fixture.candidates.len());
        let new_top = top_n_ids(&new_scores, n);
        let old_top = top_n_ids(&old_scores, n);

        let tau = kendall_tau_top_n(&new_scores, &old_scores, n);
        eprintln!("automix parity: kendall_tau_top_{n} = {tau:.6}");

        if new_top != old_top {
            eprintln!("\nautomix parity divergence (sorted by |score_delta| desc):");
            emit_divergence_table(&new_scores, &old_scores);
            panic!("top-{n} ranking diverged.\n  old: {old_top:?}\n  new: {new_top:?}");
        }
    }

    // Characterization test. The previous refactor plan got this exactly backwards:
    // it claimed malformed timestamps incur "maximum recency penalty". They do not.
    // parse_days_since_last_played returns f64::MAX on parse failure, and the caller
    // in automix_score only applies a penalty when `days_since < 14.0` (player.rs:1586).
    // f64::MAX is never < 14.0, so the penalty branch is skipped and the candidate
    // keeps its base score. This test pins that behavior so a future "cleanup" that
    // returns 0.0 or 999.0 on error would be caught.
    #[test]
    fn parse_days_since_last_played_returns_f64_max_on_malformed_input() {
        assert_eq!(parse_days_since_last_played("not a date"), f64::MAX);
        assert_eq!(parse_days_since_last_played(""), f64::MAX);
        assert_eq!(
            parse_days_since_last_played("2026-99-99T99:99:99Z"),
            f64::MAX
        );
        // Sanity: the 14-day penalty gate in automix_score is NOT triggered.
        assert!(parse_days_since_last_played("malformed") >= 14.0);
        // Sanity: a well-formed timestamp parses to a small positive number.
        let recent = parse_days_since_last_played("2026-05-13T12:00:00Z");
        assert!(recent >= 0.0 && recent < 365.0);
    }
}
