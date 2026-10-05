//! Local TIDAL content preference and observed provider labels. Never infer AI from names.
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::collections::HashSet;

pub fn enabled(conn: &Connection) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row(
            "SELECT value FROM server_config WHERE key='tidal_hide_ai'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .as_deref()
        == Some("1"))
}

pub fn set_enabled(conn: &Connection, enabled: bool) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config(key,value) VALUES('tidal_hide_ai',?1)",
        [if enabled { "1" } else { "0" }],
    )?;
    Ok(())
}

/// Record labels before any catalog projection or pagination. Missing labels retain prior evidence.
pub fn observe(conn: &Connection, payload: &Value) -> rusqlite::Result<()> {
    fn collect(value: &Value, labels: &mut Vec<(i64, Option<bool>)>) {
        match value {
            Value::Object(object) => {
                let video = object
                    .get("type")
                    .or_else(|| object.get("kind"))
                    .and_then(Value::as_str)
                    .is_some_and(|kind| kind.to_ascii_lowercase().contains("video"));
                if video {
                    return;
                }
                let track = object.contains_key("duration")
                    || object
                        .get("type")
                        .or_else(|| object.get("kind"))
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("track"));
                if track
                    && let Some(id) = object.get("id").and_then(Value::as_i64)
                    && id > 0
                {
                    labels.push((id, object.get("ai").and_then(Value::as_bool)));
                }
                for (key, child) in object {
                    if !key.to_ascii_lowercase().contains("video") {
                        collect(child, labels);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    collect(item, labels);
                }
            }
            _ => {}
        }
    }
    let mut labels = Vec::new();
    collect(payload, &mut labels);
    if labels.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for (id, ai) in labels {
        tx.execute("INSERT INTO tidal_track_labels(tidal_id,ai) VALUES(?1,?2) ON CONFLICT(tidal_id) DO UPDATE SET ai=COALESCE(excluded.ai,tidal_track_labels.ai)", params![id, ai])?;
    }
    tx.commit()
}

/// Favorites and managed-library entries remain accessible, even when marked AI.
pub fn blocked_ids(conn: &Connection) -> rusqlite::Result<HashSet<i64>> {
    if !enabled(conn)? {
        return Ok(HashSet::new());
    }
    conn.prepare("SELECT l.tidal_id FROM tidal_track_labels l WHERE l.ai=1 AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.tidal_id=l.tidal_id AND (t.is_library=1 OR t.is_favorite=1))")?
        .query_map([], |row| row.get(0))?.collect()
}

pub fn is_blocked(conn: &Connection, tidal_id: i64) -> rusqlite::Result<bool> {
    if !enabled(conn)? {
        return Ok(false);
    }
    conn.query_row("SELECT EXISTS(SELECT 1 FROM tidal_track_labels l WHERE l.tidal_id=?1 AND l.ai=1 AND NOT EXISTS(SELECT 1 FROM tracks t WHERE t.tidal_id=l.tidal_id AND (t.is_library=1 OR t.is_favorite=1)))", [tidal_id], |row| row.get(0))
}

/// Shared SQL predicate for paginated local catalog reads (the track alias is `t`).
pub fn browse_predicate(conn: &Connection) -> rusqlite::Result<Option<&'static str>> {
    Ok(enabled(conn)?.then_some("(t.is_library=1 OR t.is_favorite=1 OR NOT EXISTS(SELECT 1 FROM tidal_track_labels ai_label WHERE ai_label.tidal_id=t.tidal_id AND ai_label.ai=1))"))
}

pub fn local_is_blocked(conn: &Connection, id: i64) -> rusqlite::Result<bool> {
    let tidal: Option<i64> = conn
        .query_row("SELECT tidal_id FROM tracks WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .optional()?
        .flatten();
    tidal.map(|id| is_blocked(conn, id)).unwrap_or(Ok(false))
}

/// Filter only recognizable track objects, keeping album/artist ids in their own namespaces.
pub fn filter_json(value: &mut Value, blocked: &HashSet<i64>, tidal_context: bool) {
    fn track_id(value: &Value, tidal_context: bool) -> Option<i64> {
        if value
            .get("type")
            .or_else(|| value.get("kind"))
            .or_else(|| value.get("entity_type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.to_ascii_lowercase().contains("video"))
        {
            return None;
        }
        if let Some(item) = value
            .get("item")
            .or_else(|| value.get("track"))
            .or_else(|| value.get("local_track").filter(|v| !v.is_null()))
            .or_else(|| value.get("tidal_playable"))
        {
            return track_id(item, tidal_context);
        }
        let track = value
            .get("type")
            .or_else(|| value.get("kind"))
            .or_else(|| value.get("entity_type"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.eq_ignore_ascii_case("track"))
            || value.get("duration").is_some()
            || value.get("duration_ms").is_some()
            || value.get("track_id").is_some()
            || value.get("track_number").is_some();
        if !track {
            return None;
        }
        value
            .get("tidal_id")
            .or_else(|| value.get("tidal_track_id"))
            .and_then(Value::as_i64)
            .or_else(|| {
                tidal_context
                    .then(|| {
                        value
                            .get("id")
                            .and_then(|id| id.as_i64().or_else(|| id.as_str()?.parse().ok()))
                    })
                    .flatten()
            })
    }
    if value
        .get("type")
        .or_else(|| value.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.to_ascii_lowercase().contains("video"))
    {
        return;
    }
    match value {
        Value::Array(items) => {
            items.retain(|item| {
                !track_id(item, tidal_context).is_some_and(|id| blocked.contains(&id))
            });
            for item in items {
                filter_json(item, blocked, tidal_context);
            }
        }
        Value::Object(object) => {
            if let Some(Value::Object(cells)) = object.get_mut("cells") {
                for cell in cells.values_mut() {
                    if track_id(cell, tidal_context).is_some_and(|id| blocked.contains(&id)) {
                        *cell = Value::Null;
                    }
                }
            }
            for (key, child) in object.iter_mut() {
                if !key.to_ascii_lowercase().contains("video") {
                    filter_json(child, blocked, tidal_context);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn tidal_ai_preference_defaults_off_and_persists() {
        let conn = conn();
        assert!(!enabled(&conn).unwrap());
        set_enabled(&conn, true).unwrap();
        // Rerunning startup migrations must not reset the saved preference.
        crate::db::schema::run_migrations(&conn).unwrap();
        assert!(enabled(&conn).unwrap());
        set_enabled(&conn, false).unwrap();
        assert!(!enabled(&conn).unwrap());
    }

    #[test]
    fn tidal_ai_requires_boolean_provider_evidence_and_preserves_pagination() {
        let conn = conn();
        let payload = json!({"items": [
            {"id":1,"duration":180,"ai":true}, {"id":2,"duration":180,"ai":false},
            {"id":3,"duration":180}, {"id":4,"duration":180,"ai":"true"},
            {"id":5,"duration":180,"ai":1}, {"id":6,"duration":180,"title":"AI music"}
        ], "totalNumberOfItems":100, "offset":0, "limit":6});
        observe(&conn, &payload).unwrap();
        assert_eq!(payload["items"].as_array().unwrap().len(), 6);
        assert_eq!(payload["totalNumberOfItems"], 100);
        assert!(blocked_ids(&conn).unwrap().is_empty());
        set_enabled(&conn, true).unwrap();
        assert_eq!(blocked_ids(&conn).unwrap(), HashSet::from([1]));
        observe(&conn, &json!({"id":1,"duration":180})).unwrap();
        assert!(is_blocked(&conn, 1).unwrap());
        observe(&conn, &json!({"id":1,"duration":180,"ai":false})).unwrap();
        assert!(!is_blocked(&conn, 1).unwrap());
    }

    #[test]
    fn tidal_ai_saved_library_and_favorites_remain_available() {
        let conn = conn();
        observe(&conn, &json!([{"id":1,"duration":180,"ai":true},{"id":2,"duration":180,"ai":true},{"id":3,"duration":180,"ai":true}])).unwrap();
        conn.execute("INSERT INTO artists(id,name) VALUES(1,'Artist')", [])
            .unwrap();
        conn.execute("INSERT INTO tracks(id,tidal_id,title,artist_id,is_library,is_favorite) VALUES(10,1,'Saved',1,1,0),(11,2,'Favorite',1,0,1),(12,3,'Imported',1,0,0)", []).unwrap();
        set_enabled(&conn, true).unwrap();
        assert_eq!(blocked_ids(&conn).unwrap(), HashSet::from([3]));
        assert!(!local_is_blocked(&conn, 10).unwrap());
        assert!(!local_is_blocked(&conn, 11).unwrap());
        assert!(local_is_blocked(&conn, 12).unwrap());
    }

    #[test]
    fn tidal_ai_video_and_album_ids_do_not_collide_with_tracks() {
        let conn = conn();
        observe(&conn, &json!({"items":[{"type":"VIDEO","item":{"id":1,"duration":180,"ai":true}},{"id":2,"numberOfTracks":10,"ai":true}],"videos":{"items":[{"id":3,"duration":180,"ai":true}]}})).unwrap();
        set_enabled(&conn, true).unwrap();
        assert!(blocked_ids(&conn).unwrap().is_empty());
        let mut payload = json!({"items":[{"kind":"video","id":"1","duration":180},{"kind":"album","id":"1"},{"kind":"track","id":"1","duration":180},{"type":"TRACK","item":{"id":1,"duration":180}},{"local_track":null,"tidal_playable":{"tidal_id":1,"duration_ms":180000}}],"videos":[{"id":1,"duration":180}]});
        filter_json(&mut payload, &HashSet::from([1]), true);
        assert_eq!(payload["items"].as_array().unwrap().len(), 2);
        assert_eq!(payload["videos"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn tidal_ai_local_ids_are_never_treated_as_tidal_ids() {
        let mut payload = json!([{"id":1,"tidal_id":99,"duration_ms":180000},{"id":99,"tidal_id":1,"duration_ms":180000},{"id":1,"tidal_id":null,"duration_ms":180000}]);
        filter_json(&mut payload, &HashSet::from([1]), false);
        assert_eq!(payload.as_array().unwrap().len(), 2);
        assert_eq!(payload[0]["tidal_id"], 99);
    }
    #[test]
    fn tidal_ai_local_pagination_counts_and_candidate_limits_use_visible_tracks() {
        let conn = conn();
        conn.execute("INSERT INTO artists(id,name) VALUES(1,'Artist')", [])
            .unwrap();
        conn.execute("INSERT INTO tracks(id,tidal_id,title,artist_id,is_library) VALUES(1,101,'A Hidden',1,0),(2,102,'B Allowed',1,0),(3,103,'C Allowed',1,0)", []).unwrap();
        observe(&conn, &json!({"id":101,"duration":180,"ai":true})).unwrap();
        set_enabled(&conn, true).unwrap();
        assert_eq!(
            crate::db::queries::get_track_count(&conn, false, false).unwrap(),
            2
        );
        let first =
            crate::db::queries::get_tracks(&conn, "title", "asc", 1, 0, false, false).unwrap();
        let second =
            crate::db::queries::get_tracks(&conn, "title", "asc", 1, 1, false, false).unwrap();
        assert_eq!(first[0].id, 2);
        assert_eq!(second[0].id, 3);
        assert_eq!(
            crate::db::queries::get_discovery_candidate_tracks(&conn, 1).unwrap()[0].id,
            2
        );
        assert_eq!(
            crate::db::queries::get_tracks_excluding_with_limit(&conn, &[3], 1).unwrap()[0].id,
            2
        );
        assert_eq!(
            crate::db::queries::search(&conn, "Allowed", 10)
                .unwrap()
                .tracks
                .len(),
            2
        );
        assert!(
            crate::db::queries::search(&conn, "Hidden", 10)
                .unwrap()
                .tracks
                .is_empty()
        );
        set_enabled(&conn, false).unwrap();
        assert_eq!(
            crate::db::queries::get_track_count(&conn, false, false).unwrap(),
            3
        );
    }
}
