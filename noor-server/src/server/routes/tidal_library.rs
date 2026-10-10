//! Writing TIDAL tracks into the library: upsert, favorite flags, and source genres.

use super::*;

/// Upsert a TIDAL track (and its artist) and return the local `tracks.id`.
/// The id is looked up here anyway to attach source genres, so callers that
/// need it should use the return value rather than issuing a second SELECT.
pub(in crate::server) fn insert_tidal_track(
    conn: &rusqlite::Connection,
    track: &crate::services::tidal::client::TidalTrack,
    is_favorite: bool,
    is_library: bool,
    favorite_created: Option<&str>,
) -> anyhow::Result<Option<i64>> {
    let catalogue = crate::db::catalogue::enabled(conn)?;
    if catalogue {
        let known = crate::db::catalogue::track_id(conn, track.id)?;
        let matched = if known.is_some() {
            known
        } else {
            let incoming = crate::library::duplicates::IncomingTrack {
                tidal_id: track.id,
                title: &track.title,
                artist_name: &track.artist.name,
                isrc: track.isrc.as_deref(),
                duration_ms: track.duration * 1000,
                version: track.extra.get("version").and_then(|v| v.as_str()),
                explicit: track.extra.get("explicit").and_then(|v| v.as_bool()),
            };
            let candidates = crate::library::duplicates::fetch_import_candidates(
                conn,
                track.id,
                track.artist.id,
                track.isrc.as_deref(),
                incoming.duration_ms,
            )?;
            match crate::library::duplicates::decide_import(&incoming, &candidates) {
                crate::library::duplicates::ImportDecision::LinkAlias {
                    existing_track_id, ..
                } => Some(existing_track_id),
                _ => None,
            }
        };
        if let Some(id) = matched {
            crate::db::catalogue::record_track(conn, id, track, is_favorite, favorite_created)?;
            crate::db::catalogue::curate(conn, id, is_favorite, is_library, favorite_created)?;
            queries::replace_track_source_genres(
                conn,
                id,
                &infer_tidal_track_genres(track),
                "tidal",
                0.82,
            )?;
            return Ok(Some(id));
        }
    }
    // Ensure artist exists first (tracks.artist_id is NOT NULL)
    conn.execute(
        "INSERT INTO artists (tidal_id, name) VALUES (?1, ?2)
         ON CONFLICT(tidal_id) DO UPDATE SET name=excluded.name",
        rusqlite::params![track.artist.id, track.artist.name],
    )?;

    let quality = track.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        "LOW" => 200,
        _ => 500,
    };
    let album_tidal_id = track.album.as_ref().map(|a| a.id);

    conn.execute(
        // Curated write paths (liked tracks, playlist tracks) pass
        // is_library=1; discovery-enrichment fill from favorited albums
        // passes is_library=0 so it stays out of the Library grid and Genre
        // Galaxy while still feeding radio/similarity. The ON CONFLICT MAX
        // self-heals: a row first seen as background is promoted to library
        // when a curated write touches it, and is never demoted. See
        // MIGRATION_052.
        "INSERT INTO tracks (tidal_id, title, artist_id, album_id, disc_number, track_number, duration_ms, isrc, best_quality, best_source, fidelity_score, is_favorite, source, date_added, is_library)
         VALUES (?1, ?2, (SELECT id FROM artists WHERE tidal_id=?3), (SELECT id FROM albums WHERE tidal_id=?4), ?5, ?6, ?7, ?8, ?9, 'tidal', ?10, ?11, 'tidal', COALESCE(?12, datetime('now')), ?13)
         ON CONFLICT(tidal_id) DO UPDATE SET
            title=excluded.title, best_quality=excluded.best_quality,
            fidelity_score=MAX(tracks.fidelity_score, excluded.fidelity_score),
            is_favorite=MAX(tracks.is_favorite, excluded.is_favorite),
            is_library=MAX(tracks.is_library, excluded.is_library),
            date_added=CASE
                WHEN ?11 = 1 AND ?12 IS NOT NULL AND tracks.is_library=0 AND tracks.is_favorite=0 THEN excluded.date_added
                ELSE tracks.date_added
            END",
        rusqlite::params![
            track.id, track.title, track.artist.id, album_tidal_id,
            track.volume_number.unwrap_or(1), track.track_number,
            track.duration * 1000, track.isrc,
            quality, fidelity, is_favorite as i32, favorite_created,
            is_library as i32,
        ],
    )?;

    let local_track_id: Option<i64> = conn
        .query_row(
            "SELECT id FROM tracks WHERE tidal_id = ?1",
            rusqlite::params![track.id],
            |row| row.get(0),
        )
        .ok();
    if let Some(local_track_id) = local_track_id {
        if catalogue {
            crate::db::catalogue::record_track(
                conn,
                local_track_id,
                track,
                is_favorite,
                favorite_created,
            )?;
            crate::db::catalogue::curate(
                conn,
                local_track_id,
                is_favorite,
                is_library,
                favorite_created,
            )?;
        }
        let canonical_genres = infer_tidal_track_genres(track);
        queries::replace_track_source_genres(
            conn,
            local_track_id,
            &canonical_genres,
            "tidal",
            0.82,
        )?;
    }

    Ok(local_track_id)
}

#[cfg(test)]
pub(in crate::server) fn apply_tidal_favorite_flags(
    conn: &rusqlite::Connection,
    table: &str,
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
) -> anyhow::Result<()> {
    apply_tidal_favorite_flags_at(
        conn,
        table,
        favorite_ids,
        prev_count,
        &crate::db::catalogue_favorites::now(),
    )
}

pub(in crate::server) fn apply_tidal_favorite_flags_at(
    conn: &rusqlite::Connection,
    table: &str,
    favorite_ids: &HashSet<i64>,
    prev_count: i64,
    snapshot_started: &str,
) -> anyhow::Result<()> {
    // Refuse to wipe favorites if this run somehow returned zero items but the
    // previous run had a real population, almost always a transient TIDAL API
    // hiccup, not a legitimate "user unfavorited everything".
    if favorite_ids.is_empty() && prev_count > 0 {
        anyhow::bail!(
            "Refusing to clear is_favorite on '{}': sync returned 0 favorites but previous run had {}",
            table,
            prev_count
        );
    }

    if crate::db::catalogue::enabled(conn)? {
        let tx = conn.unchecked_transaction()?;
        crate::db::catalogue::reconcile_favorites_at(
            &tx,
            favorite_ids,
            table == "albums",
            snapshot_started,
        )?;
        tx.commit()?;
        return Ok(());
    }

    // Scope the reset to TIDAL-sourced rows so manually-imported albums/tracks
    // (e.g. from `import_tidal_album`) keep whatever favorite state they had:
    // they aren't "TIDAL favorites" in the strict sync sense.
    let reset_sql = format!(
        "UPDATE {table} SET is_favorite = 0 WHERE source = 'tidal' AND tidal_id IS NOT NULL"
    );
    conn.execute(&reset_sql, [])?;

    let mut sorted_ids: Vec<i64> = favorite_ids.iter().copied().collect();
    sorted_ids.sort_unstable();

    for chunk in sorted_ids.chunks(800) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("UPDATE {table} SET is_favorite = 1 WHERE tidal_id IN ({placeholders})");
        conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }

    Ok(())
}

pub(super) fn infer_tidal_track_genres(
    track: &crate::services::tidal::client::TidalTrack,
) -> Vec<String> {
    let mut candidates = extract_genre_candidates_from_extra(&track.extra);
    if let Some(album) = track.album.as_ref() {
        candidates.extend(extract_genre_candidates_from_extra(&album.extra));
    }

    crate::genre::builder::collect_clear_genres(candidates)
}

pub(crate) fn extract_genre_candidates_from_extra(
    extra: &std::collections::HashMap<String, Value>,
) -> Vec<String> {
    let mut candidates = Vec::new();

    for key in [
        "genre",
        "subGenre",
        "subgenre",
        "genres",
        "subGenres",
        "subgenres",
    ] {
        let Some(value) = extra.get(key) else {
            continue;
        };
        collect_genre_values(value, &mut candidates);
    }

    candidates
}

pub(super) fn collect_genre_values(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(raw) => {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                output.push(trimmed.to_string());
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_genre_values(item, output);
            }
        }
        Value::Object(map) => {
            for key in ["name", "title", "genre", "subGenre", "subgenre"] {
                if let Some(inner) = map.get(key) {
                    collect_genre_values(inner, output);
                }
            }
        }
        _ => {}
    }
}
