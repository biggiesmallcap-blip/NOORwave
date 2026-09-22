use crate::db::Database;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

pub const MAX_REMOTE_DEVICES: i64 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteConfig {
    pub server_id: String,
    pub hostname: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDeviceRow {
    pub id: String,
    pub name: String,
    pub token_hash: [u8; 32],
    pub paired_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum CreateDeviceError {
    #[error("remote device capacity reached")]
    Capacity,
    #[error(transparent)]
    Storage(#[from] anyhow::Error),
}

pub fn initialize(db: &Database) -> Result<RemoteConfig> {
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let existing_id: Option<String> = tx
            .query_row(
                "SELECT value FROM server_config WHERE key = 'remote.server_id'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let server_id = match existing_id {
            Some(value) => {
                Uuid::parse_str(&value).context("invalid persisted remote.server_id")?;
                value
            }
            None => {
                let value = Uuid::new_v4().to_string();
                tx.execute(
                    "INSERT INTO server_config (key, value) VALUES ('remote.server_id', ?1)",
                    [&value],
                )?;
                value
            }
        };

        let existing_hostname: Option<String> = tx
            .query_row(
                "SELECT value FROM server_config WHERE key = 'remote.hostname'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let hostname = match existing_hostname {
            Some(value) if valid_hostname(&value) => value,
            Some(_) => bail!("invalid persisted remote.hostname"),
            None => {
                let value = "noorwave.local.".to_string();
                tx.execute(
                    "INSERT INTO server_config (key, value) VALUES ('remote.hostname', ?1)",
                    [&value],
                )?;
                value
            }
        };
        tx.commit()?;
        Ok(RemoteConfig {
            server_id,
            hostname,
        })
    })
}

fn valid_hostname(value: &str) -> bool {
    if value != value.to_ascii_lowercase() || !value.ends_with(".local.") || value.len() > 254 {
        return false;
    }
    value.trim_end_matches('.').split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

pub fn persist_hostname(db: &Database, hostname: &str) -> Result<()> {
    if !valid_hostname(hostname) {
        bail!("invalid remote hostname");
    }
    db.with_conn(|conn| {
        let changed = conn.execute(
            "UPDATE server_config SET value = ?1 WHERE key = 'remote.hostname'",
            [hostname],
        )?;
        if changed != 1 {
            bail!("remote hostname configuration is missing");
        }
        Ok(())
    })
}

pub fn load_devices(db: &Database) -> Result<Vec<RemoteDeviceRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, token_hash, paired_at, last_seen_at
             FROM remote_devices ORDER BY paired_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let hash: Vec<u8> = row.get(2)?;
                let token_hash: [u8; 32] = hash.try_into().map_err(|_| {
                    rusqlite::Error::InvalidColumnType(
                        2,
                        "token_hash".into(),
                        rusqlite::types::Type::Blob,
                    )
                })?;
                let paired_at: String = row.get(3)?;
                let last_seen_at: Option<String> = row.get(4)?;
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    token_hash,
                    paired_at,
                    last_seen_at,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, name, token_hash, paired_at, last_seen_at)| {
                Ok(RemoteDeviceRow {
                    id,
                    name,
                    token_hash,
                    paired_at: DateTime::parse_from_rfc3339(&paired_at)?.with_timezone(&Utc),
                    last_seen_at: last_seen_at
                        .map(|value| {
                            DateTime::parse_from_rfc3339(&value).map(|v| v.with_timezone(&Utc))
                        })
                        .transpose()?,
                })
            })
            .collect()
    })
}

pub fn create_device(
    db: &Database,
    row: &RemoteDeviceRow,
) -> std::result::Result<(), CreateDeviceError> {
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM remote_devices", [], |r| r.get(0))?;
        if count >= MAX_REMOTE_DEVICES {
            bail!("remote device capacity reached");
        }
        tx.execute(
            "INSERT INTO remote_devices (id, name, token_hash, paired_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                row.id,
                row.name,
                row.token_hash.as_slice(),
                row.paired_at.to_rfc3339(),
                row.last_seen_at.map(|v| v.to_rfc3339()),
            ],
        )?;
        tx.commit()?;
        Ok(())
    })
    .map_err(|error| {
        if error.to_string() == "remote device capacity reached" {
            CreateDeviceError::Capacity
        } else {
            CreateDeviceError::Storage(error)
        }
    })
}

pub fn rename_device(db: &Database, id: &str, name: &str) -> Result<bool> {
    db.with_conn(|conn| {
        Ok(conn.execute(
            "UPDATE remote_devices SET name = ?1 WHERE id = ?2",
            params![name, id],
        )? == 1)
    })
}

pub fn revoke_device(db: &Database, id: &str) -> Result<bool> {
    db.with_conn(|conn| Ok(conn.execute("DELETE FROM remote_devices WHERE id = ?1", [id])? == 1))
}

pub fn touch_last_seen(db: &Database, id: &str, now: DateTime<Utc>) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "UPDATE remote_devices SET last_seen_at = ?1 WHERE id = ?2",
            params![now.to_rfc3339(), id],
        )?;
        Ok(())
    })
}

pub fn reset_all(db: &Database, new_pin: &str) -> Result<usize> {
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let count = tx.execute("DELETE FROM remote_devices", [])?;
        tx.execute(
            "INSERT INTO server_config (key, value) VALUES ('server_token', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [new_pin],
        )?;
        tx.commit()?;
        Ok(count)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.with_conn(schema::run_migrations).unwrap();
        db
    }

    #[test]
    fn identity_is_initialized_once_and_device_secrets_are_digest_only() {
        let db = db();
        let first = initialize(&db).unwrap();
        let second = initialize(&db).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.hostname, "noorwave.local.");
        Uuid::parse_str(&first.server_id).unwrap();

        let digest = [7_u8; 32];
        create_device(
            &db,
            &RemoteDeviceRow {
                id: Uuid::new_v4().to_string(),
                name: "Test phone".into(),
                token_hash: digest,
                paired_at: Utc::now(),
                last_seen_at: None,
            },
        )
        .unwrap();
        assert_eq!(load_devices(&db).unwrap()[0].token_hash, digest);
        let schema_sql: String = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='remote_devices'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert!(!schema_sql.contains("token TEXT"));
    }

    #[test]
    fn conflict_hostname_is_validated_persisted_and_reloaded() {
        let db = db();
        initialize(&db).unwrap();

        persist_hostname(&db, "noorwave-2.local.").unwrap();
        assert_eq!(initialize(&db).unwrap().hostname, "noorwave-2.local.");

        assert!(persist_hostname(&db, "NOORwave.local.").is_err());
        assert_eq!(initialize(&db).unwrap().hostname, "noorwave-2.local.");
    }
}
