//! Server config key/value reads and writes.

use super::*;

// ─── Server Config ────────────────────────────────────────

pub fn ensure_server_token(conn: &Connection) -> Result<String> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='server_token'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    // Keep the existing token only if it matches the current 6-digit PIN format.
    // Legacy hex/word-phrase tokens are auto-upgraded on next startup.
    if let Some(token) = existing
        && is_valid_pin(&token)
    {
        return Ok(token);
    }

    let token = generate_readable_token();
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('server_token', ?1)",
        params![token],
    )?;
    Ok(token)
}

pub(super) fn is_valid_pin(s: &str) -> bool {
    s.len() == 6 && s.chars().all(|c| c.is_ascii_digit())
}

/// First-run onboarding flag. When the row is missing, treat an existing
/// `service_auth` TIDAL row as implicit completion and persist the flag —
/// this keeps users upgrading from earlier versions out of the onboarding
/// flow they've effectively already done.
pub fn get_onboarding_complete(conn: &Connection) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key='onboarding_complete'",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(value) = stored {
        return Ok(value == "1");
    }

    let has_tidal: bool = conn
        .query_row(
            "SELECT 1 FROM service_auth WHERE service='tidal' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .optional()?
        .is_some();

    if has_tidal {
        set_onboarding_complete(conn)?;
        return Ok(true);
    }

    // Persist first-run intent before TIDAL connects so a reload cannot mistake
    // an unfinished setup for a legacy installation with an existing account.
    conn.execute(
        "INSERT OR IGNORE INTO server_config(key,value) VALUES('onboarding_complete','0')",
        [],
    )?;
    crate::db::discovery_setup::enroll(conn)?;
    Ok(false)
}

pub fn set_onboarding_complete(conn: &Connection) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO server_config (key, value) VALUES ('onboarding_complete', '1')",
        [],
    )?;
    Ok(())
}

pub(super) fn generate_readable_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 4];
    rand::rng().fill_bytes(&mut bytes);
    let n = u32::from_le_bytes(bytes) % 1_000_000;
    format!("{:06}", n)
}
