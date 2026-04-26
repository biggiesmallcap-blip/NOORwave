use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastFmCredentials {
    pub api_key: String,
}

pub fn load_credentials(conn: &Connection) -> Result<Option<LastFmCredentials>> {
    let row: rusqlite::Result<Option<String>> = conn.query_row(
        "SELECT extra_data FROM service_auth WHERE service = 'lastfm'",
        [],
        |row| row.get(0),
    );
    match row {
        Ok(Some(json)) => Ok(serde_json::from_str(&json).ok()),
        _ => Ok(None),
    }
}

pub fn save_credentials(conn: &Connection, creds: &LastFmCredentials) -> Result<()> {
    let json = serde_json::to_string(creds)?;
    conn.execute(
        "INSERT INTO service_auth (service, extra_data, user_id, connected_at)
         VALUES ('lastfm', ?1, 'app', datetime('now'))
         ON CONFLICT(service) DO UPDATE SET extra_data = excluded.extra_data",
        params![json],
    )?;
    Ok(())
}

pub fn clear_credentials(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM service_auth WHERE service = 'lastfm'", [])?;
    Ok(())
}
