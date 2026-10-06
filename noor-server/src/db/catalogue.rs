//! A local recording survives changes to its provider catalogue IDs.
use crate::services::tidal::client::{TidalClient, TidalTrack};
use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub fn enabled(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='tidal_track_aliases')",
        [],
        |r| r.get(0),
    )
}

pub fn track_id(conn: &Connection, tidal_id: i64) -> Result<Option<i64>> {
    let direct = conn
        .query_row("SELECT id FROM tracks WHERE tidal_id=?1", [tidal_id], |r| {
            r.get(0)
        })
        .optional()?;
    if direct.is_some() || !enabled(conn)? {
        return Ok(direct);
    }
    Ok(conn
        .query_row(
            "SELECT track_id FROM tidal_track_aliases WHERE tidal_id=?1",
            [tidal_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn album_id(conn: &Connection, tidal_id: i64) -> Result<Option<i64>> {
    let direct = conn
        .query_row("SELECT id FROM albums WHERE tidal_id=?1", [tidal_id], |r| {
            r.get(0)
        })
        .optional()?;
    if direct.is_some() || !enabled(conn)? {
        return Ok(direct);
    }
    Ok(conn
        .query_row(
            "SELECT album_id FROM tidal_album_aliases WHERE tidal_id=?1",
            [tidal_id],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .or_else(|_| DateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f%z"))
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|t| t.and_utc())
        })
}

pub fn earliest(left: Option<&str>, right: Option<&str>) -> Option<String> {
    match (left.and_then(timestamp), right.and_then(timestamp)) {
        (Some(a), Some(b)) => {
            if a <= b {
                left
            } else {
                right
            }
        }
        (Some(_), None) => left,
        (None, Some(_)) => right,
        (None, None) => None,
    }
    .map(str::to_owned)
}

pub fn canonical_date(raw: &str) -> Option<String> {
    timestamp(raw).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

pub fn normalize_saved_dates(conn: &Connection) -> Result<()> {
    let rows = {
        let mut stmt = conn.prepare(
            "SELECT id,date_added,library_added_at FROM tracks WHERE is_library=1 OR is_favorite=1",
        )?;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, visible, saved) in rows {
        let Some(date) = saved
            .as_deref()
            .or(visible.as_deref())
            .and_then(canonical_date)
        else {
            continue;
        };
        if visible.as_deref() == Some(&date) && saved.as_deref() == Some(&date) {
            continue;
        }
        conn.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json) VALUES('date_format',?1,?1,json_object('date_added',?2,'library_added_at',?3))",params![id,visible,saved])?;
        conn.execute(
            "UPDATE tracks SET date_added=?2,library_added_at=?2 WHERE id=?1",
            params![id, date],
        )?;
    }
    Ok(())
}

/// A discovery import date is not the date the user first saved a recording.
pub fn curate(
    conn: &Connection,
    id: i64,
    favorite: bool,
    library: bool,
    created: Option<&str>,
) -> Result<()> {
    if !favorite && !library {
        return Ok(());
    }
    let (old, source): (Option<String>, Option<String>) = conn.query_row(
        "SELECT library_added_at,library_date_source FROM tracks WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let valid_created = created.filter(|v| timestamp(v).is_some());
    // The effective library date can be an intentional re-like or accepted
    // recovery. Neither is replaced by an automatic provider observation.
    let protected = matches!(source.as_deref(), Some("user" | "recovered"));
    let date = if protected {
        old.clone()
    } else {
        earliest(old.as_deref(), valid_created)
    }
    .unwrap_or_else(|| Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
    let date = canonical_date(&date).unwrap_or(date);
    let favorite = crate::db::catalogue_favorites::desired(conn, id)?.unwrap_or(favorite);
    conn.execute("UPDATE tracks SET date_added=?2,library_added_at=?2,
        library_date_source=COALESCE(library_date_source,?3),
        is_library=MAX(is_library,?4),is_favorite=MAX(is_favorite,?5),
        remote_favorite_state=CASE WHEN ?5=1 THEN 'favorite' ELSE remote_favorite_state END WHERE id=?1",
        params![id,date,if valid_created.is_some(){"provider"}else{"library"},library as i32,favorite as i32])?;
    Ok(())
}

pub fn record_track(
    conn: &Connection,
    local_id: i64,
    track: &TidalTrack,
    favorite: bool,
    created: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO artists(tidal_id,name) VALUES(?1,?2) ON CONFLICT(tidal_id) DO NOTHING",
        params![track.artist.id, track.artist.name],
    )?;
    if let Some(album) = track.album.as_ref()
        && album_id(conn, album.id)?.is_none()
    {
        conn.execute(
            "INSERT INTO albums(tidal_id,title,artist_id,artwork_url,source)
            VALUES(?1,?2,(SELECT id FROM artists WHERE tidal_id=?3),?4,'tidal')",
            params![
                album.id,
                album.title,
                track.artist.id,
                TidalClient::get_artwork_url(&album.cover, 640)
            ],
        )?;
    }
    let availability = if track.stream_ready == Some(false)
        || track.extra.get("allowStreaming").and_then(|v| v.as_bool()) == Some(false)
    {
        "unavailable"
    } else if track.stream_ready == Some(true) {
        "available"
    } else {
        "unknown"
    };
    let owner: Option<i64> = conn
        .query_row(
            "SELECT track_id FROM tidal_track_aliases WHERE tidal_id=?1",
            [track.id],
            |r| r.get(0),
        )
        .optional()?;
    anyhow::ensure!(
        owner.is_none_or(|id| id == local_id),
        "TIDAL alias {} belongs to another recording",
        track.id
    );
    // Recheck an alternative's identity on metadata refresh. Provider metadata
    // can change; an old alias must not become a route to a different version.
    let selected: Option<i64> =
        conn.query_row("SELECT tidal_id FROM tracks WHERE id=?1", [local_id], |r| {
            r.get(0)
        })?;
    if !identity_matches(conn, local_id, track, selected == Some(track.id))? {
        conn.execute("UPDATE tidal_track_aliases SET availability='error',evidence='identity_conflict',checked_at=datetime('now') WHERE tidal_id=?1",[track.id])?;
        tracing::warn!(target:"noor.catalogue",local_id,tidal_id=track.id,"catalogue identity needs review");
        return Ok(());
    }
    conn.execute("INSERT INTO tidal_track_aliases(tidal_id,track_id,metadata_json,availability,checked_at,evidence,favorite_created,is_favorite)
        VALUES(?1,?2,?3,?4,datetime('now'),'metadata',?5,?6)
        ON CONFLICT(tidal_id) DO UPDATE SET metadata_json=excluded.metadata_json,
        availability=CASE WHEN excluded.availability='unknown' THEN tidal_track_aliases.availability ELSE excluded.availability END,
        checked_at=CASE WHEN excluded.availability='unknown' THEN tidal_track_aliases.checked_at ELSE excluded.checked_at END,
        evidence=CASE WHEN excluded.availability='unknown' THEN tidal_track_aliases.evidence ELSE 'metadata' END,
        favorite_created=COALESCE(excluded.favorite_created,tidal_track_aliases.favorite_created),
        is_favorite=MAX(tidal_track_aliases.is_favorite,excluded.is_favorite)",
        params![track.id,local_id,serde_json::to_string(track)?,availability,created,favorite as i32])?;
    let album = track
        .album
        .as_ref()
        .map(|a| album_id(conn, a.id))
        .transpose()?
        .flatten();
    let quality = track.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        "LOW" => 200,
        _ => 500,
    };
    conn.execute(
        "UPDATE tracks SET title=?2,isrc=COALESCE(?3,isrc),
        duration_ms=CASE WHEN ?4>0 THEN ?4 ELSE duration_ms END,album_id=COALESCE(?5,album_id),
        disc_number=COALESCE(?6,disc_number),track_number=COALESCE(?7,track_number),
        best_quality=?8,fidelity_score=?9 WHERE id=?1 AND tidal_id=?10",
        params![
            local_id,
            track.title,
            track.isrc,
            track.duration * 1000,
            album,
            track.volume_number,
            track.track_number,
            quality,
            fidelity,
            track.id
        ],
    )?;
    if selected == Some(track.id) && crate::db::catalogue_favorites::enabled(conn)? {
        conn.execute("UPDATE tracks SET catalogue_version=COALESCE(?2,catalogue_version),catalogue_explicit=COALESCE(?3,catalogue_explicit) WHERE id=?1",
            params![local_id,track.extra.get("version").and_then(|v|v.as_str()),track.extra.get("explicit").and_then(|v|v.as_bool())])?;
    }
    choose_available(conn, local_id)?;
    Ok(())
}

/// Only switch when the current release is known unavailable and a fresh,
/// equivalent alias is available. Unknown/error never authorizes a switch.
pub fn choose_available(conn: &Connection, local_id: i64) -> Result<bool> {
    let candidate: Option<(i64, String)> = conn
        .query_row(
            "SELECT alt.tidal_id,alt.metadata_json FROM tracks t
         JOIN tidal_track_aliases current ON current.tidal_id=t.tidal_id
         JOIN tidal_track_aliases alt ON alt.track_id=t.id
         WHERE t.id=?1 AND current.availability='unavailable'
         AND julianday(current.checked_at)>julianday('now','-1 day')
         AND alt.availability='available' AND alt.metadata_json IS NOT NULL
         AND julianday(alt.checked_at)>julianday('now','-1 day')
         ORDER BY alt.is_favorite DESC,alt.tidal_id ASC LIMIT 1",
            [local_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((new_id, json)) = candidate else {
        return Ok(false);
    };
    let track: TidalTrack = serde_json::from_str(&json)?;
    if !identity_matches(conn, local_id, &track, false)? {
        return Ok(false);
    }
    let old_id: i64 =
        conn.query_row("SELECT tidal_id FROM tracks WHERE id=?1", [local_id], |r| {
            r.get(0)
        })?;
    conn.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json)
        VALUES('replacement',?1,?2,json_object('old_tidal_id',?2,'new_tidal_id',?3,'reason','verified_availability'))",
        params![local_id,old_id,new_id])?;
    let album = track
        .album
        .as_ref()
        .map(|a| a.id)
        .map(|id| album_id(conn, id))
        .transpose()?
        .flatten();
    let quality = track.audio_quality.as_deref().unwrap_or("LOSSLESS");
    let fidelity = match quality {
        "HI_RES_LOSSLESS" => 900,
        "HI_RES" => 800,
        "LOSSLESS" => 700,
        "HIGH" => 400,
        "LOW" => 200,
        _ => 500,
    };
    conn.execute(
        "UPDATE tracks SET tidal_id=?2,title=?3,album_id=COALESCE(?4,album_id),
        duration_ms=?5,isrc=?6,disc_number=COALESCE(?7,disc_number),track_number=?8,
        best_quality=?9,fidelity_score=?10,updated_at=datetime('now') WHERE id=?1",
        params![
            local_id,
            new_id,
            track.title,
            album,
            track.duration * 1000,
            track.isrc,
            track.volume_number,
            track.track_number,
            quality,
            fidelity
        ],
    )?;
    if crate::db::catalogue_favorites::enabled(conn)? {
        conn.execute(
            "UPDATE tracks SET catalogue_version=?2,catalogue_explicit=?3 WHERE id=?1",
            params![
                local_id,
                track.extra.get("version").and_then(|v| v.as_str()),
                track.extra.get("explicit").and_then(|v| v.as_bool())
            ],
        )?;
    }
    conn.execute(
        "UPDATE queue SET tidal_id_hint=?2 WHERE track_id=?1 AND tidal_id_hint=?3",
        params![local_id, new_id, old_id],
    )?;
    tracing::info!(target:"noor.catalogue",local_id,old_id,new_id,"selected available catalogue replacement");
    Ok(true)
}

fn identity_matches(
    conn: &Connection,
    id: i64,
    track: &TidalTrack,
    adopt_unknown: bool,
) -> Result<bool> {
    use crate::library::duplicates::{
        ExistingCandidate, ImportDecision, IncomingTrack, decide_import,
    };
    let mut candidate=conn.query_row("SELECT t.tidal_id,t.title,a.name,t.isrc,COALESCE(t.duration_ms,0) FROM tracks t JOIN artists a ON a.id=t.artist_id WHERE t.id=?1",[id],|r|Ok(ExistingCandidate{track_id:id,tidal_id:r.get(0)?,title:r.get(1)?,artist_name:r.get(2)?,isrc:r.get(3)?,duration_ms:r.get(4)?,version:None,explicit:None}))?;
    if crate::db::catalogue_favorites::enabled(conn)? {
        (candidate.version, candidate.explicit) = conn.query_row(
            "SELECT catalogue_version,catalogue_explicit FROM tracks WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
    }
    let version = track.extra.get("version").and_then(|v| v.as_str());
    let explicit = track.extra.get("explicit").and_then(|v| v.as_bool());
    if adopt_unknown {
        if candidate.isrc.is_none() {
            candidate.isrc = track.isrc.clone();
        }
        if candidate.version.is_none() {
            candidate.version = version.map(str::to_owned);
        }
        if candidate.explicit.is_none() {
            candidate.explicit = explicit;
        }
        if candidate.duration_ms == 0 {
            candidate.duration_ms = track.duration * 1000;
        }
    }
    // Avoid the import shortcut for an existing primary ID: refreshed metadata
    // must still agree with the independent recording identity we already saved.
    let mut incoming_isrc = track.isrc.clone();
    if adopt_unknown && incoming_isrc.is_none() {
        incoming_isrc = candidate.isrc.clone();
    }
    if adopt_unknown && incoming_isrc.is_none() {
        candidate.isrc = Some("__same_provider_id__".into());
        incoming_isrc = candidate.isrc.clone();
    }
    let effective_version = if adopt_unknown && !track.extra.contains_key("version") {
        candidate.version.clone()
    } else {
        version.map(str::to_owned)
    };
    let effective_explicit = if adopt_unknown && !track.extra.contains_key("explicit") {
        candidate.explicit
    } else {
        explicit
    };
    let incoming = IncomingTrack {
        tidal_id: 0,
        title: &track.title,
        artist_name: &track.artist.name,
        isrc: incoming_isrc.as_deref(),
        duration_ms: track.duration * 1000,
        version: effective_version.as_deref(),
        explicit: effective_explicit,
    };
    Ok(matches!(
        decide_import(&incoming, &[candidate]),
        ImportDecision::LinkAlias { .. }
    ))
}

pub fn observe(
    conn: &Connection,
    tidal_id: i64,
    availability: &str,
    evidence: &str,
) -> Result<bool> {
    conn.execute("UPDATE tidal_track_aliases SET availability=?2,evidence=?3,checked_at=datetime('now') WHERE tidal_id=?1 AND evidence!='identity_conflict'",
        params![tidal_id,availability,evidence])?;
    let local_id = track_id(conn, tidal_id)?;
    match local_id {
        Some(id) => choose_available(conn, id),
        None => Ok(false),
    }
}

/// Preserve aliases and original curated dates before deleting a merged row.
pub fn retain_merge(conn: &Connection, kept: i64, removed: i64) -> Result<()> {
    if !enabled(conn)? {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json)
        SELECT 'track',?1,id,json_object('tidal_id',tidal_id,'date_added',date_added,
        'library_added_at',library_added_at,'is_favorite',is_favorite,'is_library',is_library,
        'library_date_source',library_date_source,'title',title,'isrc',isrc,'album_id',album_id,
        'kept_before',(SELECT json_object('date_added',date_added,'library_added_at',library_added_at,'library_date_source',library_date_source) FROM tracks WHERE id=?1)) FROM tracks WHERE id=?2",
        params![kept, removed],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO tidal_track_aliases(tidal_id,track_id,is_favorite)
        SELECT tidal_id,id,is_favorite FROM tracks WHERE id IN (?1,?2) AND tidal_id>0",
        params![kept, removed],
    )?;
    conn.execute(
        "UPDATE tidal_track_aliases SET track_id=?1 WHERE track_id=?2",
        params![kept, removed],
    )?;
    let (a,b,sa,sb):(Option<String>,Option<String>,Option<String>,Option<String>)=conn.query_row(
        "SELECT k.library_added_at,r.library_added_at,k.library_date_source,r.library_date_source FROM tracks k JOIN tracks r ON r.id=?2 WHERE k.id=?1",
        params![kept,removed],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let choices: Option<(Option<String>, Option<String>)> =
        if crate::db::catalogue_favorites::enabled(conn)? {
            Some(conn.query_row("SELECT k.date_choice_at,r.date_choice_at FROM tracks k JOIN tracks r ON r.id=?2 WHERE k.id=?1",params![kept,removed],|r|Ok((r.get(0)?,r.get(1)?)))?)
        } else {
            None
        };
    let protected_a = matches!(sa.as_deref(), Some("user" | "recovered"));
    let protected_b = matches!(sb.as_deref(), Some("user" | "recovered"));
    let choose_b = match choices.as_ref() {
        Some((ca, cb)) => match (
            ca.as_deref().and_then(timestamp),
            cb.as_deref().and_then(timestamp),
        ) {
            (Some(x), Some(y)) => Some(y > x),
            (None, Some(_)) => Some(true),
            (Some(_), None) => Some(false),
            _ => None,
        },
        None => None,
    }
    .or_else(|| {
        if protected_a {
            Some(false)
        } else if protected_b {
            Some(true)
        } else {
            None
        }
    });
    let chosen = match choose_b {
        Some(true) => b.clone(),
        Some(false) => a.clone(),
        None => earliest(a.as_deref(), b.as_deref()),
    };
    if let Some(date) = chosen {
        let source = if a.as_deref() == Some(date.as_str()) {
            sa
        } else {
            sb
        };
        conn.execute("UPDATE tracks SET library_added_at=?2,date_added=?2,library_date_source=COALESCE(?3,'merged') WHERE id=?1",params![kept,canonical_date(&date).unwrap_or(date),source])?;
        if let Some((ca, cb)) = choices {
            conn.execute(
                "UPDATE tracks SET date_choice_at=?2 WHERE id=?1",
                params![kept, if choose_b == Some(true) { cb } else { ca }],
            )?;
        }
    }
    crate::db::catalogue_favorites::retain_merge(conn, "track", kept, removed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_dates_compare_offsets_and_legacy_formats_chronologically() {
        assert_eq!(
            earliest(
                Some("2020-06-10T08:00:00+1000"),
                Some("2020-06-09 23:00:00")
            ),
            Some("2020-06-10T08:00:00+1000".into())
        );
        assert_eq!(
            earliest(
                Some("2020-06-10 07:00:00"),
                Some("2020-06-10T06:59:59.123+0000")
            ),
            Some("2020-06-10T06:59:59.123+0000".into())
        );
        assert_eq!(
            earliest(Some("not a date"), Some("2020-01-01T00:00:00Z")),
            Some("2020-01-01T00:00:00Z".into())
        );
    }
}

/// A complete favorites snapshot updates alias state, not just the selected ID.
pub fn reconcile_favorites_at(
    conn: &Connection,
    ids: &std::collections::HashSet<i64>,
    albums: bool,
    started: &str,
) -> Result<()> {
    let entity = if albums { "album" } else { "track" };
    let has_intents = crate::db::catalogue_favorites::enabled(conn)?;
    if has_intents {
        let previous: Option<String> = conn
            .query_row(
                "SELECT started_at FROM tidal_favorite_snapshots WHERE entity=?1",
                [entity],
                |r| r.get(0),
            )
            .optional()?;
        if previous
            .as_deref()
            .and_then(timestamp)
            .zip(timestamp(started))
            .is_some_and(|(previous, current)| previous > current)
        {
            // A late response must not erase newer per-alias observations.
            return Ok(());
        }
    }
    let (table, aliases, fk) = if albums {
        ("albums", "tidal_album_aliases", "album_id")
    } else {
        ("tracks", "tidal_track_aliases", "track_id")
    };
    conn.execute(&format!("UPDATE {aliases} SET is_favorite=0"), [])?;
    for id in ids {
        conn.execute(
            &format!("UPDATE {aliases} SET is_favorite=1 WHERE tidal_id=?1"),
            [id],
        )?;
    }
    if albums {
        // Missing aliases are ambiguous; the saved shelf remains visible.
        conn.execute(&format!("UPDATE {table} SET is_favorite=1 WHERE EXISTS(SELECT 1 FROM {aliases} a WHERE a.{fk}={table}.id AND a.is_favorite=1)"),[])?;
    } else {
        conn.execute("UPDATE tracks SET remote_favorite_state=CASE
            WHEN EXISTS(SELECT 1 FROM tidal_track_aliases a WHERE a.track_id=tracks.id AND a.is_favorite=1) THEN 'favorite'
            WHEN EXISTS(SELECT 1 FROM tidal_track_aliases a WHERE a.track_id=tracks.id AND
                (a.availability!='available' OR a.checked_at IS NULL OR julianday(a.checked_at)<=julianday('now','-1 day'))) THEN 'unresolved'
            ELSE 'not_favorite' END
            WHERE source='tidal' AND tidal_id IS NOT NULL",[])?;
        conn.execute(
            "UPDATE tracks SET is_favorite=CASE WHEN remote_favorite_state='favorite' THEN 1
            WHEN remote_favorite_state='not_favorite' THEN 0 ELSE is_favorite END
            WHERE source='tidal' AND tidal_id IS NOT NULL",
            [],
        )?;
    }
    if has_intents {
        conn.execute("INSERT INTO tidal_favorite_snapshots(entity,started_at) VALUES(?1,?2) ON CONFLICT(entity) DO UPDATE SET started_at=excluded.started_at",params![entity,started])?;
        crate::db::catalogue_favorites::protect_snapshot(
            conn,
            if albums { "album" } else { "track" },
            started,
        )?;
    }
    Ok(())
}

/// Equivalent albums need a complete ordered recording fingerprint, not a title match.
pub fn reconcile_album(conn: &Connection, tidal_id: i64, tracks: &[TidalTrack]) -> Result<()> {
    if tracks.is_empty()
        || tracks
            .iter()
            .any(|t| t.isrc.as_deref().is_none_or(|s| s.trim().is_empty()))
    {
        return Ok(());
    }
    let Some(local) = album_id(conn, tidal_id)? else {
        return Ok(());
    };
    let expected: Option<i64> =
        conn.query_row("SELECT track_count FROM albums WHERE id=?1", [local], |r| {
            r.get(0)
        })?;
    if expected.is_none_or(|n| n != tracks.len() as i64) {
        return Ok(());
    }
    let mut ordered: Vec<_> = tracks.iter().collect();
    ordered.sort_by_key(|t| (t.volume_number.unwrap_or(1), t.track_number.unwrap_or(0)));
    if ordered.iter().any(|t| t.track_number.is_none()) {
        return Ok(());
    }
    // Full titles retain edition/version markers; a different master is not silently collapsed.
    let fingerprint = serde_json::to_string(
        &ordered
            .iter()
            .map(|t| {
                (
                    t.volume_number.unwrap_or(1),
                    t.track_number,
                    t.isrc.as_deref().map(str::to_ascii_uppercase),
                    t.title.trim().to_lowercase(),
                    t.duration,
                    t.artist.id,
                    t.extra
                        .get("version")
                        .and_then(|v| v.as_str())
                        .map(|v| v.trim().to_lowercase()),
                    t.extra.get("explicit").and_then(|v| v.as_bool()),
                )
            })
            .collect::<Vec<_>>(),
    )?;
    conn.execute(
        "UPDATE tidal_album_aliases SET fingerprint=?2 WHERE tidal_id=?1",
        params![tidal_id, fingerprint],
    )?;
    // Completed albums imported before alias support need no extra network
    // request when their complete, ordered recording set is still local.
    let candidates = {
        let mut stmt=conn.prepare("SELECT old.id,old.tidal_id,old.track_count FROM albums old JOIN albums new ON new.id=?1
            JOIN tidal_album_aliases a ON a.tidal_id=old.tidal_id
            WHERE old.id!=?1 AND old.artist_id=new.artist_id AND lower(old.title)=lower(new.title)
            AND a.fingerprint IS NULL AND old.track_count>0")?;
        stmt.query_map([local], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?
    };
    for (id, provider, count) in candidates {
        let mut stmt=conn.prepare("SELECT COALESCE(t.disc_number,1),t.track_number,t.isrc,lower(trim(t.title)),t.duration_ms/1000,a.tidal_id,t.catalogue_version,t.catalogue_explicit
            FROM tracks t JOIN artists a ON a.id=t.artist_id WHERE t.album_id=?1
            ORDER BY COALESCE(t.disc_number,1),t.track_number")?;
        let stored = stmt
            .query_map([id], |r| {
                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, Option<i32>>(1)?,
                    r.get::<_, Option<String>>(2)?
                        .map(|s| s.to_ascii_uppercase()),
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<String>>(6)?
                        .map(|v| v.trim().to_lowercase()),
                    r.get::<_, Option<bool>>(7)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if stored.len() != count as usize
            || stored.iter().any(|r| {
                r.1.is_none()
                    || r.2.as_deref().is_none_or(str::is_empty)
                    || r.4.is_none_or(|n| n <= 0)
                    || r.5.is_none()
            })
        {
            continue;
        }
        let json = serde_json::to_string(
            &stored
                .iter()
                .map(|r| {
                    (
                        r.0,
                        r.1,
                        r.2.clone(),
                        r.3.clone(),
                        r.4.unwrap(),
                        r.5.unwrap(),
                        r.6.clone(),
                        r.7,
                    )
                })
                .collect::<Vec<_>>(),
        )?;
        conn.execute(
            "UPDATE tidal_album_aliases SET fingerprint=?2 WHERE tidal_id=?1",
            params![provider, json],
        )?;
    }
    let other:Option<i64>=conn.query_row("SELECT a.album_id FROM tidal_album_aliases a
        JOIN albums old ON old.id=a.album_id JOIN albums new ON new.id=?1
        WHERE a.fingerprint=?2 AND a.album_id!=?1 AND old.artist_id=new.artist_id
        AND lower(old.title)=lower(new.title) AND COALESCE(old.release_type,'album')=COALESCE(new.release_type,'album')
        ORDER BY a.album_id LIMIT 1",params![local,fingerprint],|r|r.get(0)).optional()?;
    let Some(other) = other else {
        return Ok(());
    };
    // Prefer the previously saved local album identity.
    let kept = local.min(other);
    let removed = local.max(other);
    conn.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json)
        SELECT 'album',?1,id,json_object('tidal_id',tidal_id,'title',title,'is_favorite',is_favorite) FROM albums WHERE id=?2",params![kept,removed])?;
    conn.execute(
        "UPDATE tidal_album_aliases SET album_id=?1 WHERE album_id=?2",
        params![kept, removed],
    )?;
    conn.execute(
        "UPDATE tracks SET album_id=?1 WHERE album_id=?2",
        params![kept, removed],
    )?;
    conn.execute("UPDATE albums SET is_favorite=MAX(is_favorite,(SELECT is_favorite FROM albums WHERE id=?2)),
        enrich_completed_at=COALESCE(enrich_completed_at,(SELECT enrich_completed_at FROM albums WHERE id=?2)) WHERE id=?1",params![kept,removed])?;
    crate::db::catalogue_favorites::retain_merge(conn, "album", kept, removed)?;
    if let Some(value) = crate::db::catalogue_favorites::desired_entity(conn, "album", kept)? {
        conn.execute(
            "UPDATE albums SET is_favorite=?2 WHERE id=?1",
            params![kept, value as i32],
        )?;
    }
    conn.execute("DELETE FROM albums WHERE id=?1", [removed])?;
    // The incoming release supplied the complete verified tracklist. Use it
    // for future catalogue detail fetches while retaining the old album ID.
    conn.execute(
        "UPDATE albums SET tidal_id=?2 WHERE id=?1",
        params![kept, tidal_id],
    )?;
    Ok(())
}
