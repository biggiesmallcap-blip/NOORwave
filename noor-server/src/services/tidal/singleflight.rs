use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex, Weak};

/// Coalesces concurrent cache misses by key without retaining an unbounded
/// key registry. The builder owns cache population; waiters re-read after the
/// winner releases the per-key lock.
pub struct KeyedSingleFlight<K> {
    locks: Mutex<HashMap<K, Weak<tokio::sync::Mutex<()>>>>,
}

impl<K> Default for KeyedSingleFlight<K> {
    fn default() -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
        }
    }
}

impl<K> KeyedSingleFlight<K>
where
    K: Eq + Hash,
{
    fn lock_for(&self, key: K) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
            return lock;
        }

        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(key, Arc::downgrade(&lock));
        lock
    }

    pub async fn get_or_build<V, E, Read, Build, BuildFuture>(
        &self,
        key: K,
        read: Read,
        build: Build,
    ) -> Result<V, E>
    where
        Read: Fn() -> Option<V> + Send + Sync,
        Build: FnOnce() -> BuildFuture + Send,
        BuildFuture: Future<Output = Result<V, E>> + Send,
        V: Send,
        E: Send,
    {
        if let Some(hit) = read() {
            return Ok(hit);
        }

        let lock = self.lock_for(key);
        let _guard = lock.lock().await;
        if let Some(hit) = read() {
            return Ok(hit);
        }

        build().await
    }
}

#[cfg(test)]
mod tests {
    use super::KeyedSingleFlight;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_cache_misses_for_the_same_key_share_one_build() {
        let flights = Arc::new(KeyedSingleFlight::<String>::default());
        let cached = Arc::new(Mutex::new(None::<String>));
        let builds = Arc::new(AtomicUsize::new(0));

        let run = || {
            let flights = flights.clone();
            let cached_for_read = cached.clone();
            let cached_for_build = cached.clone();
            let builds = builds.clone();
            tokio::spawn(async move {
                flights
                    .get_or_build(
                        "michael jackson|12|0".to_string(),
                        move || cached_for_read.lock().unwrap().clone(),
                        move || async move {
                            builds.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            let value = "result".to_string();
                            *cached_for_build.lock().unwrap() = Some(value.clone());
                            Ok::<_, ()>(value)
                        },
                    )
                    .await
                    .unwrap()
            })
        };

        let (first, second) = tokio::join!(run(), run());

        assert_eq!(first.unwrap(), "result");
        assert_eq!(second.unwrap(), "result");
        assert_eq!(builds.load(Ordering::SeqCst), 1);
    }
}
