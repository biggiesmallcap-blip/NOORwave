//! Audio DSP features, duplicate fingerprint groups, analysis quality and DJ profiles.

use super::*;

// ─── Audio DSP Features ─────────────────────────────────────────────────────

pub fn upsert_audio_dsp_features(conn: &Connection, f: &AudioDspFeatures) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dsp_features
         (track_id, bpm, key_signature, camelot_key, loudness_lufs, energy, danceability,
          beat_strength, spectral_centroid, stereo_width, is_instrumental,
          analysis_source, analysis_offset_ms, samples_analyzed, analyzed_at, analysis_version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
         ON CONFLICT(track_id) DO UPDATE SET
             bpm = excluded.bpm,
             key_signature = excluded.key_signature,
             camelot_key = excluded.camelot_key,
             loudness_lufs = excluded.loudness_lufs,
             energy = excluded.energy,
             danceability = excluded.danceability,
             beat_strength = excluded.beat_strength,
             spectral_centroid = excluded.spectral_centroid,
             stereo_width = excluded.stereo_width,
             is_instrumental = excluded.is_instrumental,
             analysis_source = excluded.analysis_source,
             analysis_offset_ms = excluded.analysis_offset_ms,
             samples_analyzed = excluded.samples_analyzed,
             analyzed_at = excluded.analyzed_at,
             analysis_version = excluded.analysis_version",
        params![
            f.track_id,
            f.bpm,
            f.key_signature,
            f.camelot_key,
            f.loudness_lufs,
            f.energy,
            f.danceability,
            f.beat_strength,
            f.spectral_centroid,
            f.stereo_width,
            f.is_instrumental as i32,
            f.analysis_source,
            f.analysis_offset_ms,
            f.samples_analyzed,
            f.analyzed_at,
            f.analysis_version,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dsp_features(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<AudioDspFeatures>> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, loudness_lufs,
                energy, danceability, beat_strength, spectral_centroid, stereo_width,
                is_instrumental, analysis_source, analysis_offset_ms, samples_analyzed,
                analyzed_at, analysis_version
         FROM audio_dsp_features
         WHERE track_id = ?1",
    )?;
    let result = stmt
        .query_row(params![track_id], |row| {
            Ok(AudioDspFeatures {
                track_id: row.get(0)?,
                bpm: row.get(1)?,
                key_signature: row.get(2)?,
                camelot_key: row.get(3)?,
                loudness_lufs: row.get(4)?,
                energy: row.get(5)?,
                danceability: row.get(6)?,
                beat_strength: row.get(7)?,
                spectral_centroid: row.get(8)?,
                stereo_width: row.get(9)?,
                is_instrumental: row.get::<_, i32>(10)? != 0,
                analysis_source: row.get(11)?,
                analysis_offset_ms: row.get(12)?,
                samples_analyzed: row.get(13)?,
                analyzed_at: row.get(14)?,
                analysis_version: row.get(15)?,
            })
        })
        .optional()?;
    Ok(result)
}

/// Batch-fetch just the harmonic inputs (camelot key + bpm) for many tracks in a
/// single query. The radio/automix re-ranker only needs these two fields; fetching
/// the full `AudioDspFeatures` row per candidate was N serialized single-row
/// queries under the DB mutex. Returns a map keyed by track_id; tracks with no
/// `audio_dsp_features` row are simply absent (callers treat that as unanalyzed).
pub fn get_dsp_harmonic_keys_batch(
    conn: &Connection,
    track_ids: &[i64],
) -> Result<std::collections::HashMap<i64, (Option<String>, Option<f64>)>> {
    let mut out: std::collections::HashMap<i64, (Option<String>, Option<f64>)> =
        std::collections::HashMap::new();
    if track_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = std::iter::repeat_n("?", track_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT track_id, camelot_key, bpm FROM audio_dsp_features WHERE track_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(track_ids.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<f64>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, camelot, bpm) = row?;
        out.insert(id, (camelot, bpm));
    }
    Ok(out)
}

pub fn get_tracks_missing_dsp_features(conn: &Connection, limit: i64) -> Result<Vec<Track>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         LEFT JOIN audio_dsp_features dsp ON t.id = dsp.track_id
         WHERE (dsp.track_id IS NULL OR dsp.analysis_version != '{}')
           AND COALESCE(dsp.manual_override, 0) = 0
         LIMIT ?1",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let tracks = stmt
        .query_map(params![limit], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tracks)
}

pub fn get_audio_features_stats(conn: &Connection) -> Result<AudioFeaturesStats> {
    let (total_analyzed, avg_bpm, avg_energy): (i64, Option<f64>, Option<f64>) = conn.query_row(
        "SELECT COUNT(*), AVG(bpm), AVG(energy) FROM audio_dsp_features",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;

    // Top key (most common)
    let top_key: Option<String> = conn
        .query_row(
            "SELECT key_signature
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC
         LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

    // Key distribution
    let mut stmt = conn.prepare(
        "SELECT key_signature, COUNT(*)
         FROM audio_dsp_features
         WHERE key_signature IS NOT NULL
         GROUP BY key_signature
         ORDER BY COUNT(*) DESC",
    )?;
    let key_rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let key_distribution: HashMap<String, i64> = key_rows
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .collect();

    Ok(AudioFeaturesStats {
        total_analyzed,
        avg_bpm,
        top_key,
        avg_energy,
        key_distribution,
    })
}

/// Bulk-load DSP features for every analyzed track. Used by smart playlist evaluation
/// so a single scan populates the evaluation context for all rules at once.
pub fn get_all_audio_dsp_features(
    conn: &Connection,
) -> Result<
    Vec<(
        i64,
        Option<f64>,
        Option<String>,
        Option<String>,
        Option<f64>,
        Option<f64>,
        bool,
    )>,
> {
    let mut stmt = conn.prepare(
        "SELECT track_id, bpm, key_signature, camelot_key, energy, danceability, is_instrumental
         FROM audio_dsp_features",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<f64>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<i32>>(6)?.unwrap_or(0) != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Track IDs that have a stored audio fingerprint.
pub fn get_track_ids_with_fingerprint(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare("SELECT track_id FROM audio_fingerprints")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn count_audio_dsp_features(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |row| {
        row.get(0)
    })
    .map_err(Into::into)
}

pub fn delete_all_audio_dsp_features(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM audio_dsp_features", [])?;
    Ok(())
}

pub fn get_genre_audio_metrics(conn: &Connection) -> Result<Vec<GenreAudioMetrics>> {
    let mut stmt = conn.prepare(
        "SELECT g.id, g.name,
                AVG(a.bpm) AS avg_bpm,
                AVG(a.energy) AS avg_energy,
                AVG(a.danceability) AS avg_danceability,
                COUNT(DISTINCT a.track_id) AS analyzed_count
         FROM genres g
         JOIN track_genres tg ON tg.genre_id = g.id
         JOIN audio_dsp_features a ON a.track_id = tg.track_id
         GROUP BY g.id, g.name
         ORDER BY analyzed_count DESC, g.name ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(GenreAudioMetrics {
            genre_id: row.get(0)?,
            genre_name: row.get(1)?,
            avg_bpm: row.get(2)?,
            avg_energy: row.get(3)?,
            avg_danceability: row.get(4)?,
            analyzed_count: row.get(5)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

#[allow(dead_code)]
pub fn upsert_fingerprint(conn: &Connection, track_id: i64, fp: &AudioFingerprint) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_fingerprints (track_id, hashes_blob, peak_count)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(track_id) DO UPDATE SET
             hashes_blob = excluded.hashes_blob,
             peak_count = excluded.peak_count",
        params![track_id, fp.hashes_blob, fp.peak_count],
    )?;
    Ok(())
}

#[allow(dead_code)]
pub fn insert_fingerprint_hashes(
    conn: &Connection,
    track_id: i64,
    hashes: &[(u32, u32)],
) -> Result<()> {
    if hashes.is_empty() {
        return Ok(());
    }

    // Wrap the whole payload in an explicit transaction; chunk inserts so a
    // very large hash list doesn't hold a single statement open for too long.
    const CHUNK: usize = 1000;
    conn.execute_batch("BEGIN;")?;
    let insert_result: Result<()> = (|| {
        let mut stmt = conn.prepare(
            "INSERT OR IGNORE INTO fingerprint_hashes (hash, track_id, time_offset)
             VALUES (?1, ?2, ?3)",
        )?;
        for chunk in hashes.chunks(CHUNK) {
            for (hash, time_offset) in chunk {
                stmt.execute(params![*hash as i64, track_id, *time_offset])?;
            }
        }
        Ok(())
    })();

    match insert_result {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

/// Run `PRAGMA optimize; ANALYZE fingerprint_hashes;` after a bulk fingerprint scan.
/// Failures are logged but not fatal.
pub fn optimize_fingerprint_hashes(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA optimize; ANALYZE fingerprint_hashes;")?;
    Ok(())
}

// ── Duplicate group helpers (fingerprint-driven dedup) ───────────────────────

/// Find an existing duplicate_group that already contains BOTH `a` and `b` as members.
pub fn find_duplicate_group_for_tracks(conn: &Connection, a: i64, b: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT ma.group_id
         FROM duplicate_members ma
         JOIN duplicate_members mb ON mb.group_id = ma.group_id
         WHERE ma.track_id = ?1 AND mb.track_id = ?2
         LIMIT 1",
        params![a, b],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Create a new empty duplicate_group and return its id.
pub fn create_duplicate_group(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO duplicate_groups (status) VALUES ('pending')",
        [],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a member into a duplicate_group. Idempotent (ON CONFLICT IGNORE).
pub fn add_duplicate_member(conn: &Connection, gid: i64, tid: i64, preferred: bool) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO duplicate_members (group_id, track_id, is_preferred)
         VALUES (?1, ?2, ?3)",
        params![gid, tid, if preferred { 1 } else { 0 }],
    )?;
    Ok(())
}

/// Tag a duplicate_group with its source (e.g. 'fingerprint') and a confidence value.
pub fn set_duplicate_group_source(
    conn: &Connection,
    gid: i64,
    source: &str,
    confidence: f64,
) -> Result<()> {
    conn.execute(
        "UPDATE duplicate_groups SET source = ?2, confidence = ?3 WHERE id = ?1",
        params![gid, source, confidence],
    )?;
    Ok(())
}

// ── Analysis quality & stale detection ───────────────────────────────────────

/// Snapshot of DSP-analysis coverage across the library.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioFeaturesQuality {
    pub total_tracks: i64,
    pub analyzed: i64,
    pub analysis_current: i64,
    pub analysis_stale: i64,
    pub low_confidence_bpm: i64,
    pub low_confidence_key: i64,
    pub no_preview_url: i64,
    pub fingerprinted: i64,
}

pub fn get_audio_features_quality(conn: &Connection) -> Result<AudioFeaturesQuality> {
    let total_tracks: i64 = conn
        .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))
        .unwrap_or(0);
    let analyzed: i64 = conn
        .query_row("SELECT COUNT(*) FROM audio_dsp_features", [], |r| r.get(0))
        .unwrap_or(0);
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analyzed_current_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version = '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_current: i64 = conn
        .query_row(&analyzed_current_sql, [], |r| r.get(0))
        .unwrap_or(0);
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let analysis_stale_sql = format!(
        "SELECT COUNT(*) FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let analysis_stale: i64 = conn
        .query_row(&analysis_stale_sql, [], |r| r.get(0))
        .unwrap_or(0);
    let low_confidence_bpm: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE bpm IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let low_confidence_key: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_dsp_features WHERE key_signature IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    // "No preview URL" = tracks we can't currently pull preview audio for.
    // We treat tracks lacking a tidal_id AND file_path as having no preview source.
    let no_preview_url: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracks
             WHERE tidal_id IS NULL
               AND (file_path IS NULL OR file_path = '')",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let fingerprinted: i64 = conn
        .query_row("SELECT COUNT(*) FROM audio_fingerprints", [], |r| r.get(0))
        .unwrap_or(0);

    Ok(AudioFeaturesQuality {
        total_tracks,
        analyzed,
        analysis_current,
        analysis_stale,
        low_confidence_bpm,
        low_confidence_key,
        no_preview_url,
        fingerprinted,
    })
}

/// Return the ids of all tracks whose stored analysis_version is not the current
/// `CURRENT_ANALYSIS_VERSION`. Used by the re-analyze admin endpoint.
pub fn get_stale_analysis_track_ids(conn: &Connection) -> Result<Vec<i64>> {
    // CURRENT_ANALYSIS_VERSION is a compile-time constant — safe to interpolate.
    let sql = format!(
        "SELECT track_id FROM audio_dsp_features WHERE analysis_version != '{}'",
        crate::services::audio_analysis::CURRENT_ANALYSIS_VERSION,
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

pub fn find_tracks_by_hash(conn: &Connection, hashes: &[u32]) -> Result<Vec<(i64, u32, u32)>> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = hashes.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT track_id, hash, time_offset
         FROM fingerprint_hashes
         WHERE hash IN ({})
         ORDER BY hash",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let hash_params: Vec<i64> = hashes.iter().map(|h| *h as i64).collect();
    let rows = stmt.query_map(params_from_iter(hash_params.iter()), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)? as u32,
            row.get::<_, i64>(2)? as u32,
        ))
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

pub fn upsert_audio_dj_profile(conn: &Connection, row: &AudioDjProfileRow) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profiles (
            media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id, profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, is_temporary, source, computed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
            ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28
        )
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            track_id = excluded.track_id,
            queue_item_id = excluded.queue_item_id,
            tidal_id = excluded.tidal_id,
            profile_version = excluded.profile_version,
            beat_grid_blob = excluded.beat_grid_blob,
            downbeats_blob = excluded.downbeats_blob,
            phrase_boundaries_blob = excluded.phrase_boundaries_blob,
            mix_in_blob = excluded.mix_in_blob,
            mix_out_blob = excluded.mix_out_blob,
            intro_end_seconds = excluded.intro_end_seconds,
            outro_start_seconds = excluded.outro_start_seconds,
            breakdown_blob = excluded.breakdown_blob,
            drop_blob = excluded.drop_blob,
            safe_transition_windows_blob = excluded.safe_transition_windows_blob,
            energy_contour_blob = excluded.energy_contour_blob,
            vocal_presence_blob = excluded.vocal_presence_blob,
            vocal_density_blob = excluded.vocal_density_blob,
            waveform_peaks_blob = excluded.waveform_peaks_blob,
            lufs_loud_body = excluded.lufs_loud_body,
            true_peak_dbtp = excluded.true_peak_dbtp,
            beat_confidence = excluded.beat_confidence,
            profile_confidence = excluded.profile_confidence,
            analysis_scope_ms = excluded.analysis_scope_ms,
            is_temporary = excluded.is_temporary,
            source = excluded.source,
            computed_at = excluded.computed_at",
        params![
            row.media_ref_kind,
            row.media_ref_id,
            row.track_id,
            row.queue_item_id,
            row.tidal_id,
            row.profile_version,
            row.beat_grid_blob,
            row.downbeats_blob,
            row.phrase_boundaries_blob,
            row.mix_in_blob,
            row.mix_out_blob,
            row.intro_end_seconds,
            row.outro_start_seconds,
            row.breakdown_blob,
            row.drop_blob,
            row.safe_transition_windows_blob,
            row.energy_contour_blob,
            row.vocal_presence_blob,
            row.vocal_density_blob,
            row.waveform_peaks_blob,
            row.lufs_loud_body,
            row.true_peak_dbtp,
            row.beat_confidence,
            row.profile_confidence,
            row.analysis_scope_ms,
            if row.is_temporary { 1 } else { 0 },
            row.source,
            row.computed_at,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dj_profile(
    conn: &Connection,
    key: &AudioDjProfileKey,
) -> Result<Option<AudioDjProfileRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id,
            profile_version, beat_grid_blob, downbeats_blob, phrase_boundaries_blob,
            mix_in_blob, mix_out_blob, intro_end_seconds, outro_start_seconds,
            breakdown_blob, drop_blob, safe_transition_windows_blob, energy_contour_blob,
            vocal_presence_blob, vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp,
            beat_confidence, profile_confidence, analysis_scope_ms, is_temporary,
            source, computed_at
         FROM audio_dj_profiles
         WHERE media_ref_kind = ?1 AND media_ref_id = ?2",
        params![key.media_ref_kind, key.media_ref_id],
        audio_dj_profile_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_audio_dj_profile_for_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<AudioDjProfileRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id,
            profile_version, beat_grid_blob, downbeats_blob, phrase_boundaries_blob,
            mix_in_blob, mix_out_blob, intro_end_seconds, outro_start_seconds,
            breakdown_blob, drop_blob, safe_transition_windows_blob, energy_contour_blob,
            vocal_presence_blob, vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp,
            beat_confidence, profile_confidence, analysis_scope_ms, is_temporary,
            source, computed_at
         FROM audio_dj_profiles
         WHERE track_id = ?1
         ORDER BY computed_at DESC
         LIMIT 1",
        params![track_id],
        audio_dj_profile_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn promote_temporary_audio_dj_profile(
    conn: &Connection,
    temporary_key: &AudioDjProfileKey,
    stable_key: &AudioDjProfileKey,
    tidal_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profiles (
            media_ref_kind, media_ref_id, track_id, queue_item_id, tidal_id, profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, is_temporary, source, computed_at
        )
        SELECT ?3, ?4, track_id, queue_item_id, COALESCE(?5, tidal_id), profile_version,
            beat_grid_blob, downbeats_blob, phrase_boundaries_blob, mix_in_blob, mix_out_blob,
            intro_end_seconds, outro_start_seconds, breakdown_blob, drop_blob,
            safe_transition_windows_blob, energy_contour_blob, vocal_presence_blob,
            vocal_density_blob, waveform_peaks_blob, lufs_loud_body, true_peak_dbtp, beat_confidence,
            profile_confidence, analysis_scope_ms, 0, source, datetime('now')
        FROM audio_dj_profiles
        WHERE media_ref_kind = ?1 AND media_ref_id = ?2
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            track_id = excluded.track_id,
            queue_item_id = excluded.queue_item_id,
            tidal_id = excluded.tidal_id,
            profile_version = excluded.profile_version,
            beat_grid_blob = excluded.beat_grid_blob,
            downbeats_blob = excluded.downbeats_blob,
            phrase_boundaries_blob = excluded.phrase_boundaries_blob,
            mix_in_blob = excluded.mix_in_blob,
            mix_out_blob = excluded.mix_out_blob,
            intro_end_seconds = excluded.intro_end_seconds,
            outro_start_seconds = excluded.outro_start_seconds,
            breakdown_blob = excluded.breakdown_blob,
            drop_blob = excluded.drop_blob,
            safe_transition_windows_blob = excluded.safe_transition_windows_blob,
            energy_contour_blob = excluded.energy_contour_blob,
            vocal_presence_blob = excluded.vocal_presence_blob,
            vocal_density_blob = excluded.vocal_density_blob,
            waveform_peaks_blob = excluded.waveform_peaks_blob,
            lufs_loud_body = excluded.lufs_loud_body,
            true_peak_dbtp = excluded.true_peak_dbtp,
            beat_confidence = excluded.beat_confidence,
            profile_confidence = excluded.profile_confidence,
            analysis_scope_ms = excluded.analysis_scope_ms,
            is_temporary = excluded.is_temporary,
            source = excluded.source,
            computed_at = excluded.computed_at",
        params![
            temporary_key.media_ref_kind,
            temporary_key.media_ref_id,
            stable_key.media_ref_kind,
            stable_key.media_ref_id,
            tidal_id,
        ],
    )?;
    Ok(())
}

pub fn upsert_audio_dj_profile_correction(
    conn: &Connection,
    row: &AudioDjProfileCorrectionRow,
) -> Result<()> {
    conn.execute(
        "INSERT INTO audio_dj_profile_corrections (
            media_ref_kind, media_ref_id, bpm_multiplier, downbeat_offset_beats,
            phrase_offset_bars, safe_crossfade_only, transition_speed_bias, notes,
            manual_drop_blob, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(media_ref_kind, media_ref_id) DO UPDATE SET
            bpm_multiplier = excluded.bpm_multiplier,
            downbeat_offset_beats = excluded.downbeat_offset_beats,
            phrase_offset_bars = excluded.phrase_offset_bars,
            safe_crossfade_only = excluded.safe_crossfade_only,
            transition_speed_bias = excluded.transition_speed_bias,
            notes = excluded.notes,
            manual_drop_blob = excluded.manual_drop_blob,
            updated_at = excluded.updated_at",
        params![
            row.media_ref_kind,
            row.media_ref_id,
            row.bpm_multiplier,
            row.downbeat_offset_beats,
            row.phrase_offset_bars,
            if row.safe_crossfade_only { 1 } else { 0 },
            row.transition_speed_bias,
            row.notes,
            row.manual_drop_blob,
            row.created_at,
            row.updated_at,
        ],
    )?;
    Ok(())
}

pub fn get_audio_dj_profile_correction(
    conn: &Connection,
    key: &AudioDjProfileKey,
) -> Result<Option<AudioDjProfileCorrectionRow>> {
    conn.query_row(
        "SELECT media_ref_kind, media_ref_id, bpm_multiplier, downbeat_offset_beats,
            phrase_offset_bars, safe_crossfade_only, transition_speed_bias, notes,
            manual_drop_blob, created_at, updated_at
         FROM audio_dj_profile_corrections
         WHERE media_ref_kind = ?1 AND media_ref_id = ?2",
        params![key.media_ref_kind, key.media_ref_id],
        |row| {
            Ok(AudioDjProfileCorrectionRow {
                media_ref_kind: row.get(0)?,
                media_ref_id: row.get(1)?,
                bpm_multiplier: row.get(2)?,
                downbeat_offset_beats: row.get(3)?,
                phrase_offset_bars: row.get(4)?,
                safe_crossfade_only: row.get::<_, i64>(5)? != 0,
                transition_speed_bias: row.get(6)?,
                notes: row.get(7)?,
                manual_drop_blob: row.get(8)?,
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn is_dj_engine_enabled(conn: &Connection) -> Result<bool> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_engine_enabled'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(value.as_deref(), Some("1") | Some("true")))
}

pub fn set_dj_engine_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_engine_enabled', ?1)",
        params![if enabled { "1" } else { "0" }],
    )?;
    Ok(())
}

pub fn get_dj_global_policy(conn: &Connection) -> Result<(String, String)> {
    let mix_intent = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_mix_intent'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "balanced".to_string());
    let transition_speed_bias = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_transition_speed_bias'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "neutral".to_string());
    Ok((mix_intent, transition_speed_bias))
}

pub fn set_dj_global_policy(
    conn: &Connection,
    mix_intent: &str,
    transition_speed_bias: &str,
) -> Result<()> {
    if !matches!(mix_intent, "safe" | "balanced" | "bold") {
        bail!("unknown DJ mix intent: {mix_intent}");
    }
    if !matches!(transition_speed_bias, "slower" | "neutral" | "faster") {
        bail!("unknown DJ transition speed bias: {transition_speed_bias}");
    }
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_mix_intent', ?1)",
        params![mix_intent],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value)
         VALUES ('dj_transition_speed_bias', ?1)",
        params![transition_speed_bias],
    )?;
    Ok(())
}

pub fn get_dj_preferred_strategy(conn: &Connection) -> Result<String> {
    Ok(conn
        .query_row(
            "SELECT value FROM server_config WHERE key = 'dj_preferred_strategy'",
            [],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or_else(|| "adaptive".to_string()))
}

pub fn set_dj_preferred_strategy(conn: &Connection, strategy: &str) -> Result<()> {
    if !matches!(
        strategy,
        "adaptive"
            | "wildcard"
            | "smooth_blend"
            | "club_mix"
            | "quick_mix"
            | "energy_lift"
            | "energy_reset"
            | "drop_swap"
            | "bass_swap"
            | "cut"
    ) {
        bail!("unknown DJ strategy: {strategy}");
    }
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('dj_preferred_strategy', ?1)",
        [strategy],
    )?;
    Ok(())
}

pub fn record_dj_feedback(
    conn: &Connection,
    id: i64,
    category: &str,
    reason: Option<&str>,
) -> Result<bool> {
    let rating = match category {
        "good" => 1,
        "bad" => -1,
        "too_safe" | "too_bold" => 0,
        _ => bail!("unknown DJ feedback"),
    };
    let transaction = conn.unchecked_transaction()?;
    let changed = transaction.execute(
        "UPDATE dj_transition_events SET user_rating = ?1
         WHERE id = ?2 AND actual_start_ms IS NOT NULL
           AND timing_status IN ('fired', 'late')
           AND runtime_renderer_status IN ('rendered_handoff', 'rendered_overlay', 'legacy_overlap')",
        params![rating, id],
    )?;
    if changed == 0 {
        return Ok(false);
    }
    // Keep feedback categories separate from planner alternatives and playback
    // outcomes, using the existing config table without a schema migration.
    transaction.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
        params![format!("dj_feedback:{id}"), category],
    )?;
    if let Some(reason) = reason {
        transaction.execute(
            "INSERT OR REPLACE INTO server_config (key, value) VALUES (?1, ?2)",
            params![format!("dj_feedback_reason:{id}"), reason],
        )?;
    }
    transaction.commit()?;
    Ok(true)
}

pub(super) fn validate_dj_fallback_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason
        && !matches!(
            reason,
            "disabled"
                | "current_profile_missing"
                | "next_profile_missing"
                | "profile_low_confidence"
                | "next_not_resolved"
                | "fetch_failed"
                | "decode_late"
                | "analysis_late"
                | "program_invalid"
                | "queue_changed"
                | "safety_override_safe"
                | "template_not_renderable"
                | "timing_unstable"
        )
    {
        bail!("unknown DJ fallback reason: {reason}");
    }
    Ok(())
}

pub(super) fn validate_dj_timing_source(source: Option<&str>) -> Result<()> {
    if let Some(source) = source
        && !matches!(
            source,
            "downbeat_sync" | "beat_sync" | "phrase_sync" | "mix_out_sync" | "fallback_overlap"
        )
    {
        bail!("unknown DJ timing source: {source}");
    }
    Ok(())
}

pub(super) fn validate_dj_timing_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status
        && !matches!(status, "armed" | "fired" | "late" | "missed")
    {
        bail!("unknown DJ timing status: {status}");
    }
    Ok(())
}

pub(super) fn validate_dj_runtime_renderer_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status
        && !matches!(
            status,
            "rendered_handoff" | "rendered_overlay" | "legacy_overlap" | "boundary_fallback"
        )
    {
        bail!("unknown DJ runtime renderer status: {status}");
    }
    Ok(())
}

pub(super) fn validate_dj_runtime_renderer_reason(reason: Option<&str>) -> Result<()> {
    if let Some(reason) = reason
        && !matches!(
            reason,
            "none"
                | "prepared_mixer_missing"
                | "lookahead_pair_mismatch"
                | "program_not_mixer_renderable"
                | "active_deck_not_decoded"
                | "next_deck_not_decoded"
                | "mixer_rejected"
                | "active_track_changed"
                | "next_track_changed"
                | "render_buffer_failed"
                | "buffer_lock_failed"
                | "dj_disabled"
                | "next_decode_late_at_fire"
                | "next_deck_missing_at_fire"
                | "transition_plan_missing_at_fire"
                | "sync_window_not_signaled"
                | "manual_seek_suppressed"
        )
    {
        bail!("unknown DJ runtime renderer reason: {reason}");
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_dj_transition_event(
    conn: &Connection,
    from_track_id: Option<i64>,
    to_track_id: Option<i64>,
    from_media_ref_kind: Option<&str>,
    from_media_ref_id: Option<&str>,
    to_media_ref_kind: Option<&str>,
    to_media_ref_id: Option<&str>,
    template: &str,
    program_json: &str,
    rejected_alternatives_json: Option<&str>,
    planner_version: &str,
    fallback_reason: Option<&str>,
    planned_start_ms: Option<i64>,
    timing_source: Option<&str>,
    timing_status: Option<&str>,
) -> Result<i64> {
    validate_dj_fallback_reason(fallback_reason)?;
    validate_dj_timing_source(timing_source)?;
    validate_dj_timing_status(timing_status)?;
    conn.execute(
        "INSERT INTO dj_transition_events (
            from_track_id, to_track_id, from_media_ref_kind, from_media_ref_id,
            to_media_ref_kind, to_media_ref_id, template, program_json,
            rejected_alternatives_json, planner_version, fallback_reason,
            planned_start_ms, timing_source, timing_status
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            from_track_id,
            to_track_id,
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
            template,
            program_json,
            rejected_alternatives_json,
            planner_version,
            fallback_reason,
            planned_start_ms,
            timing_source,
            timing_status,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update_dj_transition_outcome(
    conn: &Connection,
    id: i64,
    outcome: &str,
    skip_within_30s: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE dj_transition_events
         SET outcome = ?2,
             outcome_at = datetime('now'),
             skip_within_30s = ?3
         WHERE id = ?1",
        params![id, outcome, if skip_within_30s { 1 } else { 0 }],
    )?;
    Ok(())
}

pub fn replace_armed_dj_transition_event(
    conn: &Connection,
    id: i64,
    template: &str,
    program_json: &str,
    rejected_alternatives_json: Option<&str>,
    planner_version: &str,
    fallback_reason: Option<&str>,
    planned_start_ms: Option<i64>,
    timing_source: Option<&str>,
) -> Result<()> {
    validate_dj_fallback_reason(fallback_reason)?;
    validate_dj_timing_source(timing_source)?;
    conn.execute(
        "UPDATE dj_transition_events
         SET template = ?2,
             program_json = ?3,
             rejected_alternatives_json = ?4,
             planner_version = ?5,
             fallback_reason = ?6,
             planned_start_ms = ?7,
             runtime_planned_start_ms = NULL,
             actual_start_ms = NULL,
             timing_delta_ms = NULL,
             timing_source = ?8,
             runtime_rendered_dj_mixer = NULL,
             runtime_renderer_status = NULL,
             runtime_renderer_reason = NULL
         WHERE id = ?1
           AND timing_status = 'armed'
           AND actual_start_ms IS NULL
           AND outcome IS NULL",
        params![
            id,
            template,
            program_json,
            rejected_alternatives_json,
            planner_version,
            fallback_reason,
            planned_start_ms,
            timing_source,
        ],
    )?;
    Ok(())
}

#[cfg(test)]
pub fn update_dj_transition_fire_timing(
    conn: &Connection,
    id: i64,
    actual_start_ms: i64,
    timing_status: &str,
    runtime_rendered_dj_mixer: bool,
    runtime_renderer_status: &str,
    runtime_renderer_reason: &str,
) -> Result<()> {
    update_dj_transition_fire_timing_with_runtime_target(
        conn,
        id,
        actual_start_ms,
        None,
        timing_status,
        runtime_rendered_dj_mixer,
        runtime_renderer_status,
        runtime_renderer_reason,
    )
}

pub fn update_dj_transition_fire_timing_with_runtime_target(
    conn: &Connection,
    id: i64,
    actual_start_ms: i64,
    runtime_planned_start_ms: Option<i64>,
    timing_status: &str,
    runtime_rendered_dj_mixer: bool,
    runtime_renderer_status: &str,
    runtime_renderer_reason: &str,
) -> Result<()> {
    validate_dj_timing_status(Some(timing_status))?;
    validate_dj_runtime_renderer_status(Some(runtime_renderer_status))?;
    validate_dj_runtime_renderer_reason(Some(runtime_renderer_reason))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET actual_start_ms = CASE
                 WHEN ?3 = 'missed' THEN NULL
                 ELSE ?2
             END,
             runtime_planned_start_ms = CASE
                 WHEN ?3 = 'missed' THEN NULL
                 ELSE ?7
             END,
             timing_delta_ms = CASE
                 WHEN ?3 = 'missed' THEN NULL
                 WHEN COALESCE(?7, planned_start_ms) IS NULL THEN NULL
                 ELSE ?2 - COALESCE(?7, planned_start_ms)
             END,
             timing_status = ?3,
             runtime_rendered_dj_mixer = ?4,
             runtime_renderer_status = ?5,
             runtime_renderer_reason = ?6
         WHERE id = ?1",
        params![
            id,
            actual_start_ms,
            timing_status,
            if runtime_rendered_dj_mixer { 1 } else { 0 },
            runtime_renderer_status,
            runtime_renderer_reason,
            runtime_planned_start_ms,
        ],
    )?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = 'missed'
         WHERE id <> ?1
           AND timing_status = 'armed'
           AND EXISTS (
               SELECT 1
               FROM dj_transition_events fired
               WHERE fired.id = ?1
                 AND fired.from_media_ref_kind IS dj_transition_events.from_media_ref_kind
                 AND fired.from_media_ref_id IS dj_transition_events.from_media_ref_id
                 AND fired.to_media_ref_kind IS dj_transition_events.to_media_ref_kind
                 AND fired.to_media_ref_id IS dj_transition_events.to_media_ref_id
           )",
        params![id],
    )?;
    Ok(())
}

pub fn mark_dj_transition_timing_status_for_pair(
    conn: &Connection,
    from_media_ref_kind: &str,
    from_media_ref_id: &str,
    to_media_ref_kind: &str,
    to_media_ref_id: &str,
    timing_status: &str,
) -> Result<usize> {
    validate_dj_timing_status(Some(timing_status))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = ?5
         WHERE id = (
             SELECT id
             FROM dj_transition_events
             WHERE from_media_ref_kind = ?1
               AND from_media_ref_id = ?2
               AND to_media_ref_kind = ?3
               AND to_media_ref_id = ?4
               AND outcome IS NULL
               AND timing_status = 'armed'
               AND NOT EXISTS (
                   SELECT 1
                   FROM dj_transition_events fired
                   WHERE fired.from_media_ref_kind IS dj_transition_events.from_media_ref_kind
                      AND fired.from_media_ref_id IS dj_transition_events.from_media_ref_id
                      AND fired.to_media_ref_kind IS dj_transition_events.to_media_ref_kind
                      AND fired.to_media_ref_id IS dj_transition_events.to_media_ref_id
                      AND fired.id > dj_transition_events.id
                      AND fired.timing_status = 'fired'
                )
              ORDER BY started_at DESC, id DESC
              LIMIT 1
         )",
        params![
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
            timing_status,
        ],
    )
    .map_err(Into::into)
}

pub fn mark_dj_transition_manual_seek_suppressed_for_pair(
    conn: &Connection,
    from_media_ref_kind: &str,
    from_media_ref_id: &str,
    to_media_ref_kind: &str,
    to_media_ref_id: &str,
) -> Result<usize> {
    validate_dj_timing_status(Some("missed"))?;
    validate_dj_runtime_renderer_status(Some("boundary_fallback"))?;
    validate_dj_runtime_renderer_reason(Some("manual_seek_suppressed"))?;
    conn.execute(
        "UPDATE dj_transition_events
         SET timing_status = 'missed',
             outcome = COALESCE(outcome, 'manual_seek_suppressed'),
             outcome_at = COALESCE(outcome_at, datetime('now')),
             runtime_rendered_dj_mixer = 0,
             runtime_renderer_status = 'boundary_fallback',
             runtime_renderer_reason = 'manual_seek_suppressed'
         WHERE id = (
             SELECT id
             FROM dj_transition_events
             WHERE from_media_ref_kind = ?1
               AND from_media_ref_id = ?2
               AND to_media_ref_kind = ?3
               AND to_media_ref_id = ?4
               AND outcome IS NULL
               AND timing_status = 'armed'
              ORDER BY started_at DESC, id DESC
              LIMIT 1
         )",
        params![
            from_media_ref_kind,
            from_media_ref_id,
            to_media_ref_kind,
            to_media_ref_id,
        ],
    )
    .map_err(Into::into)
}

pub fn count_recent_bad_dj_feedback_for_ref(
    conn: &Connection,
    key: &AudioDjProfileKey,
    limit: i64,
) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM (
            SELECT user_rating
            FROM dj_transition_events
            WHERE user_rating IS NOT NULL
              AND (
                (from_media_ref_kind = ?1 AND from_media_ref_id = ?2)
                OR (to_media_ref_kind = ?1 AND to_media_ref_id = ?2)
              )
            ORDER BY started_at DESC, id DESC
            LIMIT ?3
         )
         WHERE user_rating < 0",
        params![key.media_ref_kind, key.media_ref_id, limit.max(0)],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub(super) fn audio_dj_profile_from_row(row: &Row<'_>) -> rusqlite::Result<AudioDjProfileRow> {
    Ok(AudioDjProfileRow {
        media_ref_kind: row.get(0)?,
        media_ref_id: row.get(1)?,
        track_id: row.get(2)?,
        queue_item_id: row.get(3)?,
        tidal_id: row.get(4)?,
        profile_version: row.get(5)?,
        beat_grid_blob: row.get(6)?,
        downbeats_blob: row.get(7)?,
        phrase_boundaries_blob: row.get(8)?,
        mix_in_blob: row.get(9)?,
        mix_out_blob: row.get(10)?,
        intro_end_seconds: row.get(11)?,
        outro_start_seconds: row.get(12)?,
        breakdown_blob: row.get(13)?,
        drop_blob: row.get(14)?,
        safe_transition_windows_blob: row.get(15)?,
        energy_contour_blob: row.get(16)?,
        vocal_presence_blob: row.get(17)?,
        vocal_density_blob: row.get(18)?,
        waveform_peaks_blob: row.get(19)?,
        lufs_loud_body: row.get(20)?,
        true_peak_dbtp: row.get(21)?,
        beat_confidence: row.get(22)?,
        profile_confidence: row.get(23)?,
        analysis_scope_ms: row.get(24)?,
        is_temporary: row.get::<_, i64>(25)? != 0,
        source: row.get(26)?,
        computed_at: row.get(27)?,
    })
}

/// Load enough metadata about a library track to seed external Tidal discovery.
/// Returns None if the track id isn't found.
///
/// `provider_track_id` is set from `tracks.tidal_id` if available; otherwise the
/// library `id` is used as a string. `normalized_genres` is the top 5 genres
/// for the track ordered by descending confidence.
pub fn load_external_seed_from_track(
    conn: &Connection,
    track_id: i64,
) -> Result<Option<DiscoveryCandidateSeed>> {
    let row = conn.query_row(
        "SELECT t.id, t.tidal_id, t.title, ar.name, al.title
         FROM tracks t
         LEFT JOIN artists ar ON t.artist_id = ar.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.id = ?1",
        params![track_id],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    );

    let (id, tidal_id, title, artist_name, album_title) = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM track_genres tg
         JOIN genres g ON g.id = tg.genre_id
         WHERE tg.track_id = ?1
         ORDER BY COALESCE(tg.confidence, 0) DESC
         LIMIT 5",
    )?;
    let genres: Vec<String> = stmt
        .query_map(params![track_id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Some(DiscoveryCandidateSeed {
        provider_track_id: tidal_id
            .map(|t| t.to_string())
            .unwrap_or_else(|| id.to_string()),
        title,
        artist_name,
        album_title,
        normalized_genres: genres,
    }))
}
