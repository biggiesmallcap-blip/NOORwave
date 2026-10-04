//! First-run guidance is scoped to this database and TIDAL account, never an API token.
//! Callers hold Database::with_conn's mutex across each read/modify/write.
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize, Serialize)]
struct Guidance {
    ready: bool,
    shown: bool,
    owner: Option<String>,
    expires_at: i64,
}

#[derive(Serialize)]
pub struct Status {
    pub library_id: String,
    pub account_id: Option<String>,
    pub eligible: bool,
}

fn read(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM server_config WHERE key=?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

fn write(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config(key,value) VALUES(?1,?2)",
        params![key, value],
    )?;
    Ok(())
}

fn account_key(account: &str) -> String {
    format!("discovery_setup.account.{account}")
}
fn guidance(conn: &Connection, account: &str) -> Result<Guidance> {
    Ok(read(conn, &account_key(account))?
        .map(|value| serde_json::from_str(&value))
        .transpose()?
        .unwrap_or_default())
}
fn save(conn: &Connection, account: &str, value: &Guidance) -> Result<()> {
    write(conn, &account_key(account), &serde_json::to_string(value)?)
}

/// Only new installations enter this guidance flow. Existing libraries stay quiet.
pub fn enroll(conn: &Connection) -> Result<()> {
    write(conn, "discovery_setup.enrolled", "1")
}

/// Only invoke after an actual successful TIDAL sync, not LibrarySynced broadcasts.
pub fn record_successful_sync(conn: &Connection, account: &str) -> Result<()> {
    if read(conn, "discovery_setup.enrolled")?.as_deref() != Some("1")
        || super::queries::get_track_count(conn, true, false)? == 0
    {
        return Ok(());
    }
    let mut value = guidance(conn, account)?;
    value.ready = true;
    save(conn, account, &value)
}

pub fn status(conn: &Connection, account: Option<&str>) -> Result<Status> {
    let library_id = match read(conn, "discovery_setup.library_id")? {
        Some(id) => id,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            write(conn, "discovery_setup.library_id", &id)?;
            id
        }
    };
    let completed = read(conn, "onboarding_complete")?.as_deref() == Some("1");
    let enrolled = read(conn, "discovery_setup.enrolled")?.as_deref() == Some("1");
    // Starting training already expresses intent; don't interrupt those users.
    let trained: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM training_runs)", [], |r| {
        r.get(0)
    })?;
    let eligible = if let Some(account) = account {
        let value = guidance(conn, account)?;
        completed && enrolled && value.ready && !value.shown && !trained
    } else {
        false
    };
    Ok(Status {
        library_id,
        account_id: account.map(str::to_owned),
        eligible,
    })
}

/// A short reservation prevents simultaneous prompts in different tabs/devices.
/// Failed mounts expire without consuming the one-time guidance.
pub fn reserve(conn: &Connection, account: &str, owner: &str, now: i64) -> Result<bool> {
    if !status(conn, Some(account))?.eligible {
        return Ok(false);
    }
    let mut value = guidance(conn, account)?;
    if value
        .owner
        .as_deref()
        .is_some_and(|existing| existing != owner)
        && value.expires_at > now
    {
        return Ok(false);
    }
    value.owner = Some(owner.to_owned());
    value.expires_at = now + 60;
    save(conn, account, &value)?;
    Ok(true)
}

pub fn acknowledge(conn: &Connection, account: &str, owner: &str, now: i64) -> Result<bool> {
    let mut value = guidance(conn, account)?;
    if value.shown {
        return Ok(true);
    }
    if value.owner.as_deref() != Some(owner) || value.expires_at <= now {
        return Ok(false);
    }
    value.shown = true;
    value.owner = None;
    save(conn, account, &value)?;
    Ok(true)
}

pub fn release(conn: &Connection, account: &str, owner: &str) -> Result<()> {
    let mut value = guidance(conn, account)?;
    if value.owner.as_deref() == Some(owner) {
        value.owner = None;
        value.expires_at = 0;
        save(conn, account, &value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        super::super::schema::run_migrations(&conn).unwrap();
        conn
    }
    fn nonempty_sync(conn: &Connection, account: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO artists(id,name) VALUES(1,'Artist')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO albums(id,title,artist_id) VALUES(1,'Album',1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT OR IGNORE INTO tracks(id,title,artist_id,album_id,is_favorite) VALUES(1,'Song',1,1,1)", []).unwrap();
        record_successful_sync(conn, account).unwrap();
    }
    #[test]
    fn only_enrolled_completed_nonempty_sync_is_eligible() {
        let conn = database();
        assert!(!status(&conn, Some("a")).unwrap().eligible);
        nonempty_sync(&conn, "a");
        super::super::queries::set_onboarding_complete(&conn).unwrap();
        assert!(!status(&conn, Some("a")).unwrap().eligible); // An upgrade isn't enrolled.
        enroll(&conn).unwrap();
        conn.execute("DELETE FROM tracks", []).unwrap();
        record_successful_sync(&conn, "a").unwrap();
        assert!(!status(&conn, Some("a")).unwrap().eligible);
        nonempty_sync(&conn, "a");
        assert!(status(&conn, Some("a")).unwrap().eligible);
        assert!(!status(&conn, Some("b")).unwrap().eligible);
        assert!(!status(&conn, None).unwrap().eligible);
        assert_eq!(
            status(&conn, Some("a")).unwrap().library_id,
            status(&conn, Some("b")).unwrap().library_id
        );
        assert_ne!(
            status(&conn, Some("a")).unwrap().library_id,
            status(&database(), Some("a")).unwrap().library_id
        );
    }
    #[test]
    fn reservation_expires_without_consuming_guidance_and_shown_is_durable() {
        let conn = database();
        enroll(&conn).unwrap();
        super::super::queries::set_onboarding_complete(&conn).unwrap();
        nonempty_sync(&conn, "a");
        assert!(reserve(&conn, "a", "tab1", 100).unwrap());
        assert!(!reserve(&conn, "a", "tab2", 101).unwrap());
        assert!(!acknowledge(&conn, "a", "tab2", 101).unwrap());
        release(&conn, "a", "tab2").unwrap();
        assert!(!reserve(&conn, "a", "tab2", 102).unwrap());
        assert!(reserve(&conn, "a", "tab2", 161).unwrap());
        assert!(!acknowledge(&conn, "a", "tab1", 162).unwrap());
        assert!(acknowledge(&conn, "a", "tab2", 162).unwrap());
        nonempty_sync(&conn, "a");
        assert!(!status(&conn, Some("a")).unwrap().eligible);
    }
    #[test]
    fn previous_training_suppresses_guidance_even_when_it_failed() {
        let conn = database();
        enroll(&conn).unwrap();
        super::super::queries::set_onboarding_complete(&conn).unwrap();
        nonempty_sync(&conn, "a");
        conn.execute(
            "INSERT INTO training_runs(stage,status) VALUES('full','failed')",
            [],
        )
        .unwrap();
        assert!(!status(&conn, Some("a")).unwrap().eligible);
    }
}
