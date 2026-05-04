use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::services::crypto::MasterKey;

/// Last.fm credentials persisted in `service_auth` for `service = 'lastfm'`.
///
/// Layout:
/// - `extra_data` JSON  → `api_key`, optional `session_user`, optional
///   short-lived `pending_token`. `api_key` is user-provided and not a secret
///   we own; `session_user` is just a username; `pending_token` is a one-time
///   challenge that's cleared on completion/disconnect/cancel.
/// - `access_token_enc` BLOB → AES-256-GCM-encrypted `session_key` (the actual
///   secret), see `services/crypto.rs`. Read/written via `load_session_key` /
///   `save_session_key` — NEVER stuffed into `extra_data` JSON.
///
/// IMPORTANT: do not add `api_secret` or `session_key` fields to this struct.
/// `api_secret` is server-only env (`LASTFM_API_SECRET`); `session_key` lives
/// in the encrypted blob column.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastFmCredentials {
    pub api_key: String,
    /// Last.fm username for the account that completed scrobble auth.
    /// Cleared when the user disconnects scrobbling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_user: Option<String>,
    /// Short-lived `auth.getToken` value, set by `/api/lastfm/auth/start` and
    /// cleared by `/auth/complete` / `/auth/disconnect`. Plaintext is fine:
    /// the token is only useful for ~60 minutes and is paired with the
    /// server-only `api_secret` to be redeemed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_token: Option<String>,
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

/// Decrypt and return the stored Last.fm scrobble session key, if any.
pub fn load_session_key(conn: &Connection, master: &MasterKey) -> Result<Option<String>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT access_token_enc FROM service_auth WHERE service = 'lastfm'",
            [],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten();
    let Some(blob) = blob else { return Ok(None) };
    if blob.is_empty() {
        return Ok(None);
    }
    let plain = master.decrypt(&blob)?;
    Ok(Some(String::from_utf8(plain)?))
}

/// Encrypt and persist the Last.fm session key. Also updates `session_user`
/// in the `extra_data` JSON, preserving any existing `api_key`.
pub fn save_session_key(
    conn: &Connection,
    master: &MasterKey,
    session_key: &str,
    session_user: &str,
) -> Result<()> {
    let mut creds = load_credentials(conn)?.unwrap_or_default();
    creds.session_user = Some(session_user.to_string());
    creds.pending_token = None;
    let json = serde_json::to_string(&creds)?;
    let blob = master.encrypt(session_key.as_bytes())?;
    conn.execute(
        "INSERT INTO service_auth (service, extra_data, access_token_enc, user_id, connected_at)
         VALUES ('lastfm', ?1, ?2, 'app', datetime('now'))
         ON CONFLICT(service) DO UPDATE SET
             extra_data       = excluded.extra_data,
             access_token_enc = excluded.access_token_enc",
        params![json, blob],
    )?;
    Ok(())
}

/// Clear the encrypted session key + session_user. Leaves `api_key` in place
/// so tag enrichment + new-releases reads keep working.
pub fn clear_session(conn: &Connection) -> Result<()> {
    let mut creds = load_credentials(conn)?.unwrap_or_default();
    creds.session_user = None;
    creds.pending_token = None;
    let json = serde_json::to_string(&creds)?;
    conn.execute(
        "UPDATE service_auth
            SET extra_data = ?1,
                access_token_enc = NULL
          WHERE service = 'lastfm'",
        params![json],
    )?;
    Ok(())
}

/// Stash the `auth.getToken` value while the user authorizes in their browser.
/// Cleared when /auth/complete or /auth/disconnect runs, or when the user
/// explicitly cancels.
pub fn set_pending_token(conn: &Connection, token: &str) -> Result<()> {
    let mut creds = load_credentials(conn)?.unwrap_or_default();
    creds.pending_token = Some(token.to_string());
    save_credentials(conn, &creds)
}

pub fn clear_pending_token(conn: &Connection) -> Result<()> {
    let mut creds = load_credentials(conn)?.unwrap_or_default();
    creds.pending_token = None;
    save_credentials(conn, &creds)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_in_memory_db_with_schema() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE service_auth (
                service TEXT PRIMARY KEY,
                access_token_enc BLOB,
                refresh_token_enc BLOB,
                token_expiry TEXT,
                user_id TEXT,
                subscription_type TEXT,
                extra_data TEXT,
                connected_at TEXT DEFAULT (datetime('now'))
            )",
            [],
        )
        .unwrap();
        conn
    }

    fn temp_master_key() -> MasterKey {
        let mut p = std::env::temp_dir();
        p.push(format!("noor-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).unwrap();
        MasterKey::load_or_generate(&p).unwrap()
    }

    /// Documents the env-only contract for `api_secret`. If anyone ever adds
    /// an `api_secret` field to `LastFmCredentials`, this test starts failing
    /// loudly. The plan + the user spec say `api_secret` is server-only env
    /// and must never round-trip through this struct (which is JSON-serialized
    /// into `service_auth.extra_data`).
    #[test]
    fn no_secret_in_struct() {
        let creds = LastFmCredentials {
            api_key: "abc".into(),
            session_user: Some("user".into()),
            pending_token: Some("tok".into()),
        };
        let json = serde_json::to_value(&creds).unwrap();
        let obj = json.as_object().unwrap();
        assert!(
            !obj.contains_key("api_secret"),
            "api_secret must never be serialized into LastFmCredentials"
        );
        assert!(
            !obj.contains_key("session_key"),
            "session_key must never be serialized into LastFmCredentials JSON \
             (it lives in the encrypted access_token_enc blob)"
        );
    }

    /// The plan's required regression: the raw stored bytes for the session
    /// key must NOT contain the plaintext anywhere. This exercises the full
    /// save → SELECT-raw-blob round-trip.
    #[test]
    fn session_key_encrypted() {
        let conn = open_in_memory_db_with_schema();
        let master = temp_master_key();
        let secret = "Y0u_w0nt_f1nd_th1s_1n_th3_b1n";
        save_session_key(&conn, &master, secret, "alice").unwrap();

        let raw: Vec<u8> = conn
            .query_row(
                "SELECT access_token_enc FROM service_auth WHERE service = 'lastfm'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!raw.is_empty(), "blob must not be empty");
        let secret_bytes = secret.as_bytes();
        assert!(
            !raw.windows(secret_bytes.len()).any(|w| w == secret_bytes),
            "raw session_key blob leaked plaintext"
        );

        // Also ensure session_user (non-secret) IS readable, but session_key
        // and pending_token are NOT in the JSON.
        let extra: String = conn
            .query_row(
                "SELECT extra_data FROM service_auth WHERE service = 'lastfm'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(extra.contains("\"alice\""), "session_user should be in extra_data");
        assert!(
            !extra.contains(secret),
            "session_key must never be stored in extra_data JSON"
        );

        // Round-trip read must yield the original secret.
        let back = load_session_key(&conn, &master).unwrap();
        assert_eq!(back.as_deref(), Some(secret));
    }

    #[test]
    fn clear_session_preserves_api_key() {
        let conn = open_in_memory_db_with_schema();
        let master = temp_master_key();
        save_credentials(
            &conn,
            &LastFmCredentials {
                api_key: "the-api-key".into(),
                ..Default::default()
            },
        )
        .unwrap();
        save_session_key(&conn, &master, "sk-xyz", "alice").unwrap();
        clear_session(&conn).unwrap();
        let creds = load_credentials(&conn).unwrap().unwrap();
        assert_eq!(creds.api_key, "the-api-key");
        assert!(creds.session_user.is_none());
        assert!(load_session_key(&conn, &master).unwrap().is_none());
    }
}
