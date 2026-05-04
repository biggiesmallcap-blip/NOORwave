use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{OnceLock, RwLock};

pub struct TidalBackoff {
    until_secs: AtomicI64,
    reason: RwLock<String>,
}

static GLOBAL: OnceLock<TidalBackoff> = OnceLock::new();

pub fn global() -> &'static TidalBackoff {
    GLOBAL.get_or_init(TidalBackoff::new)
}

impl TidalBackoff {
    pub fn new() -> Self {
        Self {
            until_secs: AtomicI64::new(0),
            reason: RwLock::new(String::new()),
        }
    }

    pub fn check(&self) -> anyhow::Result<()> {
        let now = chrono::Utc::now().timestamp();
        let until = self.until_secs.load(Ordering::Relaxed);
        if now < until {
            let remaining = until - now;
            let reason = self.reason.read().map(|r| r.clone()).unwrap_or_default();
            anyhow::bail!("TIDAL backoff active ({remaining}s remaining): {reason}");
        }
        Ok(())
    }

    pub fn classify(&self, status: u16, body: &str) {
        let duration_secs: i64 = match status {
            429 => 60,
            403 if {
                let lower = body.to_lowercase();
                lower.contains("abuse") || lower.contains("suspended")
            } =>
            {
                1800
            }
            _ => return,
        };
        let until = chrono::Utc::now().timestamp() + duration_secs;
        let current = self.until_secs.load(Ordering::Relaxed);
        if until > current {
            self.until_secs.store(until, Ordering::Relaxed);
            let reason_str = if status == 429 {
                "rate-limited (HTTP 429)".to_string()
            } else {
                format!(
                    "abuse-detected (HTTP 403): {}",
                    &body[..body.len().min(200)]
                )
            };
            if let Ok(mut r) = self.reason.write() {
                *r = reason_str;
            }
        }
    }

    pub fn state(&self) -> BackoffState {
        let now = chrono::Utc::now().timestamp();
        let until = self.until_secs.load(Ordering::Relaxed);
        let remaining_secs = (until - now).max(0) as f64;
        let reason = self.reason.read().map(|r| r.clone()).unwrap_or_default();
        BackoffState {
            active: remaining_secs > 0.0,
            remaining_secs,
            reason: if remaining_secs > 0.0 {
                reason
            } else {
                String::new()
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct BackoffState {
    pub active: bool,
    pub remaining_secs: f64,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> TidalBackoff {
        TidalBackoff {
            until_secs: AtomicI64::new(0),
            reason: RwLock::new(String::new()),
        }
    }

    #[test]
    fn no_backoff_initially() {
        let b = fresh();
        assert!(b.check().is_ok());
    }

    #[test]
    fn backoff_set_on_429() {
        let b = fresh();
        b.classify(429, "");
        assert!(b.check().is_err());
        let s = b.state();
        assert!(s.active);
        assert!(s.remaining_secs > 50.0 && s.remaining_secs <= 60.0);
    }

    #[test]
    fn backoff_30min_on_abuse_403() {
        let b = fresh();
        b.classify(403, "abuse detected");
        let s = b.state();
        assert!(s.active);
        assert!(s.remaining_secs > 1700.0 && s.remaining_secs <= 1800.0);
    }

    #[test]
    fn non_abuse_403_ignored() {
        let b = fresh();
        b.classify(403, "not found");
        assert!(b.check().is_ok());
    }

    #[test]
    fn backoff_not_shortened_by_429_after_30min_403() {
        let b = fresh();
        b.classify(403, "abuse");
        let long_until = b.until_secs.load(Ordering::Relaxed);
        b.classify(429, "");
        let after = b.until_secs.load(Ordering::Relaxed);
        assert_eq!(
            long_until, after,
            "60s window must not shorten the 1800s window"
        );
    }

    #[test]
    fn server_errors_ignored() {
        let b = fresh();
        b.classify(500, "internal error");
        assert!(b.check().is_ok());
    }

    #[test]
    fn state_inactive_initially() {
        let b = fresh();
        let s = b.state();
        assert!(!s.active);
        assert_eq!(s.remaining_secs, 0.0);
    }
}
