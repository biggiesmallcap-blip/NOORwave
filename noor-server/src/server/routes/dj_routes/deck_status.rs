//! Per-deck DJ status: profile readiness, waveform, drop preview and active transition.

use super::*;

pub(super) fn deck_status(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
    label_override: Option<&(String, Option<String>)>,
    rebuild_inflight: bool,
) -> anyhow::Result<DjDeckStatus> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let correction = queries::get_audio_dj_profile_correction(conn, &key)?;
    let rebuild_key = dj_profile_inflight_key(&key);
    let updating_analysis = profile.as_ref().is_some_and(|row| {
        // A cached profile still needs its waveform, regardless of provenance.
        // Do not let that cache clear a running rebuild or its retry budget.
        decode_f32_blob(&row.waveform_peaks_blob).is_none_or(|peaks| peaks.is_empty())
            || (row.source.starts_with("dj_playback") && !dj_profile_row_is_current(row))
    });
    let known_failure = recent_dj_profile_rebuild_failure(&rebuild_key);
    let source_unavailable = known_failure
        .as_ref()
        .is_some_and(|failure| failure.status == "source_unavailable");
    let rebuild_failure = if profile.is_some() && !updating_analysis && !source_unavailable {
        clear_dj_profile_rebuild_failure(&rebuild_key);
        None
    } else {
        known_failure
    };
    let profile_status = if source_unavailable {
        "source_unavailable".to_string()
    } else if profile.is_some() && !updating_analysis {
        "ready".to_string()
    } else if let Some(failure) = rebuild_failure.as_ref() {
        failure.status.clone()
    } else if rebuild_inflight {
        "analyzing".to_string()
    } else if profile.is_some() {
        "ready".to_string()
    } else {
        "missing".to_string()
    };
    let profile_retry_after_ms = rebuild_failure
        .as_ref()
        .and_then(profile_rebuild_retry_after_ms);
    let profile_retry_reason = rebuild_failure
        .as_ref()
        .and_then(|failure| failure.retry_reason.clone());
    let profile_error = rebuild_failure.map(|failure| failure.message);
    let (title, artist) = match label_override {
        Some((title, artist)) => (title.clone(), artist.clone()),
        None => media_ref_label(conn, media_ref)?,
    };
    let passive_analysis = media_ref
        .track_id()
        .and_then(crate::services::audio_analysis::queue_prescanner::prescan_status_for_track);
    let (
        beat_count,
        downbeat_count,
        phrase_count,
        profile_confidence,
        waveform_peaks,
        beat_markers_ms,
        downbeat_markers_ms,
        phrase_markers_ms,
        drop_markers_ms,
        mix_in_markers_ms,
        mix_out_markers_ms,
    ) = if let Some(profile) = profile.as_ref() {
        let beat_markers = decode_f32_blob(&profile.beat_grid_blob).unwrap_or_default();
        let downbeat_markers = decode_f32_blob(&profile.downbeats_blob).unwrap_or_default();
        let phrase_markers = phrase_markers_ms(
            &decode_u32_blob(&profile.phrase_boundaries_blob).unwrap_or_default(),
            &downbeat_markers,
        );
        (
            Some(beat_markers.len()),
            Some(downbeat_markers.len()),
            decode_u32_blob(&profile.phrase_boundaries_blob).map(|values| values.len()),
            Some(profile.profile_confidence),
            capped_waveform_peaks(&profile.waveform_peaks_blob),
            seconds_markers_ms(&beat_markers),
            seconds_markers_ms(&downbeat_markers),
            phrase_markers,
            seconds_markers_ms(&decode_f32_blob(&profile.drop_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.mix_in_blob).unwrap_or_default()),
            seconds_markers_ms(&decode_f32_blob(&profile.mix_out_blob).unwrap_or_default()),
        )
    } else {
        (
            None,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    };
    let manual_drop_markers_ms = correction
        .as_ref()
        .map(|row| decode_marker_blob_ms(&row.manual_drop_blob))
        .unwrap_or_default();
    let waveform_status = waveform_status(&profile_status, &waveform_peaks);
    let energy = media_ref
        .track_id()
        .or_else(|| profile.as_ref().and_then(|row| row.track_id))
        .map(|track_id| queries::get_audio_dsp_features(conn, track_id))
        .transpose()?
        .flatten()
        .and_then(|features| features.energy)
        .or_else(|| {
            profile
                .as_ref()
                .and_then(|row| row.lufs_loud_body)
                .map(crate::services::audio_analysis::features::energy_from_db)
        });
    Ok(DjDeckStatus {
        media_ref_kind: key.media_ref_kind,
        media_ref_id: key.media_ref_id,
        title,
        artist,
        profile_ready: profile.is_some(),
        profile_status,
        profile_error,
        profile_retry_after_ms,
        profile_retry_reason,
        profile_confidence,
        beat_confidence: profile.as_ref().and_then(|row| row.beat_confidence),
        grid_is_synthetic: profile.as_ref().is_some_and(|row| {
            crate::playback::dj_engine::dj_grid_is_synthetic(
                row,
                &decode_f32_blob(&row.beat_grid_blob).unwrap_or_default(),
            )
        }),
        analysis_scope_ms: profile.as_ref().map(|row| row.analysis_scope_ms),
        energy,
        beat_count,
        downbeat_count,
        phrase_count,
        waveform_status,
        waveform_peaks,
        beat_markers_ms,
        downbeat_markers_ms,
        phrase_markers_ms,
        drop_markers_ms,
        manual_drop_markers_ms,
        mix_in_markers_ms,
        mix_out_markers_ms,
        passive_analysis_status: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.status.to_string()),
        passive_analysis_reason: passive_analysis
            .as_ref()
            .map(|snapshot| snapshot.reason.to_string()),
        safe_crossfade_only: correction
            .as_ref()
            .is_some_and(|row| row.safe_crossfade_only),
    })
}

pub(super) fn capped_waveform_peaks(blob: &[u8]) -> Vec<f32> {
    decode_f32_blob(blob)
        .unwrap_or_default()
        .into_iter()
        .take(DJ_WAVEFORM_PEAK_COUNT)
        .map(|peak| peak.clamp(0.0, 1.0))
        .collect()
}

pub(super) fn active_transition_for_runtime(
    conn: &Connection,
    session: Option<&player::ActiveListenSession>,
    current: Option<&DjMediaRef>,
    elapsed_ms: Option<i64>,
) -> anyhow::Result<Option<DjActiveTransition>> {
    // The installed overlap's output clock is the evidence of live mixing.
    // Pause flushes listening history, and resume starts a new listen session;
    // neither action removes the audio already installed in the buffer.
    let Some(elapsed_ms) = elapsed_ms else {
        return Ok(None);
    };
    let session_id = session
        .filter(|session| session.transition_visual_valid)
        .and_then(|session| session.dj_transition_event_id);
    let latest_id = if let Some(current) = current {
        let key = current.profile_key();
        conn.query_row("SELECT id FROM dj_transition_events
            WHERE to_media_ref_kind = ?1 AND to_media_ref_id = ?2
              AND actual_start_ms IS NOT NULL AND runtime_rendered_dj_mixer = 1
              AND runtime_renderer_status = 'rendered_handoff' AND timing_status IN ('fired', 'late')
            ORDER BY id DESC LIMIT 1",
            params![key.media_ref_kind, key.media_ref_id], |row| row.get::<_, i64>(0)).optional()?
    } else {
        None
    };
    // Started can open the listening session before promotion timing is
    // persisted. Its remembered event may then belong to an earlier mix of
    // this track. The installed overlap clock still supplies the live proof;
    // prefer a newer confirmed execution for this same incoming track.
    let event_id = match (session_id, latest_id) {
        (Some(session), Some(latest)) => Some(session.max(latest)),
        (session, latest) => session.or(latest),
    };
    event_id
        .map(|id| active_transition_for_event(conn, id, elapsed_ms))
        .transpose()
        .map(Option::flatten)
}

pub(super) fn active_transition_for_event(
    conn: &Connection,
    id: i64,
    position_ms: i64,
) -> anyhow::Result<Option<DjActiveTransition>> {
    let event = conn
        .query_row(
            "SELECT from_media_ref_kind, from_media_ref_id, to_media_ref_kind, to_media_ref_id,
                program_json, COALESCE(runtime_planned_start_ms, planned_start_ms, actual_start_ms), actual_start_ms
         FROM dj_transition_events WHERE id = ?1 AND runtime_rendered_dj_mixer = 1
           AND actual_start_ms IS NOT NULL
           AND runtime_renderer_status = 'rendered_handoff' AND timing_status IN ('fired', 'late')",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((from_kind, from_id, to_kind, to_id, json, start_ms, actual_start_ms)) = event else {
        return Ok(None);
    };
    let Ok(program) = serde_json::from_str::<noor_mix::TransitionProgram>(&json) else {
        return Ok(None);
    };
    let duration_ms =
        program.resolve_at.saturating_mul(1000) / u64::from(program.sample_rate.max(1));
    if position_ms < 0 || position_ms as u64 >= duration_ms {
        return Ok(None);
    }
    let media_ref = |kind: &str, id: &str| {
        let id = id.parse::<i64>().ok()?;
        match kind {
            "library_track" => Some(DjMediaRef::LibraryTrack { track_id: id }),
            "tidal_track" => Some(DjMediaRef::TidalTrack {
                tidal_id: id,
                track_id: None,
            }),
            _ => None,
        }
    };
    let (Some(from), Some(to)) = (media_ref(&from_kind, &from_id), media_ref(&to_kind, &to_id))
    else {
        return Ok(None);
    };
    Ok(Some(DjActiveTransition {
        event_id: id,
        outgoing: deck_status(conn, &from, None, false)?,
        incoming: deck_status(conn, &to, None, false)?,
        program,
        start_ms,
        actual_start_ms,
        elapsed_ms: position_ms,
    }))
}

pub(super) fn waveform_status(profile_status: &str, peaks: &[f32]) -> String {
    if !peaks.is_empty() {
        "ready".to_string()
    } else if profile_status == "analyzing" || profile_status == "retrying" {
        "analyzing".to_string()
    } else {
        "missing".to_string()
    }
}

pub(super) fn seconds_markers_ms(values: &[f32]) -> Vec<i64> {
    values
        .iter()
        .filter(|value| value.is_finite() && **value >= 0.0)
        .map(|value| (*value as f64 * 1000.0).round() as i64)
        .collect()
}

pub(super) fn phrase_markers_ms(phrases: &[u32], downbeats: &[f32]) -> Vec<i64> {
    phrases
        .iter()
        .filter_map(|index| downbeats.get(*index as usize))
        .filter(|value| value.is_finite() && **value >= 0.0)
        .map(|value| (*value as f64 * 1000.0).round() as i64)
        .collect()
}

pub(super) fn drop_preview_status(
    conn: &rusqlite::Connection,
    enabled: bool,
    current_ref: Option<&DjMediaRef>,
    next_ref: Option<&DjMediaRef>,
    current: Option<&DjDeckStatus>,
    next: Option<&DjDeckStatus>,
    current_duration_ms: Option<i64>,
    actual_fire_ms: Option<i64>,
) -> anyhow::Result<DjDropPreviewStatus> {
    let skipped = |reason: &str| DjDropPreviewStatus {
        status: "skipped".to_string(),
        planned_fire_ms: None,
        actual_fire_ms: None,
        incoming_drop_ms: incoming_drop_marker(next).map(|marker| marker.0),
        source: incoming_drop_marker(next).map(|marker| marker.1.to_string()),
        reason: Some(reason.to_string()),
    };

    if !enabled {
        return Ok(skipped("disabled"));
    }
    let (Some(current_ref), Some(next_ref), Some(current), Some(next)) =
        (current_ref, next_ref, current, next)
    else {
        return Ok(skipped("pair_missing"));
    };
    if current.profile_status == "source_unavailable" {
        return Ok(skipped("current_source_unavailable"));
    }
    if next.profile_status == "source_unavailable" {
        return Ok(skipped("next_source_unavailable"));
    }
    if !current.profile_ready {
        return Ok(skipped(&deck_profile_unavailable_reason(
            "current", current,
        )));
    }
    if !next.profile_ready {
        return Ok(skipped(&deck_profile_unavailable_reason("next", next)));
    }
    if current.safe_crossfade_only || next.safe_crossfade_only {
        return Ok(skipped("safe_crossfade_only"));
    }
    if current
        .profile_confidence
        .is_some_and(|value| value < DJ_PROFILE_CONFIDENCE_FLOOR)
        || next
            .profile_confidence
            .is_some_and(|value| value < DJ_PROFILE_CONFIDENCE_FLOOR)
    {
        return Ok(skipped("profile_low_confidence"));
    }
    if !drop_preview_pair_harmonic_compatible(conn, current_ref, next_ref)? {
        return Ok(skipped("harmonic_incompatible"));
    }
    let Some((incoming_drop_ms, source)) = incoming_drop_marker(Some(next)) else {
        return Ok(skipped("missing_incoming_drop"));
    };
    let Some(planned_fire_ms) = select_drop_preview_fire_ms(current, current_duration_ms) else {
        return Ok(DjDropPreviewStatus {
            status: "skipped".to_string(),
            planned_fire_ms: None,
            actual_fire_ms: None,
            incoming_drop_ms: Some(incoming_drop_ms),
            source: Some(source.to_string()),
            reason: Some("no_safe_mid_song_marker".to_string()),
        });
    };
    Ok(DjDropPreviewStatus {
        status: if actual_fire_ms.is_some() {
            "fired".to_string()
        } else {
            "armed".to_string()
        },
        planned_fire_ms: Some(planned_fire_ms),
        actual_fire_ms,
        incoming_drop_ms: Some(incoming_drop_ms),
        source: Some(source.to_string()),
        reason: None,
    })
}

pub(super) fn deck_profile_unavailable_reason(prefix: &str, deck: &DjDeckStatus) -> String {
    if deck.profile_status == "retrying" {
        if let Some(reason) = deck.profile_retry_reason.as_deref() {
            return format!("{prefix}_profile_retrying_{reason}");
        }
        return format!("{prefix}_profile_retrying");
    }
    format!("{prefix}_profile_missing")
}

pub(crate) fn drop_preview_plan_for_pair(
    conn: &rusqlite::Connection,
    current_ref: &DjMediaRef,
    next_ref: &DjMediaRef,
    current_duration_ms: Option<i64>,
) -> anyhow::Result<Option<DropPreviewPlan>> {
    let enabled = queries::is_dj_engine_enabled(conn)?;
    let current = deck_status(conn, current_ref, None, false)?;
    let next = deck_status(conn, next_ref, None, false)?;
    let status = drop_preview_status(
        conn,
        enabled,
        Some(current_ref),
        Some(next_ref),
        Some(&current),
        Some(&next),
        current_duration_ms,
        None,
    )?;
    Ok(
        match (
            status.status.as_str(),
            status.planned_fire_ms,
            status.incoming_drop_ms,
            status.source,
        ) {
            ("armed", Some(planned_fire_ms), Some(incoming_drop_ms), Some(source)) => {
                Some(DropPreviewPlan {
                    planned_fire_ms,
                    incoming_drop_ms,
                    source,
                })
            }
            _ => None,
        },
    )
}

pub(super) fn incoming_drop_marker(next: Option<&DjDeckStatus>) -> Option<(i64, &'static str)> {
    let next = next?;
    next.manual_drop_markers_ms
        .iter()
        .copied()
        .find(|marker| *marker >= 0)
        .map(|marker| (marker, "manual"))
        .or_else(|| {
            next.drop_markers_ms
                .iter()
                .copied()
                .find(|marker| *marker >= 0)
                .map(|marker| (marker, "profile"))
        })
}

pub(super) fn select_drop_preview_fire_ms(
    current: &DjDeckStatus,
    duration_ms: Option<i64>,
) -> Option<i64> {
    let duration_ms = duration_ms.filter(|duration| *duration > 0)?;
    let min_ms = DROP_PREVIEW_MIN_POSITION_MS.max(duration_ms * 45 / 100);
    let max_ms = (duration_ms * 65 / 100)
        .min(duration_ms - DJ_READY_PAIR_TRANSITION_WINDOW_MS - DROP_PREVIEW_FINAL_WINDOW_GUARD_MS);
    if max_ms < min_ms {
        return None;
    }
    let target_ms = duration_ms * 55 / 100;
    current
        .phrase_markers_ms
        .iter()
        .chain(current.downbeat_markers_ms.iter())
        .copied()
        .filter(|marker| (min_ms..=max_ms).contains(marker))
        .min_by_key(|marker| (*marker - target_ms).abs())
}

pub(super) fn drop_preview_pair_harmonic_compatible(
    conn: &rusqlite::Connection,
    current_ref: &DjMediaRef,
    next_ref: &DjMediaRef,
) -> anyhow::Result<bool> {
    let current_key = media_ref_camelot_key(conn, current_ref)?;
    let next_key = media_ref_camelot_key(conn, next_ref)?;
    Ok(match (current_key.as_deref(), next_key.as_deref()) {
        (Some(current), Some(next)) => noor_mix::planner::scoring::camelot_distance(current, next)
            .is_some_and(|distance| matches!(distance, 0 | 1 | 7)),
        _ => false,
    })
}

pub(super) fn media_ref_camelot_key(
    conn: &rusqlite::Connection,
    media_ref: &DjMediaRef,
) -> anyhow::Result<Option<String>> {
    let key = media_ref.profile_key();
    let profile = queries::get_audio_dj_profile(conn, &key)?;
    let track_id = media_ref
        .track_id()
        .or_else(|| profile.as_ref().and_then(|profile| profile.track_id))
        .or_else(|| {
            media_ref
                .tidal_id()
                .and_then(|tidal_id| track_id_for_tidal_id(conn, tidal_id).ok().flatten())
        });
    let Some(track_id) = track_id else {
        return Ok(None);
    };
    Ok(queries::get_audio_dsp_features(conn, track_id)?.and_then(|features| features.camelot_key))
}

pub(super) fn track_id_for_tidal_id(
    conn: &rusqlite::Connection,
    tidal_id: i64,
) -> anyhow::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM tracks WHERE tidal_id = ?1 LIMIT 1",
        [tidal_id],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(anyhow::Error::from)
}
