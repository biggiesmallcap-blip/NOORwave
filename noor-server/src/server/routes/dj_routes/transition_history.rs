//! Transition timing history, renderer status and overlay details for the DJ status view.

use super::*;

pub(super) fn latest_open_transition_for_pair(
    conn: &rusqlite::Connection,
    current: Option<&DjMediaRef>,
    next: Option<&DjMediaRef>,
) -> anyhow::Result<Option<OpenTransition>> {
    let (Some(current), Some(next)) = (current, next) else {
        return Ok(None);
    };
    let current_key = current.profile_key();
    let next_key = next.profile_key();
    conn.query_row(
        "SELECT id, template, program_json, fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, rejected_alternatives_json,
                runtime_rendered_dj_mixer, runtime_renderer_status, runtime_renderer_reason
         FROM dj_transition_events
         WHERE from_media_ref_kind = ?1
           AND from_media_ref_id = ?2
           AND to_media_ref_kind = ?3
           AND to_media_ref_id = ?4
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
            let program_json: String = row.get(2)?;
            let planned_start_ms: Option<i64> = row.get(4)?;
            let timing_status: Option<String> = row.get(8)?;
            Ok(OpenTransition {
                id: row.get(0)?,
                template: row.get(1)?,
                renderer_template: renderer_template_from_program_json(&program_json),
                fallback_reason: row.get(3)?,
                planned_start_ms,
                actual_start_ms: row.get(5)?,
                timing_delta_ms: row.get(6)?,
                timing_source: row.get(7)?,
                timing_status: timing_status.clone(),
                overlay_details: overlay_details_from_program_json(
                    &program_json,
                    planned_start_ms,
                    timing_status.as_deref(),
                ),
                runtime_rendered_dj_mixer: row.get::<_, Option<i64>>(10)?.map(|value| value != 0),
                runtime_renderer_status: row.get(11)?,
                runtime_renderer_reason: row.get(12)?,
                rejected_alternatives: decode_rejected_alternatives(row.get(9)?),
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub(super) fn latest_dj_transition_timing_history(
    conn: &rusqlite::Connection,
    limit: i64,
) -> anyhow::Result<Vec<DjTimingHistoryEvent>> {
    let mut stmt = conn.prepare(
        "SELECT e.id,
                COALESCE(from_track.title, from_tidal_track.title, from_queue_track.title, from_queue.pending_title, e.from_media_ref_kind || ':' || e.from_media_ref_id),
                COALESCE(from_artist.name, from_tidal_artist.name, from_queue_artist.name, from_queue.pending_artist),
                COALESCE(to_track.title, to_tidal_track.title, to_queue_track.title, to_queue.pending_title, e.to_media_ref_kind || ':' || e.to_media_ref_id),
                COALESCE(to_artist.name, to_tidal_artist.name, to_queue_artist.name, to_queue.pending_artist),
                e.template, e.program_json, e.fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, e.started_at, e.rejected_alternatives_json,
                e.runtime_rendered_dj_mixer, e.runtime_renderer_status, e.runtime_renderer_reason, e.runtime_planned_start_ms
         FROM dj_transition_events e
         LEFT JOIN tracks from_track ON from_track.id = e.from_track_id
         LEFT JOIN artists from_artist ON from_artist.id = from_track.artist_id
         LEFT JOIN tracks to_track ON to_track.id = e.to_track_id
         LEFT JOIN artists to_artist ON to_artist.id = to_track.artist_id
         LEFT JOIN tracks from_tidal_track
           ON e.from_media_ref_kind = 'tidal_track'
          AND from_tidal_track.tidal_id = CAST(e.from_media_ref_id AS INTEGER)
         LEFT JOIN artists from_tidal_artist ON from_tidal_artist.id = from_tidal_track.artist_id
         LEFT JOIN tracks to_tidal_track
           ON e.to_media_ref_kind = 'tidal_track'
          AND to_tidal_track.tidal_id = CAST(e.to_media_ref_id AS INTEGER)
         LEFT JOIN artists to_tidal_artist ON to_tidal_artist.id = to_tidal_track.artist_id
         LEFT JOIN queue from_queue
           ON e.from_media_ref_kind = 'queue_item'
          AND from_queue.id = CAST(e.from_media_ref_id AS INTEGER)
         LEFT JOIN tracks from_queue_track ON from_queue_track.id = from_queue.track_id
         LEFT JOIN artists from_queue_artist ON from_queue_artist.id = from_queue_track.artist_id
         LEFT JOIN queue to_queue
           ON e.to_media_ref_kind = 'queue_item'
          AND to_queue.id = CAST(e.to_media_ref_id AS INTEGER)
         LEFT JOIN tracks to_queue_track ON to_queue_track.id = to_queue.track_id
         LEFT JOIN artists to_queue_artist ON to_queue_artist.id = to_queue_track.artist_id
         WHERE e.timing_status IN ('fired', 'late', 'missed')
           AND COALESCE(e.runtime_renderer_reason, '') <> 'manual_seek_suppressed'
           AND NOT (
             e.timing_status = 'missed'
             AND EXISTS (
               SELECT 1
               FROM dj_transition_events fired
               WHERE fired.from_media_ref_kind IS e.from_media_ref_kind
                  AND fired.from_media_ref_id IS e.from_media_ref_id
                  AND fired.to_media_ref_kind IS e.to_media_ref_kind
                  AND fired.to_media_ref_id IS e.to_media_ref_id
                  AND fired.id > e.id
                  AND fired.timing_status = 'fired'
              )
            )
         ORDER BY e.started_at DESC, e.id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit.max(0)], |row| {
        let program_json: String = row.get(6)?;
        let timing_delta_ms: Option<i64> = row.get(10)?;
        let timing_status: Option<String> = row.get(12)?;
        let rejected_json: Option<String> = row.get(14)?;
        let runtime_rendered_dj_mixer = row.get::<_, Option<i64>>(15)?.map(|value| value != 0);
        Ok(DjTimingHistoryEvent {
            event_id: row.get(0)?,
            from_title: row.get(1)?,
            from_artist: row.get(2)?,
            to_title: row.get(3)?,
            to_artist: row.get(4)?,
            planned_template: row.get(5)?,
            renderer_template: renderer_template_from_program_json(&program_json),
            planning_reason: row.get(7)?,
            planned_start_ms: row.get(8)?,
            runtime_planned_start_ms: row.get(18)?,
            actual_start_ms: row.get(9)?,
            timing_delta_ms,
            timing_source: row.get(11)?,
            timing_status: timing_status.clone(),
            timing_quality: timing_quality(timing_status.as_deref(), timing_delta_ms).to_string(),
            timing_direction: timing_direction(timing_status.as_deref(), timing_delta_ms)
                .to_string(),
            runtime_rendered_dj_mixer,
            runtime_renderer_status: row.get(16)?,
            runtime_renderer_reason: row.get(17)?,
            started_at: row.get(13)?,
            rejected_alternatives: decode_rejected_alternatives(rejected_json),
        })
    })?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

pub(super) fn timing_quality(
    timing_status: Option<&str>,
    timing_delta_ms: Option<i64>,
) -> &'static str {
    if timing_status == Some("missed") {
        return "bad";
    }
    if timing_status == Some("armed") {
        return "pending";
    }
    let Some(delta_ms) = timing_delta_ms else {
        return "bad";
    };
    match delta_ms.abs() {
        0..=150 => "tight",
        151..=500 => "usable",
        501..=1000 => "loose",
        _ => "bad",
    }
}

pub(super) fn timing_direction(
    timing_status: Option<&str>,
    timing_delta_ms: Option<i64>,
) -> &'static str {
    match timing_status {
        Some("missed") => "missed",
        Some("armed") => "pending",
        Some("late") => "late",
        Some("fired") => match timing_delta_ms {
            Some(delta_ms) if delta_ms < -150 => "early",
            Some(delta_ms) if delta_ms > 150 => "late",
            Some(_) => "on_time",
            None => "unknown",
        },
        _ => "unknown",
    }
}

pub(super) fn decode_rejected_alternatives(json: Option<String>) -> Vec<DjRejectedAlternative> {
    let Some(json) = json else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<DjRejectedAlternative>>(&json).unwrap_or_default()
}

pub(super) fn latest_fired_dj_timing_deltas(
    conn: &rusqlite::Connection,
    limit: i64,
) -> anyhow::Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT timing_delta_ms
         FROM dj_transition_events
         WHERE timing_status = 'fired'
           AND timing_delta_ms IS NOT NULL
           AND ABS(timing_delta_ms) <= ?2
           AND template != 'DropPreview16'
         ORDER BY started_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map(
        params![limit.max(0), DJ_TIMING_SANITY_MAX_DELTA_MS],
        |row| row.get::<_, i64>(0),
    )?;
    let mut deltas = Vec::new();
    for row in rows {
        deltas.push(row?);
    }
    Ok(deltas)
}

pub(super) fn summarize_timing_history(
    events: &[DjTimingHistoryEvent],
    tuning_deltas: &[i64],
) -> DjTimingHistorySummary {
    let mut delta_sum = 0i64;
    let mut abs_delta_sum = 0i64;
    let mut delta_count = 0i64;
    let mut tight_count = 0usize;
    let mut usable_count = 0usize;
    let mut loose_count = 0usize;
    let mut bad_count = 0usize;
    let mut late_count = 0usize;
    let mut missed_count = 0usize;

    for event in events {
        match event.timing_quality.as_str() {
            "tight" => tight_count += 1,
            "usable" => usable_count += 1,
            "loose" => loose_count += 1,
            _ => bad_count += 1,
        }
        if event.timing_status.as_deref() == Some("late") {
            late_count += 1;
        }
        if event.timing_status.as_deref() == Some("missed") {
            missed_count += 1;
        }
        if let Some(delta_ms) = event
            .timing_delta_ms
            .filter(|delta| timing_delta_is_sane(*delta))
        {
            delta_sum += delta_ms;
            abs_delta_sum += delta_ms.abs();
            delta_count += 1;
        }
    }

    DjTimingHistorySummary {
        event_count: events.len(),
        average_delta_ms: if delta_count > 0 {
            Some(delta_sum / delta_count)
        } else {
            None
        },
        average_abs_delta_ms: if delta_count > 0 {
            Some(abs_delta_sum / delta_count)
        } else {
            None
        },
        median_abs_delta_ms: median_abs_delta(tuning_deltas),
        worst_abs_delta_ms: worst_abs_delta(tuning_deltas),
        tight_count,
        usable_count,
        loose_count,
        bad_count,
        late_count,
        missed_count,
    }
}

pub(super) fn timing_delta_is_sane(delta_ms: i64) -> bool {
    delta_ms.abs() <= DJ_TIMING_SANITY_MAX_DELTA_MS
}

pub(super) fn median_abs_delta(deltas: &[i64]) -> Option<i64> {
    if deltas.is_empty() {
        return None;
    }
    let mut abs_values = deltas.iter().map(|delta| delta.abs()).collect::<Vec<_>>();
    abs_values.sort_unstable();
    let middle = abs_values.len() / 2;
    if abs_values.len() % 2 == 0 {
        Some((abs_values[middle - 1] + abs_values[middle]) / 2)
    } else {
        Some(abs_values[middle])
    }
}

pub(super) fn worst_abs_delta(deltas: &[i64]) -> Option<i64> {
    deltas.iter().map(|delta| delta.abs()).max()
}

#[allow(dead_code)]
pub(super) fn fire_ahead_evidence_passes(deltas: &[i64]) -> bool {
    if deltas.len() < 20 {
        return false;
    }
    let positive_count = deltas.iter().filter(|delta| **delta > 0).count();
    positive_count * 10 >= deltas.len() * 7 && median_delta(deltas).is_some_and(|delta| delta > 150)
}

pub(super) fn median_delta(deltas: &[i64]) -> Option<i64> {
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

#[cfg(test)]
pub(super) fn open_transition_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<OpenTransition> {
    let program_json: String = row.get(2)?;
    let planned_start_ms: Option<i64> = row.get(4)?;
    let timing_status: Option<String> = row.get(8)?;
    Ok(OpenTransition {
        id: row.get(0)?,
        template: row.get(1)?,
        renderer_template: renderer_template_from_program_json(&program_json),
        fallback_reason: row.get(3)?,
        planned_start_ms,
        actual_start_ms: row.get(5)?,
        timing_delta_ms: row.get(6)?,
        timing_source: row.get(7)?,
        timing_status: timing_status.clone(),
        overlay_details: overlay_details_from_program_json(
            &program_json,
            planned_start_ms,
            timing_status.as_deref(),
        ),
        runtime_rendered_dj_mixer: row.get::<_, Option<i64>>(9)?.map(|value| value != 0),
        runtime_renderer_status: row.get(10)?,
        runtime_renderer_reason: row.get(11)?,
        rejected_alternatives: Vec::new(),
    })
}

#[cfg(test)]
pub(super) fn latest_completed_timing_transition(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Option<OpenTransition>> {
    conn.query_row(
        "SELECT id, template, program_json, fallback_reason,
                planned_start_ms, actual_start_ms, timing_delta_ms,
                timing_source, timing_status, runtime_rendered_dj_mixer,
                runtime_renderer_status, runtime_renderer_reason
         FROM dj_transition_events
         WHERE timing_status IN ('fired', 'late', 'missed')
         ORDER BY started_at DESC, id DESC
         LIMIT 1",
        [],
        open_transition_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub(super) fn renderer_status_for_transition(
    transition: Option<&OpenTransition>,
) -> RendererStatus {
    let Some(transition) = transition else {
        return RendererStatus {
            planned_template: None,
            renderer_template: None,
            renderer_mode: None,
            downgrade_reason: None,
            planning_reason: None,
            sync_target: None,
            planned_start_ms: None,
            actual_start_ms: None,
            timing_delta_ms: None,
            timing_source: None,
            timing_status: None,
            timing_quality: "unknown".to_string(),
            timing_direction: "unknown".to_string(),
            runtime_rendered_dj_mixer: None,
            runtime_renderer_status: None,
            runtime_renderer_reason: None,
            overlay_details: None,
            rejected_alternatives: Vec::new(),
        };
    };
    let quality = timing_quality(
        transition.timing_status.as_deref(),
        transition.timing_delta_ms,
    )
    .to_string();
    let direction = timing_direction(
        transition.timing_status.as_deref(),
        transition.timing_delta_ms,
    )
    .to_string();
    if transition
        .renderer_template
        .as_deref()
        .is_some_and(is_renderable_template)
    {
        let downgrade_reason = renderer_downgrade_reason(transition);
        return RendererStatus {
            planned_template: Some(transition.template.clone()),
            renderer_template: transition.renderer_template.clone(),
            renderer_mode: Some(
                if transition.renderer_template.as_deref() == Some("DropTease16") {
                    "dj_overlay_program"
                } else {
                    "dj_gain_program"
                }
                .to_string(),
            ),
            downgrade_reason,
            planning_reason: planning_reason_without_renderer_downgrade(transition),
            sync_target: transition.timing_source.clone(),
            planned_start_ms: transition.planned_start_ms,
            actual_start_ms: transition.actual_start_ms,
            timing_delta_ms: transition.timing_delta_ms,
            timing_source: transition.timing_source.clone(),
            timing_status: transition.timing_status.clone(),
            timing_quality: quality,
            timing_direction: direction,
            runtime_rendered_dj_mixer: transition.runtime_rendered_dj_mixer,
            runtime_renderer_status: transition.runtime_renderer_status.clone(),
            runtime_renderer_reason: transition.runtime_renderer_reason.clone(),
            overlay_details: if transition.renderer_template.as_deref() == Some("DropTease16") {
                transition.overlay_details.clone()
            } else {
                None
            },
            rejected_alternatives: transition.rejected_alternatives.clone(),
        };
    }
    RendererStatus {
        planned_template: Some(transition.template.clone()),
        renderer_template: None,
        renderer_mode: Some("legacy_overlap".to_string()),
        planning_reason: transition.fallback_reason.clone(),
        downgrade_reason: Some(
            if transition.template == "SafeCrossfade" {
                "dj_program_renderer_pending"
            } else {
                transition
                    .fallback_reason
                    .as_deref()
                    .filter(|reason| is_renderer_downgrade_reason(reason))
                    .unwrap_or("template_not_renderable")
            }
            .to_string(),
        ),
        sync_target: transition.timing_source.clone(),
        planned_start_ms: transition.planned_start_ms,
        actual_start_ms: transition.actual_start_ms,
        timing_delta_ms: transition.timing_delta_ms,
        timing_source: transition.timing_source.clone(),
        timing_status: transition.timing_status.clone(),
        timing_quality: quality,
        timing_direction: direction,
        runtime_rendered_dj_mixer: transition.runtime_rendered_dj_mixer,
        runtime_renderer_status: transition.runtime_renderer_status.clone(),
        runtime_renderer_reason: transition.runtime_renderer_reason.clone(),
        overlay_details: None,
        rejected_alternatives: transition.rejected_alternatives.clone(),
    }
}

pub(super) fn renderer_downgrade_reason(transition: &OpenTransition) -> Option<String> {
    if transition.renderer_template.as_deref() == Some(transition.template.as_str()) {
        return None;
    }
    Some(
        transition
            .fallback_reason
            .as_deref()
            .filter(|reason| is_renderer_downgrade_reason(reason))
            .unwrap_or("template_not_renderable")
            .to_string(),
    )
}

pub(super) fn planning_reason_without_renderer_downgrade(
    transition: &OpenTransition,
) -> Option<String> {
    transition
        .fallback_reason
        .as_deref()
        .filter(|reason| !is_renderer_downgrade_reason(reason))
        .map(str::to_string)
}

pub(super) fn is_renderer_downgrade_reason(reason: &str) -> bool {
    matches!(
        reason,
        "template_not_renderable"
            | "timing_unstable"
            | "overlay_not_handoff"
            | "beat_sync_unverified"
    )
}

pub(super) fn is_renderable_template(template: &str) -> bool {
    matches!(
        template,
        "SafeCrossfade"
            | "FilterSweep"
            | "BassSwap16"
            | "BassSwap32"
            | "SlamCut"
            | "LongHarmonicBlend"
            | "DropTease16"
            | "ClubMix"
            | "QuickMix"
            | "EnergyLift"
            | "EnergyReset"
            | "DropSwap"
    )
}

pub(super) fn renderer_template_from_program_json(program_json: &str) -> Option<String> {
    let program: noor_mix::TransitionProgram = serde_json::from_str(program_json).ok()?;
    is_renderable_template(program.template.as_str()).then_some(program.template)
}

pub(super) fn overlay_details_from_program_json(
    program_json: &str,
    planned_start_ms: Option<i64>,
    timing_status: Option<&str>,
) -> Option<DjOverlayDetails> {
    let program: noor_mix::TransitionProgram = serde_json::from_str(program_json).ok()?;
    if program.template != "DropTease16" || program.sample_rate == 0 {
        return None;
    }
    let resolve_ms = ((u128::from(program.resolve_at) * 1_000)
        + (u128::from(program.sample_rate) / 2))
        / u128::from(program.sample_rate);
    let overlay_end_ms = planned_start_ms
        .zip(i64::try_from(resolve_ms).ok())
        .and_then(|(start_ms, duration_ms)| start_ms.checked_add(duration_ms));
    let drop_marker_ms = ((u128::from(
        program
            .deck_b_start_frame
            .saturating_add(program.swap_start),
    ) * 1_000)
        + (u128::from(program.sample_rate) / 2))
        / u128::from(program.sample_rate);
    let tempo_ratio = program
        .automation
        .iter()
        .find(|event| event.param == noor_mix::Param::PlaybackRate(noor_mix::DeckId::B))
        .and_then(|event| event.to.is_finite().then_some(f64::from(event.to)));
    Some(DjOverlayDetails {
        overlay_status: timing_status.unwrap_or("armed").to_string(),
        overlay_start_ms: planned_start_ms,
        overlay_end_ms,
        tempo_ratio,
        deck_b_start_frame: program.deck_b_start_frame,
        drop_marker_ms: i64::try_from(drop_marker_ms).ok(),
        drop_source: program
            .drop_source
            .unwrap_or_else(|| "program_json".to_string()),
    })
}

pub(super) fn annotate_overlay_drop_source(
    details: Option<DjOverlayDetails>,
    incoming: Option<&DjDeckStatus>,
) -> Option<DjOverlayDetails> {
    let mut details = details?;
    if details.drop_source != "program_json" {
        return Some(details);
    }
    let Some(drop_marker_ms) = details.drop_marker_ms else {
        return Some(details);
    };
    let Some(incoming) = incoming else {
        return Some(details);
    };
    if marker_matches(&incoming.manual_drop_markers_ms, drop_marker_ms) {
        details.drop_source = "manual_drop_cue".to_string();
    } else if marker_matches(&incoming.drop_markers_ms, drop_marker_ms) {
        details.drop_source = "profile_drop_candidate".to_string();
    }
    Some(details)
}

pub(super) fn marker_matches(markers_ms: &[i64], target_ms: i64) -> bool {
    markers_ms
        .iter()
        .any(|marker| marker.abs_diff(target_ms) <= 25)
}

pub(super) fn media_ref_label(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
) -> anyhow::Result<(String, Option<String>)> {
    if let Some(track_id) = media_ref.track_id()
        && let Some(row) = conn
            .query_row(
                "SELECT t.title, ar.name
                 FROM tracks t
                 LEFT JOIN artists ar ON ar.id = t.artist_id
                 WHERE t.id = ?1",
                params![track_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
    {
        return Ok(row);
    }
    // Executed events retain provider identity after queue promotion. They
    // need not retain the local track id to display the original pair.
    if let DjMediaRef::TidalTrack { tidal_id, .. } = media_ref
        && let Some(row) = conn
            .query_row(
                "SELECT t.title, ar.name FROM tracks t
                 LEFT JOIN artists ar ON ar.id = t.artist_id
                 WHERE t.tidal_id = ?1 ORDER BY t.id LIMIT 1",
                params![tidal_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?
    {
        return Ok(row);
    }
    match media_ref {
        DjMediaRef::PendingQueueItem {
            pending_artist,
            pending_title,
            ..
        } => Ok((pending_title.clone(), Some(pending_artist.clone()))),
        _ => Ok((media_ref.profile_key().media_ref_id, None)),
    }
}

pub(super) fn safe_crossfade_suggestion(
    conn: &rusqlite::Connection,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
) -> anyhow::Result<Option<DjSafeSuggestion>> {
    for deck in [current, next].into_iter().flatten() {
        let key = AudioDjProfileKey {
            media_ref_kind: deck.media_ref_kind.clone(),
            media_ref_id: deck.media_ref_id.clone(),
        };
        let bad_feedback_count =
            queries::count_recent_bad_dj_feedback_for_ref(conn, &key, SAFE_SUGGESTION_BAD_COUNT)?;
        if bad_feedback_count >= SAFE_SUGGESTION_BAD_COUNT {
            return Ok(Some(DjSafeSuggestion {
                media_ref_kind: key.media_ref_kind,
                media_ref_id: key.media_ref_id,
                bad_feedback_count,
            }));
        }
    }
    Ok(None)
}

pub(super) fn feedback_rating(value: &str) -> Option<i64> {
    match value {
        "good" => Some(1),
        "bad" => Some(-1),
        "too_safe" | "too_bold" => Some(0),
        _ => None,
    }
}
