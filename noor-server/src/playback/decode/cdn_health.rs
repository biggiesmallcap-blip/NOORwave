//! Per-CDN-host health tracking and dead-edge failover for TIDAL DASH segment
//! fetches.
//!
//! Background: TIDAL routes some tracks' DASH segments (frequently the init /
//! segment 0) through `sp-ad-cf.audio.tidal.com`, an ad-tier CloudFront edge.
//! On some networks that edge is a black hole - every request hangs until the
//! full request timeout, then fails. When a track's manifest points at that
//! host, playback either never starts or stalls mid-stream, and background
//! consumers (DJ profile rebuild, queue prescanner) pile up their own hung
//! requests onto the same dead host. A single unreachable edge was enough to
//! make the whole runtime feel wedged until a server restart.
//!
//! This module gives every segment fetch two defenses, shared across the
//! playback decoder, the DJ analysis prebuffer, and the prescanner so they
//! cannot each independently rediscover the dead edge:
//!
//!   1. A per-host circuit breaker. After a few consecutive failures a host is
//!      marked degraded and its fetches use a short timeout instead of the full
//!      12 s, so a black-holed host fails in ~4 s rather than 12-24 s. A single
//!      success clears it.
//!   2. Dead-edge host rewriting. For a URL on the known-bad `sp-ad-cf` edge we
//!      first try the same signed path on the healthy `sp-pr-cf` sibling edge
//!      (same CloudFront key group, so a canned-policy signature validates on
//!      either host). If the rewrite works the dead edge is never touched; if
//!      it never works on this deployment (signatures turn out to be host-
//!      bound) the swap self-disables after a short probe window and we fall
//!      back to the short-timeout dead edge.
//!
//! All decisions are cheap in-memory atomics/mutex lookups - no I/O.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The ad-tier CloudFront edge TIDAL sometimes assigns. Unreachable on some
/// networks; when a track's segments route here they black-hole until timeout.
pub(crate) const AD_CDN_HOST: &str = "sp-ad-cf.audio.tidal.com";
/// The healthy production edge we rewrite the dead edge to. Same CloudFront
/// distribution / key group as the ad edge, so a signed URL's signature is
/// expected to validate on either host.
pub(crate) const FALLBACK_CDN_HOST: &str = "sp-pr-cf.audio.tidal.com";

/// Full timeout for a segment fetch on a host with no recent failures.
pub(crate) const HEALTHY_SEGMENT_TIMEOUT: Duration = Duration::from_secs(12);
/// Short leash for a host the breaker has marked degraded (or the known dead
/// edge). Long enough for a genuinely slow-but-alive edge to answer, short
/// enough that a black-holed host does not freeze playback.
pub(crate) const DEGRADED_SEGMENT_TIMEOUT: Duration = Duration::from_secs(4);

/// Consecutive failures on a host before it is treated as degraded.
const TRIP_THRESHOLD: u32 = 3;
/// A degraded host stays degraded for at most this long after its last failure
/// (a success clears it sooner). Bounds the blast radius of a transient blip.
const TRIP_TTL: Duration = Duration::from_secs(300);
/// How many times we probe the `sp-ad-cf -> sp-pr-cf` rewrite before giving up
/// on it, once it has never produced a single success. If it ever works once,
/// it stays enabled for the life of the process.
const SWAP_PROBE_LIMIT: u64 = 8;

/// One fetch target produced from a manifest URL: possibly a rewritten host,
/// with the timeout and retry budget the breaker wants for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentCandidate {
    /// URL to actually fetch (may differ from the manifest URL after a rewrite).
    pub url: String,
    /// Host this candidate targets, for breaker bookkeeping.
    pub host: String,
    /// Per-fetch timeout for this candidate.
    pub timeout: Duration,
    /// Max fetch attempts for this candidate (retries on retryable errors).
    pub max_attempts: u32,
    /// True when this candidate is the `sp-ad-cf -> sp-pr-cf` rewrite, so the
    /// caller can feed the swap-probe counters.
    pub is_dead_edge_swap: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct HostHealth {
    consecutive_failures: u32,
    last_failure_at: Option<Instant>,
}

/// Health state for all CDN hosts. A single global instance backs playback,
/// DJ analysis, and the prescanner; tests construct their own.
#[derive(Debug)]
pub(crate) struct CdnHealthState {
    hosts: Mutex<HashMap<String, HostHealth>>,
    swap_successes: AtomicU64,
    swap_failures: AtomicU64,
}

impl CdnHealthState {
    fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
            swap_successes: AtomicU64::new(0),
            swap_failures: AtomicU64::new(0),
        }
    }

    fn is_degraded(&self, host: &str) -> bool {
        let guard = match self.hosts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match guard.get(host) {
            Some(health) => {
                health.consecutive_failures >= TRIP_THRESHOLD
                    && health
                        .last_failure_at
                        .is_some_and(|at| at.elapsed() < TRIP_TTL)
            }
            None => false,
        }
    }

    fn timeout_for(&self, host: &str) -> Duration {
        if self.is_degraded(host) {
            DEGRADED_SEGMENT_TIMEOUT
        } else {
            HEALTHY_SEGMENT_TIMEOUT
        }
    }

    /// Whether the dead-edge rewrite is still worth trying. Stays enabled once
    /// it has ever succeeded; otherwise disables after `SWAP_PROBE_LIMIT`
    /// failures so we stop paying for a rewrite that this deployment rejects.
    fn swap_enabled(&self) -> bool {
        self.swap_successes.load(Ordering::Relaxed) > 0
            || self.swap_failures.load(Ordering::Relaxed) < SWAP_PROBE_LIMIT
    }

    /// Ordered fetch targets for a manifest segment URL. Callers try them in
    /// order and stop on the first success.
    pub(crate) fn build_candidates(&self, url: &str) -> Vec<SegmentCandidate> {
        let host = host_from_url(url).unwrap_or_default();

        if host == AD_CDN_HOST {
            let mut candidates = Vec::with_capacity(2);
            if self.swap_enabled() {
                // Prefer the healthy sibling edge with the same signed path.
                candidates.push(SegmentCandidate {
                    url: url.replacen(AD_CDN_HOST, FALLBACK_CDN_HOST, 1),
                    host: FALLBACK_CDN_HOST.to_string(),
                    timeout: self.timeout_for(FALLBACK_CDN_HOST),
                    max_attempts: 2,
                    is_dead_edge_swap: true,
                });
            }
            // Last resort: the dead edge itself, on a short leash and no retry
            // (retrying a known black hole only doubles the stall).
            candidates.push(SegmentCandidate {
                url: url.to_string(),
                host: AD_CDN_HOST.to_string(),
                timeout: DEGRADED_SEGMENT_TIMEOUT,
                max_attempts: 1,
                is_dead_edge_swap: false,
            });
            candidates
        } else {
            vec![SegmentCandidate {
                url: url.to_string(),
                host: host.clone(),
                timeout: self.timeout_for(&host),
                // A degraded host gets one shot; a healthy one keeps the
                // retry-once-on-flap behaviour.
                max_attempts: if self.is_degraded(&host) { 1 } else { 2 },
                is_dead_edge_swap: false,
            }]
        }
    }

    pub(crate) fn record_success(&self, candidate: &SegmentCandidate) {
        {
            let mut guard = match self.hosts.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.remove(&candidate.host);
        }
        if candidate.is_dead_edge_swap {
            self.swap_successes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_failure(&self, candidate: &SegmentCandidate) {
        {
            let mut guard = match self.hosts.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            let health = guard.entry(candidate.host.clone()).or_default();
            health.consecutive_failures = health.consecutive_failures.saturating_add(1);
            health.last_failure_at = Some(Instant::now());
        }
        if candidate.is_dead_edge_swap {
            self.swap_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Extract the lowercased host (no port) from an absolute URL.
fn host_from_url(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // Strip userinfo and port.
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host.split_once(':').map(|(host, _)| host).unwrap_or(host);
    if host.is_empty() {
        None
    } else {
        Some(host.to_ascii_lowercase())
    }
}

static GLOBAL: LazyLock<CdnHealthState> = LazyLock::new(CdnHealthState::new);

/// Ordered fetch targets for a manifest segment URL (global breaker instance).
pub(crate) fn build_candidates(url: &str) -> Vec<SegmentCandidate> {
    GLOBAL.build_candidates(url)
}

/// Record a successful fetch of `candidate` (global breaker instance).
pub(crate) fn record_success(candidate: &SegmentCandidate) {
    GLOBAL.record_success(candidate);
}

/// Record a failed fetch of `candidate` (global breaker instance).
pub(crate) fn record_failure(candidate: &SegmentCandidate) {
    GLOBAL.record_failure(candidate);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_from_url_parses_scheme_host_port_path() {
        assert_eq!(
            host_from_url("https://sp-ad-cf.audio.tidal.com/0.mp4?Key=x").as_deref(),
            Some("sp-ad-cf.audio.tidal.com")
        );
        assert_eq!(
            host_from_url("https://Sp-Pr-Cf.Audio.Tidal.com:443/3.mp4").as_deref(),
            Some("sp-pr-cf.audio.tidal.com")
        );
        assert_eq!(host_from_url("not a url"), Some("not a url".to_string()));
    }

    #[test]
    fn ad_edge_url_tries_healthy_sibling_first_then_short_leash_dead_edge() {
        let state = CdnHealthState::new();
        let candidates =
            state.build_candidates("https://sp-ad-cf.audio.tidal.com/0.mp4?Signature=abc");

        assert_eq!(candidates.len(), 2);
        // Swap first, on the healthy host, carrying the identical signed path.
        assert_eq!(candidates[0].host, FALLBACK_CDN_HOST);
        assert_eq!(
            candidates[0].url,
            "https://sp-pr-cf.audio.tidal.com/0.mp4?Signature=abc"
        );
        assert!(candidates[0].is_dead_edge_swap);
        // Dead edge last, short timeout, single shot.
        assert_eq!(candidates[1].host, AD_CDN_HOST);
        assert_eq!(candidates[1].timeout, DEGRADED_SEGMENT_TIMEOUT);
        assert_eq!(candidates[1].max_attempts, 1);
        assert!(!candidates[1].is_dead_edge_swap);
    }

    #[test]
    fn healthy_host_is_a_single_full_timeout_candidate() {
        let state = CdnHealthState::new();
        let candidates =
            state.build_candidates("https://sp-pr-cf.audio.tidal.com/5.mp4?Signature=abc");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].host, FALLBACK_CDN_HOST);
        assert_eq!(candidates[0].timeout, HEALTHY_SEGMENT_TIMEOUT);
        assert_eq!(candidates[0].max_attempts, 2);
        assert!(!candidates[0].is_dead_edge_swap);
    }

    #[test]
    fn breaker_trips_to_short_timeout_after_consecutive_failures_and_clears_on_success() {
        let state = CdnHealthState::new();
        let candidate = SegmentCandidate {
            url: "https://sp-pr-cf.audio.tidal.com/5.mp4".to_string(),
            host: FALLBACK_CDN_HOST.to_string(),
            timeout: HEALTHY_SEGMENT_TIMEOUT,
            max_attempts: 2,
            is_dead_edge_swap: false,
        };

        assert!(!state.is_degraded(FALLBACK_CDN_HOST));
        for _ in 0..TRIP_THRESHOLD {
            state.record_failure(&candidate);
        }
        assert!(state.is_degraded(FALLBACK_CDN_HOST));
        assert_eq!(
            state.timeout_for(FALLBACK_CDN_HOST),
            DEGRADED_SEGMENT_TIMEOUT
        );
        // A degraded healthy host collapses to a single short-timeout candidate.
        let candidates = state.build_candidates("https://sp-pr-cf.audio.tidal.com/5.mp4");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].max_attempts, 1);

        state.record_success(&candidate);
        assert!(!state.is_degraded(FALLBACK_CDN_HOST));
        assert_eq!(
            state.timeout_for(FALLBACK_CDN_HOST),
            HEALTHY_SEGMENT_TIMEOUT
        );
    }

    #[test]
    fn swap_self_disables_after_probe_limit_with_no_success() {
        let state = CdnHealthState::new();
        let swap = SegmentCandidate {
            url: "https://sp-pr-cf.audio.tidal.com/0.mp4".to_string(),
            host: FALLBACK_CDN_HOST.to_string(),
            timeout: HEALTHY_SEGMENT_TIMEOUT,
            max_attempts: 2,
            is_dead_edge_swap: true,
        };

        for _ in 0..SWAP_PROBE_LIMIT {
            assert!(state.swap_enabled());
            state.record_failure(&swap);
        }
        assert!(!state.swap_enabled());

        // With the swap disabled, an ad-edge URL collapses to the dead edge on
        // a short leash (still far better than the old 12 s x 2).
        let candidates = state.build_candidates("https://sp-ad-cf.audio.tidal.com/0.mp4");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].host, AD_CDN_HOST);
        assert_eq!(candidates[0].timeout, DEGRADED_SEGMENT_TIMEOUT);
    }

    #[test]
    fn a_single_swap_success_keeps_the_swap_enabled_forever() {
        let state = CdnHealthState::new();
        let swap = SegmentCandidate {
            url: "https://sp-pr-cf.audio.tidal.com/0.mp4".to_string(),
            host: FALLBACK_CDN_HOST.to_string(),
            timeout: HEALTHY_SEGMENT_TIMEOUT,
            max_attempts: 2,
            is_dead_edge_swap: true,
        };

        state.record_success(&swap);
        for _ in 0..(SWAP_PROBE_LIMIT * 4) {
            state.record_failure(&swap);
        }
        assert!(state.swap_enabled());
    }
}
