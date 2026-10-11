//! TIDAL sync bookkeeping.

use super::*;

// ─── Sync Metadata ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncInfo {
    pub service: String,
    pub last_sync_at: String,
    pub auto_sync_daily: bool,
    /// Import the rest of favorited albums as hidden discovery fill
    /// (is_library=0) after each sync. Default on.
    pub enrich_from_favorite_albums: bool,
    pub last_sync_track_count: i64,
    pub last_sync_album_count: i64,
    pub last_full_sync_at: Option<String>,
    pub last_sync_kind: Option<String>,
    pub tidal_favorite_artist_cursor: Option<String>,
    pub tidal_favorite_album_cursor: Option<String>,
    pub tidal_favorite_track_cursor: Option<String>,
}

pub fn get_sync_info(conn: &Connection, service: &str) -> Result<Option<SyncInfo>> {
    let mut stmt = conn.prepare(
        "SELECT service, last_sync_at, auto_sync_daily, last_sync_track_count, last_sync_album_count,
                last_full_sync_at, last_sync_kind,
                tidal_favorite_artist_cursor, tidal_favorite_album_cursor, tidal_favorite_track_cursor,
                enrich_from_favorite_albums
         FROM sync_metadata WHERE service = ?1",
    )?;
    let result = stmt
        .query_row([service], |row| {
            Ok(SyncInfo {
                service: row.get(0)?,
                last_sync_at: row.get(1)?,
                auto_sync_daily: row.get::<_, i64>(2)? != 0,
                enrich_from_favorite_albums: row.get::<_, i64>(10)? != 0,
                last_sync_track_count: row.get(3)?,
                last_sync_album_count: row.get(4)?,
                last_full_sync_at: row.get(5)?,
                last_sync_kind: row.get(6)?,
                tidal_favorite_artist_cursor: row.get(7)?,
                tidal_favorite_album_cursor: row.get(8)?,
                tidal_favorite_track_cursor: row.get(9)?,
            })
        })
        .optional()?;
    Ok(result)
}

pub fn update_sync_timestamp_with_metadata(
    conn: &Connection,
    service: &str,
    track_count: i64,
    album_count: i64,
    sync_kind: &str,
    artist_cursor: Option<&str>,
    album_cursor: Option<&str>,
    track_cursor: Option<&str>,
) -> Result<()> {
    let last_full_sync_expr = if sync_kind == "full" {
        "datetime('now')"
    } else {
        "sync_metadata.last_full_sync_at"
    };
    conn.execute(
        &format!(
            "INSERT INTO sync_metadata (
                service, last_sync_at, auto_sync_daily,
                last_sync_track_count, last_sync_album_count,
                last_full_sync_at, last_sync_kind,
                tidal_favorite_artist_cursor, tidal_favorite_album_cursor, tidal_favorite_track_cursor
             )
             VALUES (
                ?1, datetime('now'), 0,
                ?2, ?3,
                CASE WHEN ?4 = 'full' THEN datetime('now') ELSE NULL END,
                ?4, ?5, ?6, ?7
             )
         ON CONFLICT(service) DO UPDATE SET
             last_sync_at = datetime('now'),
             last_sync_track_count = ?2,
             last_sync_album_count = ?3,
             last_full_sync_at = {last_full_sync_expr},
             last_sync_kind = ?4,
             tidal_favorite_artist_cursor = COALESCE(?5, sync_metadata.tidal_favorite_artist_cursor),
             tidal_favorite_album_cursor = COALESCE(?6, sync_metadata.tidal_favorite_album_cursor),
             tidal_favorite_track_cursor = COALESCE(?7, sync_metadata.tidal_favorite_track_cursor)"
        ),
        rusqlite::params![
            service,
            track_count,
            album_count,
            sync_kind,
            artist_cursor,
            album_cursor,
            track_cursor
        ],
    )?;
    Ok(())
}

pub fn set_auto_sync_daily(conn: &Connection, service: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE sync_metadata SET auto_sync_daily = ?1 WHERE service = ?2",
        rusqlite::params![if enabled { 1 } else { 0 }, service],
    )?;
    Ok(())
}

pub fn set_enrich_from_favorite_albums(
    conn: &Connection,
    service: &str,
    enabled: bool,
) -> Result<()> {
    conn.execute(
        "UPDATE sync_metadata SET enrich_from_favorite_albums = ?1 WHERE service = ?2",
        rusqlite::params![if enabled { 1 } else { 0 }, service],
    )?;
    Ok(())
}

pub fn get_auto_sync_services(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT service FROM sync_metadata WHERE auto_sync_daily = 1")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// True if `service` has a recorded `last_sync_at` within `window_secs`.
/// Used by the boot-time auto-sync to honour the "daily" promise instead of
/// running on every server start.
pub fn sync_within_window(conn: &Connection, service: &str, window_secs: i64) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sync_metadata
         WHERE service = ?1
           AND last_sync_at IS NOT NULL
           AND last_sync_at != ''
           AND (strftime('%s','now') - strftime('%s', last_sync_at)) < ?2",
        rusqlite::params![service, window_secs],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod sync_metadata_tests {
    use super::*;
    use crate::db::schema;

    fn migrated_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        schema::run_migrations(&conn).expect("migrations");
        conn
    }

    #[test]
    fn sync_info_exposes_full_marker_kind_and_favorite_cursors() {
        let conn = migrated_db();

        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            42,
            7,
            "full",
            Some("2026-05-01T01:00:00Z"),
            Some("2026-05-02T01:00:00Z"),
            Some("2026-05-03T01:00:00Z"),
        )
        .expect("metadata update");

        let info = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row");
        assert_eq!(info.last_sync_kind.as_deref(), Some("full"));
        assert!(info.last_full_sync_at.is_some());
        assert_eq!(
            info.tidal_favorite_artist_cursor.as_deref(),
            Some("2026-05-01T01:00:00Z")
        );
        assert_eq!(
            info.tidal_favorite_album_cursor.as_deref(),
            Some("2026-05-02T01:00:00Z")
        );
        assert_eq!(
            info.tidal_favorite_track_cursor.as_deref(),
            Some("2026-05-03T01:00:00Z")
        );
    }

    #[test]
    fn incremental_sync_metadata_does_not_replace_last_full_marker() {
        let conn = migrated_db();
        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            20,
            3,
            "full",
            Some("2026-05-01T01:00:00Z"),
            Some("2026-05-02T01:00:00Z"),
            Some("2026-05-03T01:00:00Z"),
        )
        .expect("full update");
        let full_at = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row")
            .last_full_sync_at;

        update_sync_timestamp_with_metadata(
            &conn,
            "tidal",
            2,
            1,
            "incremental",
            Some("2026-05-04T01:00:00Z"),
            Some("2026-05-05T01:00:00Z"),
            Some("2026-05-06T01:00:00Z"),
        )
        .expect("incremental update");

        let info = get_sync_info(&conn, "tidal")
            .expect("sync info")
            .expect("tidal row");
        assert_eq!(info.last_sync_kind.as_deref(), Some("incremental"));
        assert_eq!(info.last_full_sync_at, full_at);
        assert_eq!(
            info.tidal_favorite_track_cursor.as_deref(),
            Some("2026-05-06T01:00:00Z")
        );
    }
}
