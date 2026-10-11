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
use crate::services::audio_analysis::dj_profile::{decode_f32_blob, decode_u32_blob};
use crate::services::tidal::stream::{self, StreamInfo, StreamRequest};
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use crate::db::models::AudioDspFeatures;
#[cfg(test)]
use crate::smart::taste_vector::adapters::from_session_profile;

#[derive(Debug, Clone)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub queue: Vec<QueueItem>,
    pub queue_revision: u64,
}

// Queue snapshots can cross on the wire even though SQLite serializes their
// mutations. Tag each snapshot with a process epoch plus SQLite's monotonic
// connection change counter so clients can reject an older full-queue response.
// The epoch keeps revisions increasing when the sidecar self-heals in place.
static QUEUE_REVISION_EPOCH: LazyLock<u64> = LazyLock::new(|| {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or(0)
});

pub fn queue_revision(conn: &Connection) -> u64 {
    QUEUE_REVISION_EPOCH.saturating_add(conn.total_changes())
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

/// A stream the dispatching route already resolved, handed to the decoder so it
/// does not pay for a second TIDAL `playbackinfo` round-trip.
///
/// Every play/switch/prepare route resolves the stream before building the job,
/// and the decoder used to resolve it again from the bare request - two calls
/// per track start, both through the 4-permit TIDAL request limiter. Measured on
/// the live server across 4436 same-track pairs: the redundant resolve cost
/// p50 254ms, p90 508ms, p99 3.7s, every single track start.
///
/// Reuse is gated on freshness because TIDAL's signed manifest URLs are
/// time-limited: past `STREAM_INFO_REUSE_MAX_AGE` the decoder resolves again
/// rather than risk a stale URL. In practice the decoder starts within
/// milliseconds of the route resolving, so the fresh path is the normal one.
#[derive(Debug, Clone)]
pub struct ResolvedStream {
    pub info: StreamInfo,
    pub resolved_at: std::time::Instant,
}

/// How long a route-resolved stream stays reusable by the decoder. Comfortably
/// longer than the dispatch hop (sub-millisecond to seconds) and far shorter
/// than a TIDAL signed-URL lifetime, so a job that somehow sits around falls
/// back to a fresh resolve instead of failing on an expired URL.
pub const STREAM_INFO_REUSE_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(60);

impl ResolvedStream {
    pub fn is_fresh(&self) -> bool {
        self.resolved_at.elapsed() < STREAM_INFO_REUSE_MAX_AGE
    }
}

#[derive(Debug, Clone)]
pub struct PreparedPlaybackJob {
    pub track: Track,
    pub source: PlaybackSourceRequest,
    pub gapless: GaplessPlan,
    pub generation: u64,
    pub output_sample_rate: Option<u32>,
    /// Stream the dispatching route already resolved. `None` for jobs built
    /// without one (local library, tests); the decoder then resolves itself.
    pub resolved_stream: Option<ResolvedStream>,
    pub dj_media_ref: Option<DjMediaRef>,
    pub prepared_transition: Option<PreparedTransitionProgram>,
    // Segment-aware seek (option C): for DASH-segmented sources, these tell the
    // decoder to skip ahead by `start_from_segment_index` segments. Position
    // accounting in `PlaybackSharedState` is then absolute-track-samples seeded
    // from `start_from_offset_ms`. A fresh play has both = 0.
    pub start_from_segment_index: usize,
    pub start_from_offset_ms: u64,
    // Transport intent carried WITH the job: when true, the engine this job
    // spawns (or promotes) must come up silent. Set from the authoritative DB
    // is_playing at dispatch time so an auto-advance racing a user's pause
    // can no longer cold-start an audible engine while the UI says paused.
    pub start_paused: bool,
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
            resolved_stream: None,
            dj_media_ref: None,
            prepared_transition: None,
            start_from_segment_index: 0,
            start_from_offset_ms: 0,
            start_paused: false,
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
    /// A transport seek invalidates the audible transition view without
    /// discarding the event identity used for listening history and feedback.
    pub transition_visual_valid: bool,
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
/// Listens older than this do not describe the current session (after a week
/// away the last 60 listens were last week's).
const SESSION_FEEDBACK_MAX_AGE_DAYS: i64 = 3;

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
            resolved_stream: None,
            dj_media_ref: None,
            prepared_transition: None,
            start_from_segment_index: 0,
            start_from_offset_ms: 0,
            start_paused: false,
        }
    }

    pub fn with_generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// Carry the user's transport intent with the job. `true` means the
    /// engine spawned or promoted for this job comes up silent (paused).
    /// Dispatch sites derive this from the authoritative `is_playing` right
    /// before sending, so an advance racing a user pause starts muted
    /// instead of blasting audio under a paused UI.
    pub fn with_start_paused(mut self, start_paused: bool) -> Self {
        self.start_paused = start_paused;
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
            let duration_ms = current_duration_ms.or(engine
                .db()
                .with_conn(|conn| current_track_duration_ms(conn, current))?);
            let mut restored_gapless = dj_gapless_plan_from_program(&existing_program);
            if existing_anchor.is_some() {
                // Structural alignment can reserve more tail than the audio
                // envelope itself. Restore that saved window when reusing the
                // event, rather than reverting to the player's crossfade setting.
                if let Some(overlap_ms) = duration_ms
                    .zip(existing.planned_start_ms)
                    .map(|(duration, start)| duration.saturating_sub(start))
                    .filter(|overlap| (250..=DJ_MAX_RENDER_MS as i64).contains(overlap))
                {
                    restored_gapless.overlap_ms = overlap_ms as i32;
                }
            }
            job.gapless = restored_gapless;
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
        let render_timing_unstable = timing_sensitive_dj_program(&plan.program)
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

/// A proposed update for a queued deck. Persist only after the runtime accepts
/// it; an already audible transition must retain its original program.
pub(crate) struct PreparedDjTransitionUpdate {
    pub transition: PreparedTransitionProgram,
    pub gapless: GaplessPlan,
    current: DjMediaRef,
    next: DjMediaRef,
    plan: DjTransitionPlan,
    timing: DjTransitionTimingPlan,
}

pub(crate) fn plan_prepared_dj_transition_update(
    engine: &DjEngine,
    pair: DjLookaheadPair,
    sample_rate: u32,
    channels: u16,
    current_position_ms: i64,
) -> Result<Option<PreparedDjTransitionUpdate>> {
    let (Some(current), Some(next), Some(next_queue_id)) =
        (pair.current, pair.next, pair.next_queue_item_id)
    else {
        return Ok(None);
    };
    let existing = engine
        .db()
        .with_conn(|conn| latest_armed_dj_transition_event_for_pair(conn, &current, &next))?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing
        .planned_start_ms
        .is_none_or(|start| start <= current_position_ms.saturating_add(2_000))
    {
        return Ok(None);
    }
    let Some(mut plan) = engine.plan_transition_details(&current, &next, sample_rate, channels)?
    else {
        return Ok(None);
    };
    let unstable = timing_sensitive_dj_program(&plan.program)
        && engine.db().with_conn(render_timing_unstable)?;
    let (rendered, fallback) =
        v1_renderable_program(&plan.program, sample_rate, channels, unstable);
    plan.program = rendered;
    plan.fallback_reason = fallback.or(plan.fallback_reason);
    let timing = dj_gapless_plan_for_pair(engine, &current, None, &plan.program);
    if timing
        .planned_start_ms
        .is_none_or(|start| start <= current_position_ms.saturating_add(2_000))
    {
        return Ok(None);
    }
    if serde_json::to_string(&plan.program)?
        == serde_json::to_string(&existing.program.clone().rescaled_to(sample_rate))?
        && timing.planned_start_ms == existing.planned_start_ms
        && Some(timing.timing_source) == existing.timing_source.as_deref()
    {
        return Ok(None);
    }
    let fire_ahead_ms = engine.db().with_conn(dj_transition_fire_ahead_ms)?;
    Ok(Some(PreparedDjTransitionUpdate {
        transition: PreparedTransitionProgram {
            program: plan.program.clone(),
            transition_event_id: Some(existing.id),
            fire_ahead_ms,
            queue_generation: pair.queue_generation,
            current_queue_item_id: pair.current_queue_item_id,
            next_queue_item_id: Some(next_queue_id),
            anchor_start_ms: timing.anchor_start_ms,
        },
        gapless: timing.gapless,
        current,
        next,
        plan,
        timing,
    }))
}

pub(crate) fn persist_prepared_dj_transition_update(
    engine: &DjEngine,
    update: &PreparedDjTransitionUpdate,
) -> Result<()> {
    log_dj_transition_event(
        engine,
        update.transition.transition_event_id,
        &update.current,
        &update.next,
        &update.plan.program.template,
        &update.plan,
        &update.timing,
    )?;
    Ok(())
}

impl ArmedDjTransitionEvent {
    /// The planned start doubles as the decoded-audio-time fire anchor, but
    /// only when it was derived from an analysis grid; a fallback overlap's
    /// planned start is metadata arithmetic and must not be fired against.
    fn anchor_start_ms(&self) -> Option<i64> {
        match self.timing_source.as_deref() {
            Some("downbeat_sync" | "beat_sync" | "phrase_sync" | "mix_out_sync") => {
                self.planned_start_ms
            }
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
const DJ_MAX_RENDER_MS: u64 = 28_000;

fn dj_transition_fire_ahead_ms(conn: &Connection) -> Result<u32> {
    let deltas = dj_timing_calibration_deltas(conn, DJ_FIRE_AHEAD_WINDOW)?;
    Ok(fire_ahead_ms_from_deltas(&deltas))
}

fn render_timing_unstable(conn: &Connection) -> Result<bool> {
    let deltas = dj_timing_calibration_deltas(conn, DJ_FILTER_SWEEP_TIMING_WINDOW)?;
    Ok(render_timing_unstable_from_deltas(&deltas))
}

fn dj_timing_calibration_deltas(conn: &Connection, limit: i64) -> Result<Vec<i64>> {
    // Calibration describes recent runtime conditions, not a permanent veto
    // carried by copied libraries or a months-old seek/decoder incident.
    // Historical events remain available unchanged in the timing UI.
    let mut stmt = conn.prepare(
        "SELECT timing_delta_ms
         FROM dj_transition_events
         WHERE timing_status = 'fired'
           AND timing_delta_ms IS NOT NULL
           AND timing_source IN ('downbeat_sync', 'beat_sync', 'phrase_sync', 'mix_out_sync')
           AND runtime_rendered_dj_mixer = 1
           AND runtime_renderer_status IN ('rendered_handoff', 'rendered_overlay')
           AND COALESCE(runtime_renderer_reason, 'none') = 'none'
           AND datetime(started_at) >= datetime('now', '-24 hours')
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
    if values.len().is_multiple_of(2) {
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
    if !profile.profile_confidence.is_finite() || profile.profile_confidence < 0.65 {
        return Ok(None);
    }

    let correction = queries::get_audio_dj_profile_correction(conn, &key)?;
    let beats = decode_f32_blob(&profile.beat_grid_blob).unwrap_or_default();
    if crate::playback::dj_engine::dj_grid_is_synthetic(&profile, &beats) {
        // A zero-origin tempo projection is not measured phase. Scope and
        // raw confidence cannot certify its guessed beats or phrase markers.
        return Ok(None);
    }
    let original_downbeats = decode_f32_blob(&profile.downbeats_blob).unwrap_or_default();
    let beat_ms = median_delta(&grid_intervals_ms(&beats))
        .unwrap_or(500)
        .max(1);
    let multiplier = correction
        .as_ref()
        .and_then(|value| value.bpm_multiplier)
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(1.0);
    let corrected_beat_ms = beat_ms as f64 / multiplier;
    let downbeat_shift_ms = correction
        .as_ref()
        .and_then(|value| value.downbeat_offset_beats)
        .unwrap_or(0) as f64
        * corrected_beat_ms;
    let downbeats = original_downbeats
        .iter()
        .map(|seconds| *seconds + (downbeat_shift_ms / 1_000.0) as f32)
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .collect::<Vec<_>>();
    let phrase_offset = correction
        .as_ref()
        .and_then(|value| value.phrase_offset_bars)
        .unwrap_or(0);
    let phrase_ms = decode_u32_blob(&profile.phrase_boundaries_blob)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|bar| {
            let index = usize::try_from(i64::from(bar) + phrase_offset).ok()?;
            let seconds = *original_downbeats.get(index)?;
            let marker = (f64::from(seconds) * 1_000.0 + downbeat_shift_ms).round() as i64;
            (marker >= 0 && marker <= profile.analysis_scope_ms).then_some(marker)
        })
        .collect::<Vec<_>>();
    // Structure extracted from the first 90 seconds describes that analysed
    // region, not a three-minute track's true outro. Only real, covered tail
    // cues may earn structural preference; phrases are never extrapolated.
    let mix_out_ms = decode_f32_blob(&profile.mix_out_blob)
        .unwrap_or_default()
        .into_iter()
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1_000.0).round() as i64)
        .filter(|ms| *ms <= profile.analysis_scope_ms)
        .collect::<Vec<_>>();
    let outro_ms = profile
        .outro_start_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .map(|seconds| (seconds * 1_000.0).round() as i64)
        .filter(|ms| *ms <= profile.analysis_scope_ms);
    // Playback analysis intentionally covers the first 90 seconds and earns
    // a scope score of .65. That coverage does not weaken a well-supported
    // tempo grid. Keep structural cues scope-bounded, while allowing stable,
    // high-confidence rhythm to retain the established absolute fire anchor.
    let confident_grid = profile
        .beat_confidence
        .is_some_and(|confidence| confidence.is_finite() && confidence >= 0.75);
    let program_ms =
        ((program.resolve_at.saturating_mul(1_000)) / u64::from(program.sample_rate.max(1))) as i64;
    let preferred_ms = if program.template == "SafeCrossfade" {
        i64::from(preferred_overlap_ms).max(8_000)
    } else {
        i64::from(preferred_overlap_ms).max(250)
    };
    for (measured, source) in [(&downbeats, "downbeat_sync"), (&beats, "beat_sync")] {
        let Some((overlap_ms, timing_source)) = tail_transition_candidate_ms(
            duration_ms,
            measured,
            preferred_ms,
            program_ms,
            &program.template,
            confident_grid && stable_grid(measured),
            &phrase_ms,
            &mix_out_ms,
            outro_ms,
            source,
        ) else {
            continue;
        };
        return Ok(Some(SyncedDjOverlap {
            overlap_ms,
            timing_source,
        }));
    }
    Ok(None)
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

#[cfg(test)]
fn synced_overlap_from_grid_ms(
    duration_ms: i64,
    grid_seconds: &[f32],
    preferred_overlap_ms: i32,
    program_samples: Option<u64>,
    sample_rate: u32,
) -> Option<i32> {
    const MAX_SYNC_OVERLAP_MS: i64 = 28_000;
    let preferred_ms = preferred_overlap_ms.max(250) as i64;
    let program_ms = program_samples
        .map(|samples| {
            ((samples.saturating_mul(1000)) / u64::from(sample_rate.max(1)))
                .clamp(250, i64::MAX as u64) as i64
        })
        .unwrap_or(preferred_ms);
    let min_overlap_ms = preferred_ms.max(program_ms).min(MAX_SYNC_OVERLAP_MS);
    let grid = extrapolated_grid_ms(grid_seconds, duration_ms)?;

    grid.into_iter()
        .filter_map(|start_ms| {
            let overlap_ms = duration_ms.saturating_sub(start_ms);
            (overlap_ms >= min_overlap_ms && overlap_ms <= MAX_SYNC_OVERLAP_MS)
                .then_some(overlap_ms as i32)
        })
        .min()
}

fn grid_intervals_ms(grid_seconds: &[f32]) -> Vec<i64> {
    grid_seconds
        .windows(2)
        .filter_map(|pair| {
            let delta = ((pair[1] - pair[0]) * 1_000.0).round();
            (delta.is_finite() && delta > 0.0).then_some(delta as i64)
        })
        .collect()
}

fn stable_grid(grid_seconds: &[f32]) -> bool {
    let intervals = grid_intervals_ms(grid_seconds);
    let Some(median) = median_delta(&intervals).filter(|median| *median >= 100) else {
        return false;
    };
    intervals.len() >= 3
        && intervals
            .iter()
            .filter(|interval| (**interval - median).abs() <= (median / 5).max(20))
            .count()
            * 4
            >= intervals.len() * 3
}

#[allow(clippy::too_many_arguments)]
fn tail_transition_candidate_ms(
    duration_ms: i64,
    measured_grid: &[f32],
    preferred_ms: i64,
    program_ms: i64,
    template: &str,
    extrapolate: bool,
    phrase_ms: &[i64],
    mix_out_ms: &[i64],
    outro_ms: Option<i64>,
    grid_source: &'static str,
) -> Option<(i32, &'static str)> {
    let min_overlap = preferred_ms.max(program_ms).max(250);
    let extra = match template {
        "SlamCut" => 2_000,
        "QuickMix" => 4_000,
        _ => 8_000,
    };
    let max_overlap = (min_overlap + extra).min(DJ_MAX_RENDER_MS as i64);
    let grid = if extrapolate {
        extrapolated_grid_ms(measured_grid, duration_ms)?
    } else {
        measured_grid
            .iter()
            .filter(|seconds| seconds.is_finite() && **seconds >= 0.0)
            .map(|seconds| (seconds * 1_000.0).round() as i64)
            .collect::<Vec<_>>()
    };
    grid.into_iter()
        .filter_map(|start_ms| {
            let overlap = duration_ms.saturating_sub(start_ms);
            if overlap < min_overlap || overlap > max_overlap {
                return None;
            }
            // A structural cue can beat a slightly later grid marker, but never
            // buy an arbitrary mid-song skip. Extra tail removal costs score.
            let phrase = phrase_ms.iter().any(|ms| (*ms - start_ms).abs() <= 100);
            let mix_out = mix_out_ms.iter().any(|ms| (*ms - start_ms).abs() <= 100);
            let outro = outro_ms.is_some_and(|ms| (ms - start_ms).abs() <= 100);
            let score = if phrase {
                0.24
            } else if mix_out {
                0.16
            } else if outro {
                0.10
            } else {
                0.0
            } - (overlap - min_overlap) as f64 / 1_000.0 * 0.04;
            let source = if phrase {
                "phrase_sync"
            } else if mix_out {
                "mix_out_sync"
            } else {
                grid_source
            };
            Some((overlap as i32, source, score))
        })
        .max_by(|left, right| {
            left.2
                .total_cmp(&right.2)
                .then_with(|| right.0.cmp(&left.0))
        })
        .map(|(overlap, source, _)| (overlap, source))
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

    // Keep the fitted fractional period and origin until each final marker is
    // rounded. Repeatedly adding an integer-ms interval accumulates phase
    // error. This projection is still an estimate outside measured coverage;
    // the caller's confidence gates and renderer phase check remain required.
    let fit = noor_mix::beat_grid::fit_beat_grid(grid_seconds)?;
    let last_seconds = *grid.last()? as f64 / 1_000.0;
    let mut index = ((last_seconds - fit.origin_seconds) / fit.period_seconds)
        .floor()
        .max(0.0)
        + 1.0;
    let end_index =
        ((duration_ms as f64 / 1_000.0 - fit.origin_seconds) / fit.period_seconds).ceil();
    if end_index - index > 65_536.0 {
        return None;
    }
    while index < end_index {
        let next = ((fit.origin_seconds + index * fit.period_seconds) * 1_000.0).round() as i64;
        if next >= duration_ms {
            break;
        }
        if grid.last().is_none_or(|previous| next > *previous) {
            grid.push(next);
        }
        index += 1.0;
    }
    Some(grid)
}

fn timing_sensitive_dj_program(program: &noor_mix::TransitionProgram) -> bool {
    // A short unity-rate energy handoff uses the same protected scheduling
    // as QuickMix. It does not claim phrase/drop lock, so historical phase
    // jitter must not erase its energy envelope solely because of its name.
    if matches!(program.template.as_str(), "EnergyLift" | "EnergyReset")
        && program
            .decision
            .as_ref()
            .is_some_and(|decision| decision.outgoing_window == "tempo_informed_short_overlap")
        && program.resolve_at <= 4 * u64::from(program.sample_rate)
        && program.deck_b_start_frame == 0
        && program.automation.iter().all(|event| {
            !matches!(event.param, noor_mix::Param::PlaybackRate(_))
                || ((event.from - 1.0).abs() < 0.0001 && (event.to - 1.0).abs() < 0.0001)
        })
    {
        return false;
    }
    matches!(
        program.template.as_str(),
        "FilterSweep"
            | "BassSwap16"
            | "BassSwap32"
            | "ClubMix"
            | "EnergyLift"
            | "EnergyReset"
            | "DropSwap"
    )
}

fn v1_renderable_program(
    program: &noor_mix::TransitionProgram,
    sample_rate: u32,
    channels: u16,
    render_timing_unstable: bool,
) -> (noor_mix::TransitionProgram, Option<&'static str>) {
    // The safe path is a proven fallback and already carries its own envelope.
    if program.template == "SafeCrossfade" {
        return (program.clone(), None);
    }
    let supported = matches!(
        program.template.as_str(),
        "BassSwap16"
            | "BassSwap32"
            | "SlamCut"
            | "LongHarmonicBlend"
            | "DropTease16"
            | "FilterSweep"
            | "ClubMix"
            | "QuickMix"
            | "EnergyLift"
            | "EnergyReset"
            | "DropSwap"
    );
    let mut renderer_program = program.clone().rescaled_to(sample_rate.max(1));
    renderer_program.channels = channels.max(1);
    let duration_valid = renderer_program.resolve_at > 0
        && renderer_program.resolve_at
            <= DJ_MAX_RENDER_MS * u64::from(renderer_program.sample_rate) / 1_000;
    // The handoff resumes deck B after the frames it consumed. Preserve the
    // existing constant-rate contract; dynamic rates or deck A nudges need a
    // different consumption model and must not reach this renderer.
    let rates = renderer_program
        .automation
        .iter()
        .filter(|event| matches!(event.param, noor_mix::Param::PlaybackRate(_)))
        .collect::<Vec<_>>();
    let rates_valid = rates.len() <= 1
        && rates.iter().all(|event| {
            event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B)
                && event.start_sample == 0
                && event.end_sample >= renderer_program.resolve_at
                && (event.from - event.to).abs() <= 0.0001
        });
    let safety_valid = noor_mix::planner::safety::validate_audio_safety(
        &renderer_program,
        &noor_mix::planner::safety::AudioSafetyPolicy::default(),
    )
    .is_ok();
    let reason = if !supported {
        Some("template_not_renderable")
    } else if render_timing_unstable && timing_sensitive_dj_program(program) {
        Some("timing_unstable")
    } else if !duration_valid || !rates_valid || !safety_valid {
        Some("audio_safety_rejected")
    } else {
        None
    };
    if let Some(reason) = reason {
        let mut fallback = crate::playback::dj_engine::safe_crossfade_program(
            sample_rate,
            channels,
            noor_mix::Policy::default(),
        );
        // Source positions remain useful even when the ambitious envelope
        // cannot be rendered. Never carry rejected rate/automation into safety.
        if reason == "timing_unstable" && rates_valid && safety_valid {
            fallback.deck_a_start_frame = renderer_program.deck_a_start_frame;
            fallback.deck_b_start_frame = renderer_program.deck_b_start_frame;
            if let Some(event) = rates.first() {
                let mut rate = (**event).clone();
                rate.end_sample = fallback.resolve_at;
                fallback.automation.push(rate);
            }
        }
        return (fallback, Some(reason));
    }
    // Keep the planner's beat-derived length, phase markers, gain/EQ curves,
    // source cues, drop provenance and explanation intact. Rebuilding from the
    // template label erased the musical decisions before the mixer heard them.
    (renderer_program, None)
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
            transition_visual_valid: false,
        }
    }

    pub fn with_dj_transition_event_id(mut self, event_id: Option<i64>) -> Self {
        self.dj_transition_event_id = event_id;
        self.transition_visual_valid = event_id.is_some();
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
             WHEN 'late' THEN 0
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
    Ok(PlaybackSnapshot {
        state,
        queue,
        queue_revision: queue_revision(conn),
    })
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

/// Jump the playback anchor to a specific queue row (library or pending).
///
/// Returns `None` when the queue item does not exist. `current_track_id` is
/// NULL for a pending row; the route layer resolves it (import + promote) and
/// switches the audio runtime. Anchoring by queue-item id - not track id -
/// means duplicate tracks in the queue always highlight the clicked row.
pub fn play_queue_item_anchor(
    conn: &Connection,
    queue_item_id: i64,
) -> Result<Option<PlaybackSnapshot>> {
    let row: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, track_id FROM queue WHERE id = ?1",
            params![queue_item_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((queue_id, track_id)) = row else {
        return Ok(None);
    };
    conn.execute(
        "UPDATE playback_state
         SET current_track_id = ?1,
             current_queue_item_id = ?2,
             position_ms = 0,
             is_playing = 1
         WHERE id = 1",
        params![track_id, queue_id],
    )?;
    Ok(Some(load_snapshot(conn)?))
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
    let tx = conn.unchecked_transaction()?;
    let outcome = reconcile_after_track_delete_in_transaction(&tx, deleted_track_ids)?;
    tx.commit()?;
    Ok(outcome)
}

/// Transaction-aware form used by operations that combine queue repair with
/// other track-reference changes. The caller owns commit/rollback.
pub(crate) fn reconcile_after_track_delete_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    deleted_track_ids: &[i64],
) -> Result<ReconcileOutcome> {
    if deleted_track_ids.is_empty() {
        return Ok(ReconcileOutcome::default());
    }

    let deleted_set: HashSet<i64> = deleted_track_ids.iter().copied().collect();

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

    // Carry the caller's already-resolved stream so the decoder can skip its own
    // `playbackinfo` round-trip. Stamped now; the decoder only reuses it while
    // fresh (see `ResolvedStream`).
    let resolved_stream = stream_info.map(|info| ResolvedStream {
        info: info.clone(),
        resolved_at: std::time::Instant::now(),
    });

    let mut job = PreparedPlaybackJob {
        output_sample_rate,
        resolved_stream,
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

/// What a listen says about taste. Only an early skip is negative; a partial
/// play (often a DJ mix-out or a track you moved on from late) says little.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListenOutcome {
    Completed,
    Partial,
    EarlySkip,
}

/// Under 30 seconds, or under a quarter of the track, without completing.
pub(crate) fn classify_listen(
    completed: bool,
    listened_ms: i64,
    duration_ms: Option<i64>,
) -> ListenOutcome {
    if completed {
        return ListenOutcome::Completed;
    }
    let under_quarter =
        duration_ms.is_some_and(|duration| duration > 0 && listened_ms * 4 < duration);
    if listened_ms < 30_000 || under_quarter {
        ListenOutcome::EarlySkip
    } else {
        ListenOutcome::Partial
    }
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
        "SELECT lh.track_id, lh.completed, COALESCE(lh.duration_listened_ms, 0), t.duration_ms
         FROM listen_history lh
         LEFT JOIN tracks t ON t.id = lh.track_id
         WHERE julianday(lh.started_at) >= julianday('now', printf('-%d days', ?2))
         ORDER BY julianday(lh.started_at) DESC, lh.id DESC
         LIMIT ?1",
    )?;
    let feedback_rows = stmt
        .query_map(
            params![SESSION_FEEDBACK_LIMIT, SESSION_FEEDBACK_MAX_AGE_DAYS],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    classify_listen(
                        row.get::<_, bool>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ),
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut feedback_tracks = Vec::new();
    let mut feedback_entries = Vec::new();

    // Batch load all feedback tracks in a single query instead of N individual SELECTs.
    let feedback_track_ids: Vec<i64> = feedback_rows.iter().map(|(id, _)| *id).collect();
    let found_tracks = queue::get_tracks_by_ids(conn, &feedback_track_ids)?;
    let track_map: HashMap<i64, &Track> = found_tracks.iter().map(|t| (t.id, t)).collect();

    for (track_id, outcome) in feedback_rows {
        profile.recent_track_ids.insert(track_id);
        // Partial plays only mark the track as recent; they carry no taste.
        let completed = match outcome {
            ListenOutcome::Completed => true,
            ListenOutcome::EarlySkip => {
                profile.skipped_track_ids.insert(track_id);
                false
            }
            ListenOutcome::Partial => continue,
        };

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

    // Videos you watched properly count toward an artist too, a little less
    // than listens do. A failure here only drops the video part.
    if let Err(error) = add_video_watch_affinity(conn, &mut profile) {
        tracing::debug!(%error, "taste: video watches skipped");
    }

    Ok(profile)
}

/// Days of video watches that count toward taste.
const VIDEO_TASTE_DAYS: f64 = 30.0;
/// Most any one artist can gain from videos, so a binge of one artist's
/// clips cannot outweigh what you actually listen to.
const VIDEO_TASTE_ARTIST_CAP: f64 = 3.0;

/// Adds artists whose videos you watched properly (finished, or 70% watched:
/// the crawler's "enjoyed" rule) in the last 30 days: 1.2 for a watch today,
/// fading to 0.4 at 30 days, at most 3.0 per artist. Recent listens add about
/// 1 to 11, so videos nudge the profile rather than lead it. Only artists in
/// your library count (taste is keyed by library artist).
fn add_video_watch_affinity(conn: &Connection, profile: &mut SessionTasteProfile) -> Result<()> {
    use crate::services::video_discovery::roots;
    let mut stmt = conn.prepare(
        "SELECT a.id, h.duration_watched_ms, h.video_duration_ms, h.completed,
                julianday('now') - julianday(h.started_at)
           FROM video_history h
           JOIN artists a ON a.tidal_id = h.artist_tidal_id
          WHERE h.artist_tidal_id > 0
            AND h.duration_watched_ms IS NOT NULL
            AND h.started_at >= datetime('now', '-30 days')",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, i64>(3)? != 0,
            row.get::<_, f64>(4)?,
        ))
    })?;
    let mut gained = HashMap::<i64, f64>::new();
    for row in rows {
        let (artist_id, watched_ms, duration_ms, completed, age_days) = row?;
        let watch = roots::WatchRow {
            artist_id,
            age_days,
            watched_ms,
            duration_ms,
            completed,
        };
        if !roots::is_enjoyed(&watch) {
            continue;
        }
        let freshness = (1.0 - age_days.max(0.0) / VIDEO_TASTE_DAYS).clamp(0.0, 1.0);
        let entry = gained.entry(artist_id).or_insert(0.0);
        *entry = (*entry + 0.4 + 0.8 * freshness).min(VIDEO_TASTE_ARTIST_CAP);
    }
    for (artist_id, weight) in gained {
        *profile.positive_artists.entry(artist_id).or_insert(0.0) += weight;
    }
    Ok(())
}

pub(super) fn normalize_genre_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests;

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
    //! The frozen pre-TasteVector scorer and its top-30 parity gate lived here.
    //! The automix scorer was deliberately reworked (relevance first, weighted
    //! Jaccard genres, tamed harmonic, penalties last), so the gate is gone;
    //! the characterization test below still pins timestamp parsing.
    use crate::playback::automix::parse_days_since_last_played;

    // Characterization test: parse_days_since_last_played returns f64::MAX on
    // parse failure, and the 14-day recency penalty only applies when
    // days_since < 14.0, so malformed timestamps keep their score. This pins
    // that behavior so a future cleanup returning 0.0 would be caught.
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
        assert!((0.0..365.0).contains(&recent));
    }
}
