//! Reviewed historical dates travel with a patch as a local manifest. Recovery
//! runs before sync, never favorites anything, and rejects intervening choices.
use super::{Database, catalogue};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::path::Path;

const MANIFEST_NAME: &str = "tidal-catalogue-recovery.json";
const MAX_MANIFEST_BYTES: u64 = 4 * 1024 * 1024;

/// Optional patch payload beside the executable, or a previously queued payload
/// beside the database. No manifest means ordinary migration/sync only.
pub fn consume_pending(db: &Database) -> Result<usize> {
    let mut paths = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        paths.push(parent.join(MANIFEST_NAME));
    }
    if let Some(path) = db.path()
        && let Some(parent) = path.parent()
    {
        paths.push(parent.join(MANIFEST_NAME));
    }
    paths.dedup();
    let packaged: Value = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/tidal-recovery.json"
    )))?;
    let mut count = if packaged.is_null() {
        0
    } else {
        consume_payload(db, &packaged)?
    };
    for path in paths {
        if !path.is_file() {
            continue;
        }
        if std::fs::metadata(&path)?.len() > MAX_MANIFEST_BYTES {
            bail!("Recovery manifest is too large");
        }
        let manifest: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        count += consume_payload(db, &manifest)?;
    }
    Ok(count)
}

fn consume_payload(db: &Database, manifest: &Value) -> Result<usize> {
    let backup = db
        .path()
        .context("Recovery needs a file-backed database")?
        .with_extension(format!("recovery-{}.db", uuid::Uuid::new_v4()));
    db.with_conn(|conn| apply(conn, manifest, &backup))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing recovery field: {key}"))
}
fn fold(value: &str) -> String {
    value
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
fn same_instant(a: Option<&str>, b: Option<&str>) -> bool {
    a == b
        || a.and_then(catalogue::timestamp)
            .zip(b.and_then(catalogue::timestamp))
            .is_some_and(|(a, b)| a == b)
}
fn marker(batch: &str, id: i64) -> String {
    // Matches Python's stable json.dumps(..., sort_keys=True) marker.
    format!(
        "{{\"batch_id\": {}, \"historical_id\": {}}}",
        serde_json::to_string(batch).unwrap(),
        id
    )
}

/// A single atomic batch. Validation and repairs roll back together. SQLite's
/// consistent snapshot for the backup, including committed WAL transactions.
pub fn apply(conn: &Connection, manifest: &Value, backup: &Path) -> Result<usize> {
    if manifest["version"] != 2 || manifest["changes_favorites"] != false {
        bail!("Reviewed version-2 recovery required");
    }
    let batch = text(manifest, "batch_id")?;
    if batch.len() != 64 || !batch.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Invalid recovery batch ID");
    }
    let reviewed = text(manifest, "reviewed_at")?;
    if catalogue::timestamp(reviewed).is_none() {
        bail!("Invalid recovery review date");
    }
    let entries = manifest["entries"]
        .as_array()
        .context("Recovery entries required")?;
    let mut pending = Vec::new();
    for item in entries {
        if item["status"] != "reviewed" {
            bail!("Unreviewed recovery entry");
        }
        let old_id = item["old"]["tidal_id"]
            .as_i64()
            .context("Historical ID required")?;
        let marker = marker(batch, old_id);
        let applied:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM catalogue_merge_audit WHERE entity='recovery' AND json_extract(snapshot_json,'$.marker')=?1)",[&marker],|r|r.get(0))?;
        if !applied {
            pending.push((item, old_id, marker));
        }
    }
    if pending.is_empty() {
        return Ok(0);
    }
    if backup.exists() {
        bail!("Recovery backup must be a new file");
    }
    // Reject stale or unrelated batches before a potentially large backup.
    // Revalidate inside the transaction to catch changes from other processes.
    for (item, old_id, _) in &pending {
        validate_entry(conn, item, *old_id)?;
    }
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()])?;
    let tx = conn.unchecked_transaction()?;
    for (item, old_id, marker) in &pending {
        let (id, row, date) = validate_entry(&tx, item, *old_id)?;
        let old = &item["old"];
        tx.execute("INSERT INTO catalogue_merge_audit(entity,kept_id,removed_id,snapshot_json) VALUES('recovery',?1,?2,?3)",params![id,old_id,serde_json::json!({"marker":marker,"before":row,"historical":old,"sources":item["sources"]})])?;
        tx.execute("INSERT OR IGNORE INTO tidal_track_aliases(tidal_id,track_id,evidence,favorite_created) VALUES(?1,?2,'historical_backup',?3)",params![old_id,id,text(old,"date_added")?])?;
        tx.execute("UPDATE tracks SET date_added=?2,library_added_at=?2,library_date_source='recovered',date_choice_at=?3 WHERE id=?1",params![id,date,reviewed])?;
    }
    tx.commit()?;
    Ok(pending.len())
}

fn validate_entry(conn: &Connection, item: &Value, old_id: i64) -> Result<(i64, Value, String)> {
    let expected = &item["current"];
    let id = expected["id"]
        .as_i64()
        .context("Local recovery ID required")?;
    let row:Option<Value>=conn.query_row("SELECT json_object('tidal_id',t.tidal_id,'title',t.title,'isrc',t.isrc,'duration_ms',t.duration_ms,
            'artist',a.name,'date_added',t.date_added,'library_added_at',t.library_added_at,'library_date_source',t.library_date_source,
            'date_choice_at',t.date_choice_at,'catalogue_version',t.catalogue_version,'catalogue_explicit',t.catalogue_explicit,
            'intent_revision',(SELECT revision FROM tidal_favorite_intents WHERE entity='track' AND local_id=t.id),
            'is_favorite',t.is_favorite,'is_library',t.is_library)
            FROM tracks t JOIN artists a ON a.id=t.artist_id WHERE t.id=?1",[id],|r|r.get(0)).optional()?;
    let row = row.context("Recording disappeared since review")?;
    let old = &item["old"];
    let isrc = text(old, "isrc")?.trim().to_uppercase();
    let a = old["duration_ms"].as_i64().unwrap_or(0);
    let b = row["duration_ms"].as_i64().unwrap_or(0);
    let gap = a.abs_diff(b);
    if row["tidal_id"] != expected["tidal_id"]
        || isrc.is_empty()
        || isrc != text(&row, "isrc")?.trim().to_uppercase()
        || fold(text(old, "title")?) != fold(text(&row, "title")?)
        || fold(text(old, "artist")?) != fold(text(&row, "artist")?)
        || fold(old["catalogue_version"].as_str().unwrap_or(""))
            != fold(row["catalogue_version"].as_str().unwrap_or(""))
        || old["catalogue_explicit"] != row["catalogue_explicit"]
        || a <= 0
        || b <= 0
        || gap > 15000
        || gap * 100 > a.max(b) as u64 * 8
    {
        bail!("Recording identity changed since review");
    }
    let baseline = &item["expected"];
    for key in ["date_added", "library_added_at", "date_choice_at"] {
        let expected = if key == "library_added_at" && baseline[key].is_null() {
            baseline["date_added"].as_str()
        } else {
            baseline[key].as_str()
        };
        if !same_instant(row[key].as_str(), expected) {
            bail!("Date choice changed since review");
        }
    }
    if row["intent_revision"] != baseline["intent_revision"]
        || (row["library_date_source"] != baseline["library_date_source"]
            && !(baseline["library_date_source"].is_null()
                && row["library_date_source"] == "legacy"))
    {
        bail!("User intent changed since review");
    }
    let accepted = text(item, "accepted_date")?;
    let instant = catalogue::timestamp(accepted).context("Invalid accepted date")?;
    if Some(instant) != catalogue::timestamp(text(old, "date_added")?)
        || Some(instant) >= catalogue::timestamp(text(&row, "date_added")?)
    {
        bail!("Recovery date must be older historical evidence");
    }
    let owner: Option<i64> = conn
        .query_row(
            "SELECT track_id FROM tidal_track_aliases WHERE tidal_id=?1",
            [old_id],
            |r| r.get(0),
        )
        .optional()?;
    if owner.is_some_and(|owner| owner != id) {
        bail!("Historical alias changed owner");
    }

    let date = catalogue::canonical_date(accepted).context("Invalid recovery date")?;
    Ok((id, row, date))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> (Database, Value) {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|c| {c.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist');
            INSERT INTO tracks(id,tidal_id,title,artist_id,isrc,duration_ms,date_added,library_added_at,library_date_source,is_library,is_favorite,source)
            VALUES(1,20,'Song',1,'ISRC1',180000,'2026-09-18T07:37:31Z','2026-09-18T07:37:31Z','legacy',1,1,'tidal');")?;Ok(())}).unwrap();
        let manifest = json!({"version":2,"batch_id":"a".repeat(64),"reviewed_at":"2026-10-06T01:00:00Z","changes_favorites":false,"entries":[{
            "status":"reviewed","old":{"tidal_id":10,"title":"Song","artist":"Artist","isrc":"ISRC1","duration_ms":180000,"date_added":"2020-06-10T07:10:44.221+0000","catalogue_version":null,"catalogue_explicit":null},
            "current":{"id":1,"tidal_id":20},"expected":{"date_added":"2026-09-18 07:37:31","library_added_at":null,"library_date_source":null,"date_choice_at":null,"intent_revision":null},
            "accepted_date":"2020-06-10T07:10:44.221000Z","sources":["historical.db"]}]});
        (db, manifest)
    }
    fn backup_path() -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/catalogue-recovery/tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{}.db", uuid::Uuid::new_v4()))
    }
    #[test]
    #[ignore = "Requires a disposable whole-library snapshot and reviewed payload in target/catalogue-recovery/patch-validation"]
    fn catalogue_recovery_whole_library_patch_consumption() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/catalogue-recovery/patch-validation");
        let database = root.join("noor.db");
        assert!(
            database.is_file(),
            "Create the disposable validation snapshot first"
        );
        let db = Database::open(&database).unwrap();
        db.run_migrations().unwrap();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.join(MANIFEST_NAME)).unwrap()).unwrap();
        let packaged: Value = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/tidal-recovery.json"
        )))
        .unwrap();
        if std::env::var_os("NOOR_CATALOGUE_RECOVERY_MANIFEST").is_some() {
            assert_eq!(
                packaged, manifest,
                "The reviewed payload must travel inside the patched binary"
            );
        }
        let entries = manifest["entries"].as_array().unwrap();
        let baseline=db.with_conn(|c|c.query_row("SELECT json_group_array(json_object('id',id,'tidal_id',tidal_id,'favorite',is_favorite,'library',is_library)) FROM tracks",[],|r|r.get::<_,String>(0)).map_err(Into::into)).unwrap();
        assert_eq!(consume_pending(&db).unwrap(), entries.len());
        assert_eq!(consume_pending(&db).unwrap(), 0);
        db.with_conn(|c| {
            let after:String=c.query_row("SELECT json_group_array(json_object('id',id,'tidal_id',tidal_id,'favorite',is_favorite,'library',is_library)) FROM tracks",[],|r|r.get(0))?;
            assert_eq!(baseline,after);
            for item in entries {
                let id=item["current"]["id"].as_i64().unwrap();
                catalogue::curate(c,id,true,true,Some("2099-01-01T00:00:00Z"))?;
                let date:String=c.query_row("SELECT date_added FROM tracks WHERE id=?1",[id],|r|r.get(0))?;
                assert_eq!(catalogue::timestamp(&date),catalogue::timestamp(item["accepted_date"].as_str().unwrap()));
            }
            Ok(())
        }).unwrap();
    }

    #[test]
    fn catalogue_recovery_patch_payload_preserves_flags_and_survives_sync_and_relike() {
        let (db, manifest) = fixture();
        let backup = backup_path();
        db.with_conn(|c| {
            assert_eq!(apply(c, &manifest, &backup)?, 1);
            catalogue::curate(c, 1, true, true, Some("2099-01-01T00:00:00Z"))?;
            assert_eq!(
                c.query_row("SELECT date_added FROM tracks", [], |r| r
                    .get::<_, String>(0))?,
                "2020-06-10T07:10:44.221Z"
            );
            crate::db::catalogue_favorites::request(c, "track", 1, false)?;
            crate::db::catalogue_favorites::request(c, "track", 1, true)?;
            let chosen: String = c.query_row("SELECT date_added FROM tracks", [], |r| r.get(0))?;
            assert_eq!(apply(c, &manifest, &backup_path())?, 0);
            catalogue::curate(c, 1, true, true, Some("2020-01-01T00:00:00Z"))?;
            assert_eq!(
                c.query_row("SELECT date_added FROM tracks", [], |r| r
                    .get::<_, String>(0))?,
                chosen
            );
            assert_eq!(
                c.query_row(
                    "SELECT is_favorite,is_library,tidal_id FROM tracks",
                    [],
                    |r| Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?
                    ))
                )?,
                (1, 1, 20)
            );
            Ok(())
        })
        .unwrap();
        std::fs::remove_file(backup).unwrap();
    }
    #[test]
    fn catalogue_recovery_stale_choice_and_late_conflict_roll_back_batch() {
        for stale in [false, true] {
            let (db, mut manifest) = fixture();
            let backup = backup_path();
            db.with_conn(|c| {
                if stale {
                    crate::db::catalogue_favorites::request(c, "track", 1, false)?;
                    crate::db::catalogue_favorites::request(c, "track", 1, true)?;
                } else {
                    let mut conflict = manifest["entries"][0].clone();
                    conflict["current"]["id"] = json!(999);
                    manifest["entries"].as_array_mut().unwrap().push(conflict);
                }
                assert!(apply(c, &manifest, &backup).is_err());
                assert_eq!(
                    c.query_row(
                        "SELECT COUNT(*) FROM catalogue_merge_audit WHERE entity='recovery'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
                assert_eq!(
                    c.query_row(
                        "SELECT COUNT(*) FROM tidal_track_aliases WHERE tidal_id=10",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
                Ok(())
            })
            .unwrap();
            assert!(
                !backup.exists(),
                "A stale or invalid manifest must not accumulate backups"
            );
        }
    }
}
