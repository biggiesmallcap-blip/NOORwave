//! Serialized, bounded delivery of durable explicit favorite actions.
use super::{auth, mutations};
use crate::db::catalogue_favorites as intents;
use crate::{AppEvent, SharedState};
use std::time::Duration;
static WORKER: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn run_if_idle(state: SharedState) {
    tokio::spawn(async move {
        let Ok(_guard) = WORKER.try_lock() else {
            return;
        };
        if let Err(error) = deliver(&state).await {
            tracing::warn!(target:"noor.favorites",%error,"favorite delivery paused");
        }
    });
}

async fn deliver(state: &SharedState) -> anyhow::Result<()> {
    let (db, http, mut tokens, events) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.http_client.clone(),
            s.tidal_tokens.clone(),
            s.event_tx.clone(),
        )
    };
    if tokens.take().is_none() || !db.with_conn(|c| Ok(intents::enabled(c)?))? {
        return Ok(());
    }
    // Enforce development isolation before requests, including token refresh.
    if mutations::check_library_writes().is_err() {
        return Ok(());
    }
    let delivered = deliver_pending(&db, |op| {
        let db = &db;
        let http = &http;
        async move {
            let session = state
                .read()
                .await
                .tidal_tokens
                .clone()
                .ok_or_else(|| anyhow::anyhow!("TIDAL disconnected"))?;
            let result = send(http, &session, &op).await;
            match result {
                Err(error)
                    if error.to_string().contains("401")
                        || error.to_string().contains("unauthorized") =>
                {
                    let session =
                        crate::server::routes::recover_tidal_session(state, http, &session).await?;
                    if !db.with_conn(|c| intents::current(c, &op))? {
                        return Ok(());
                    }
                    send(http, &session, &op).await
                }
                other => other,
            }
        }
    })
    .await?;
    if delivered {
        let _ = events.send(AppEvent::LibrarySynced);
    }
    Ok(())
}

/// A small transport seam keeps failure/restart/reversal tests completely
/// independent of credentials. Each operation is checked before and after I/O.
async fn deliver_pending<F, Fut>(db: &crate::db::Database, mut transport: F) -> anyhow::Result<bool>
where
    F: FnMut(intents::Operation) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    let mut delivered = false;
    for _ in 0..4 {
        let operations = db.with_conn(intents::pending)?;
        if operations.is_empty() {
            break;
        }
        for op in operations {
            if !db.with_conn(|c| intents::current(c, &op))? {
                continue;
            }
            let result = transport(op.clone()).await;
            let failure = result.as_ref().err().map(|_| "provider_request_failed");
            db.with_conn(|c| {
                let tx = c.unchecked_transaction()?;
                intents::finish(&tx, &op, failure)?;
                tx.commit()?;
                Ok(())
            })?;
            tracing::info!(target:"noor.favorites",entity=%op.entity,local_id=op.local_id,tidal_id=op.tidal_id,revision=op.revision,favorite=op.favorite,success=result.is_ok(),"explicit favorite delivery");
            delivered = true;
            if result.is_err() {
                return Ok(delivered);
            }
        }
    }
    Ok(delivered)
}

async fn send(
    http: &reqwest::Client,
    session: &auth::TidalTokens,
    op: &intents::Operation,
) -> anyhow::Result<()> {
    let request = async {
        match (op.entity.as_str(), op.favorite) {
            ("track", true) => {
                mutations::add_favorite_track(
                    http,
                    &session.access_token,
                    &session.user_id,
                    op.tidal_id,
                    &session.country_code,
                )
                .await
            }
            ("track", false) => {
                mutations::remove_favorite_track(
                    http,
                    &session.access_token,
                    &session.user_id,
                    op.tidal_id,
                    &session.country_code,
                )
                .await
            }
            ("album", true) => {
                mutations::add_favorite_album(
                    http,
                    &session.access_token,
                    &session.user_id,
                    op.tidal_id,
                    &session.country_code,
                )
                .await
            }
            ("album", false) => {
                mutations::remove_favorite_album(
                    http,
                    &session.access_token,
                    &session.user_id,
                    op.tidal_id,
                    &session.country_code,
                )
                .await
            }
            _ => anyhow::bail!("Invalid favorite operation"),
        }
    };
    tokio::time::timeout(Duration::from_secs(15), request).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, catalogue};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    #[tokio::test]
    async fn catalogue_album_unlike_retries_after_restart_with_mock_provider() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../target/catalogue-recovery/tests");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(&path).unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|c|{c.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist'); INSERT INTO albums(id,tidal_id,title,artist_id,is_favorite,source) VALUES(1,100,'Album',1,1,'tidal');
            INSERT INTO tidal_album_aliases(tidal_id,album_id,is_favorite) VALUES(200,1,1); UPDATE tidal_album_aliases SET is_favorite=1;")?;
            intents::request(c,"album",1,false)?;Ok(())}).unwrap();
        let remote = Arc::new(Mutex::new(HashSet::from([100, 200])));
        assert!(
            deliver_pending(&db, |_| async { anyhow::bail!("mock offline") })
                .await
                .unwrap()
        );
        drop(db);
        let db = Database::open(&path).unwrap();
        db.with_conn(|c| {
            assert_eq!(
                c.query_row("SELECT is_favorite FROM albums", [], |r| r.get::<_, i64>(0))?,
                0
            );
            c.execute(
                "UPDATE tidal_favorite_operations SET attempted_at='2000-01-01T00:00:00Z'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            deliver_pending(&db, |op| {
                let remote = remote.clone();
                async move {
                    remote.lock().unwrap().remove(&op.tidal_id);
                    Ok(())
                }
            })
            .await
            .unwrap()
        );
        assert!(remote.lock().unwrap().is_empty());
        db.with_conn(|c| {
            catalogue::reconcile_favorites_at(c, &HashSet::new(), true, "2099-01-01T00:00:00Z")?;
            assert_eq!(
                c.query_row("SELECT is_favorite FROM albums", [], |r| r.get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
        assert!(
            !deliver_pending(&db, |_| async {
                panic!("No completed operation may be sent twice")
            })
            .await
            .unwrap()
        );
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn catalogue_inflight_unlike_cannot_undo_newer_like() {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db.with_conn(|c|{c.execute_batch("INSERT INTO artists(id,name) VALUES(1,'Artist'); INSERT INTO tracks(id,tidal_id,title,artist_id,is_favorite,is_library,source) VALUES(1,10,'Song',1,1,1,'tidal');")?;
            intents::request(c,"track",1,false)?;Ok(())}).unwrap();
        let remote = Arc::new(Mutex::new(HashSet::from([10])));
        deliver_pending(&db, |op| {
            let remote = remote.clone();
            let db = db.clone();
            async move {
                if op.favorite {
                    remote.lock().unwrap().insert(op.tidal_id);
                } else {
                    db.with_conn(|c| {
                        intents::request(c, "track", 1, true)?;
                        Ok(())
                    })?;
                    remote.lock().unwrap().remove(&op.tidal_id);
                }
                Ok(())
            }
        })
        .await
        .unwrap();
        assert!(remote.lock().unwrap().contains(&10));
        db.with_conn(|c| {
            assert_eq!(
                c.query_row("SELECT is_favorite FROM tracks", [], |r| r.get::<_, i64>(0))?,
                1
            );
            assert!(intents::pending(c)?.is_empty());
            Ok(())
        })
        .unwrap();
    }
}
