//! Explicit favorite intent is durable and separate from observed provider state.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub fn enabled(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='tidal_favorite_intents')",
        [],
        |r| r.get(0),
    )
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

/// Called within the action transaction. No network request happens here.
pub fn request(conn: &Connection, entity: &str, local_id: i64, favorite: bool) -> Result<bool> {
    let (table, aliases, fk) = tables(entity)?;
    let (selected, was_favorite): (Option<i64>, bool) = conn.query_row(
        &format!("SELECT tidal_id,is_favorite FROM {table} WHERE id=?1"),
        [local_id],
        |r| Ok((r.get(0)?, r.get::<_, i32>(1)? != 0)),
    )?;
    if was_favorite == favorite {
        return Ok(false);
    }
    let at = now();
    if entity == "track" && favorite {
        conn.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json)
            SELECT 'date_choice',id,id,json_object('date_added',date_added,'library_added_at',library_added_at,
            'library_date_source',library_date_source,'date_choice_at',date_choice_at) FROM tracks WHERE id=?1",[local_id])?;
        conn.execute("UPDATE tracks SET date_added=?2,library_added_at=?2,library_date_source='user',date_choice_at=?3,is_library=1 WHERE id=?1",params![local_id,crate::db::catalogue::canonical_date(&at),at])?;
    }
    // Include outstanding add targets when reversing an action, so an in-flight
    // old add cannot leave a favorite behind after the user requests an unlike.
    let mut targets = std::collections::BTreeSet::new();
    if let Some(id) = selected {
        targets.insert(id);
    }
    if !favorite {
        let mut stmt=conn.prepare(&format!("SELECT tidal_id FROM {aliases} WHERE {fk}=?1 AND is_favorite=1
            UNION SELECT tidal_id FROM tidal_favorite_operations WHERE entity=?2 AND local_id=?1 AND favorite=1"))?;
        for id in stmt.query_map(params![local_id, entity], |r| r.get::<_, i64>(0))? {
            targets.insert(id?);
        }
    }
    conn.execute("INSERT INTO tidal_favorite_intents(entity,local_id,favorite,requested_at) VALUES(?1,?2,?3,?4)
        ON CONFLICT(entity,local_id) DO UPDATE SET favorite=excluded.favorite,revision=revision+1,
        requested_at=excluded.requested_at,completed_at=NULL,confirmed_at=NULL",params![entity,local_id,favorite as i32,at])?;
    let revision: i64 = conn.query_row(
        "SELECT revision FROM tidal_favorite_intents WHERE entity=?1 AND local_id=?2",
        params![entity, local_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "DELETE FROM tidal_favorite_operations WHERE entity=?1 AND local_id=?2",
        params![entity, local_id],
    )?;
    for id in targets {
        conn.execute("INSERT INTO tidal_favorite_operations(entity,local_id,tidal_id,revision,favorite) VALUES(?1,?2,?3,?4,?5)",params![entity,local_id,id,revision,favorite as i32])?;
    }
    conn.execute(
        &format!("UPDATE {table} SET is_favorite=?2 WHERE id=?1"),
        params![local_id, favorite as i32],
    )?;
    if entity == "track" {
        conn.execute(
            "UPDATE tracks SET remote_favorite_state='unresolved' WHERE id=?1",
            [local_id],
        )?;
    }
    Ok(true)
}

fn tables(entity: &str) -> Result<(&'static str, &'static str, &'static str)> {
    match entity {
        "track" => Ok(("tracks", "tidal_track_aliases", "track_id")),
        "album" => Ok(("albums", "tidal_album_aliases", "album_id")),
        _ => anyhow::bail!("Invalid favorite entity"),
    }
}

#[derive(Debug, Clone)]
pub struct Operation {
    pub entity: String,
    pub local_id: i64,
    pub tidal_id: i64,
    pub revision: i64,
    pub favorite: bool,
}

pub fn pending(conn: &Connection) -> Result<Vec<Operation>> {
    let mut stmt = conn.prepare(
        "SELECT o.entity,o.local_id,o.tidal_id,o.revision,o.favorite
        FROM tidal_favorite_operations o JOIN tidal_favorite_intents i USING(entity,local_id)
        WHERE o.done=0 AND o.revision=i.revision AND
        (o.attempted_at IS NULL OR julianday(o.attempted_at)<julianday('now','-1 minute'))
        ORDER BY i.requested_at,o.tidal_id LIMIT 24",
    )?;
    Ok(stmt
        .query_map([], |r| {
            Ok(Operation {
                entity: r.get(0)?,
                local_id: r.get(1)?,
                tidal_id: r.get(2)?,
                revision: r.get(3)?,
                favorite: r.get::<_, i32>(4)? != 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn current(conn: &Connection, op: &Operation) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM tidal_favorite_operations WHERE entity=?1 AND local_id=?2 AND tidal_id=?3 AND revision=?4 AND favorite=?5 AND done=0)",params![op.entity,op.local_id,op.tidal_id,op.revision,op.favorite as i32],|r|r.get(0))?)
}

pub fn finish(conn: &Connection, op: &Operation, error: Option<&str>) -> Result<()> {
    let (table, aliases, fk) = tables(&op.entity)?;
    let at = now();
    if !current(conn, op)? {
        return Ok(());
    }
    conn.execute(
        "UPDATE tidal_favorite_operations SET attempted_at=?5,done=?6,last_error=?7
        WHERE entity=?1 AND local_id=?2 AND tidal_id=?3 AND revision=?4",
        params![
            op.entity,
            op.local_id,
            op.tidal_id,
            op.revision,
            at,
            error.is_none() as i32,
            error
        ],
    )?;
    if error.is_none() {
        conn.execute(
            &format!("UPDATE {aliases} SET is_favorite=?3 WHERE tidal_id=?1 AND {fk}=?2"),
            params![op.tidal_id, op.local_id, op.favorite as i32],
        )?;
        conn.execute("UPDATE tidal_favorite_intents SET completed_at=?3 WHERE entity=?1 AND local_id=?2
            AND NOT EXISTS(SELECT 1 FROM tidal_favorite_operations WHERE entity=?1 AND local_id=?2 AND done=0)",params![op.entity,op.local_id,at])?;
    }
    conn.execute(
        &format!("UPDATE {table} SET is_favorite=?2 WHERE id=?1"),
        params![op.local_id, op.favorite as i32],
    )?;
    Ok(())
}

/// Apply only a complete snapshot. Its start time is the trust boundary: a
/// response fetched before a newer action or completed mutation cannot undo it.
pub fn protect_snapshot(conn: &Connection, entity: &str, started: &str) -> Result<()> {
    let (table, aliases, fk) = tables(entity)?;
    let mut stmt = conn.prepare(
        "SELECT local_id,favorite,revision,requested_at,completed_at,confirmed_at
        FROM tidal_favorite_intents WHERE entity=?1",
    )?;
    let rows = stmt
        .query_map([entity], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    for (id, favorite, revision, requested, completed, confirmed) in rows {
        let start = crate::db::catalogue::timestamp(started);
        let latest = confirmed
            .as_deref()
            .or(completed.as_deref())
            .unwrap_or(&requested);
        let fresh = start
            .zip(crate::db::catalogue::timestamp(latest))
            .is_some_and(|(a, b)| a >= b);
        let observed: bool = conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM {aliases} WHERE {fk}=?1 AND is_favorite=1)"),
            [id],
            |r| r.get(0),
        )?;
        if fresh && confirmed.is_some() {
            // After a confirmed post-action snapshot, a later independent cloud
            // action may take effect. Retain the marker to reject older snapshots.
            let current: bool = conn.query_row(
                &format!("SELECT is_favorite FROM {table} WHERE id=?1"),
                [id],
                |r| r.get(0),
            )?;
            conn.execute("UPDATE tidal_favorite_intents SET favorite=?3,confirmed_at=?4 WHERE entity=?1 AND local_id=?2",params![entity,id,current as i32,started])?;
            continue;
        }
        if fresh && completed.is_some() && observed == favorite {
            conn.execute(
                "UPDATE tidal_favorite_intents SET confirmed_at=?3 WHERE entity=?1 AND local_id=?2",
                params![entity, id, started],
            )?;
        } else if fresh && !favorite && observed {
            // A newly seen favorite alias belongs to the same logical unlike.
            conn.execute(&format!("INSERT INTO tidal_favorite_operations(entity,local_id,tidal_id,revision,favorite)
                SELECT ?1,?2,tidal_id,?3,0 FROM {aliases} WHERE {fk}=?2 AND is_favorite=1
                ON CONFLICT(entity,local_id,tidal_id) DO UPDATE SET done=0,attempted_at=NULL,last_error=NULL"),params![entity,id,revision])?;
            conn.execute("UPDATE tidal_favorite_intents SET completed_at=NULL WHERE entity=?1 AND local_id=?2",params![entity,id])?;
        }
        conn.execute(
            &format!("UPDATE {table} SET is_favorite=?2 WHERE id=?1"),
            params![id, favorite as i32],
        )?;
        if entity == "track" {
            conn.execute(
                "UPDATE tracks SET remote_favorite_state=?2 WHERE id=?1",
                params![
                    id,
                    if completed.is_some() && fresh && observed == favorite {
                        if favorite { "favorite" } else { "not_favorite" }
                    } else {
                        "unresolved"
                    }
                ],
            )?;
        }
    }
    Ok(())
}

pub fn desired(conn: &Connection, id: i64) -> Result<Option<bool>> {
    desired_entity(conn, "track", id)
}

pub fn desired_entity(conn: &Connection, entity: &str, id: i64) -> Result<Option<bool>> {
    if !enabled(conn)? {
        return Ok(None);
    }
    Ok(conn
        .query_row(
            "SELECT favorite FROM tidal_favorite_intents WHERE entity=?1 AND local_id=?2",
            params![entity, id],
            |r| r.get(0),
        )
        .optional()?)
}

/// Move the newest explicit intent before the losing identity is deleted.
pub fn retain_merge(conn: &Connection, entity: &str, kept: i64, removed: i64) -> Result<()> {
    if !enabled(conn)? {
        return Ok(());
    }
    let winner:Option<i64>=conn.query_row("SELECT local_id FROM tidal_favorite_intents WHERE entity=?1 AND local_id IN (?2,?3) ORDER BY requested_at DESC LIMIT 1",params![entity,kept,removed],|r|r.get(0)).optional()?;
    if winner == Some(removed) {
        conn.execute(
            "DELETE FROM tidal_favorite_intents WHERE entity=?1 AND local_id=?2",
            params![entity, kept],
        )?;
        conn.execute("INSERT INTO tidal_favorite_intents SELECT entity,?2,favorite,revision,requested_at,completed_at,confirmed_at FROM tidal_favorite_intents WHERE entity=?1 AND local_id=?3",params![entity,kept,removed])?;
        conn.execute("INSERT INTO tidal_favorite_operations SELECT entity,?2,tidal_id,revision,favorite,done,attempted_at,last_error FROM tidal_favorite_operations WHERE entity=?1 AND local_id=?3",params![entity,kept,removed])?;
    }
    conn.execute(
        "DELETE FROM tidal_favorite_intents WHERE entity=?1 AND local_id=?2",
        params![entity, removed],
    )?;
    Ok(())
}
