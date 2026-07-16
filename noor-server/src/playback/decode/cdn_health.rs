//! Per-CDN-host health tracking for TIDAL DASH segment fetches.
//!
//! Background: TIDAL routes some tracks' DASH segments (frequently the init /
//! segment 0) through `sp-ad-cf.audio.tidal.com`, an ad-tier CloudFront edge.
//! On some networks that edge is unreliable: it serves real signed audio for
//! some tracks and black-holes others until the request times out. The LOW/AAC
//! quality tier routes there especially often. When a fetch lands on it and
//! hangs, playback either never starts or stalls mid-stream, and background
//! consumers (DJ profile rebuild, queue prescanner) pile up their own hung
//! requests onto the same host.
//!
//! Defense: a per-host circuit breaker shared across the playback decoder, the
//! DJ analysis prebuffer, and the prescanner, so they cannot each independently
//! rediscover a host that is already known to be failing. After a few
//! consecutive failures a host is marked degraded and its fetches use a short
//! timeout instead of the full 12 s, so a black-holed host fails in ~4 s rather
//! than 12-24 s. A single success clears it.
//!
//! Not done here: rewriting an `sp-ad-cf` URL onto a sibling edge. That was
//! tried and does not work - CloudFront signatures are bound to the host they
//! were issued for, so the rewritten URL returns 403. The only ways off a bad
//! edge are re-resolving the manifest or asking for a different quality tier,
//! both of which belong to the caller, not to a segment fetch.
//!
//! All decisions are cheap in-memory lookups - no I/O.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// The ad-tier CloudFront edge TIDAL sometimes assigns. Unreliable on some
/// networks: it serves some segments and black-holes others until timeout, so
/// it starts on a short leash rather than waiting for the breaker to trip.
pub(crate) const AD_CDN_HOST: &str = "sp-ad-cf.audio.tidal.com";

/// Full timeout for a segment fetch on a host with no recent failures.
pub(crate) const HEALTHY_SEGMENT_TIMEOUT: Duration = Duration::from_secs(12);
/// Short leash for a host the breaker has marked degraded (or the known-flaky
/// ad edge). Long enough for a genuinely slow-but-alive edge to answer, short
/// enough that a black-holed host does not freeze playback.
pub(crate) const DEGRADED_SEGMENT_TIMEOUT: Duration = Duration::from_secs(4);

/// Consecutive failures on a host before it is treated as degraded.
const TRIP_THRESHOLD: u32 = 3;
/// A degraded host stays degraded for at most this long after its last failure
/// (a success clears it sooner). Bounds the blast radius of a transient blip.
const TRIP_TTL: Duration = Duration::from_secs(300);

/// One fetch target produced from a manifest URL, with the timeout and retry
/// budget the breaker wants for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentCandidate {
    /// URL to fetch.
    pub url: String,
    /// Host this candidate targets, for breaker bookkeeping.
    pub host: String,
    /// Per-fetch timeout for this candidate.
    pub timeout: Duration,
    /// Max fetch attempts for this candidate (retries on failure).
    pub max_attempts: u32,
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
}

impl CdnHealthState {
    fn new() -> Self {
        Self {
            hosts: Mutex::new(HashMap::new()),
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
        if host == AD_CDN_HOST || self.is_degraded(host) {
            DEGRADED_SEGMENT_TIMEOUT
        } else {
            HEALTHY_SEGMENT_TIMEOUT
        }
    }

    /// The fetch target for a manifest segment URL.
    pub(crate) fn build_candidates(&self, url: &str) -> Vec<SegmentCandidate> {
        let host = host_from_url(url).unwrap_or_default();
        // A known-flaky or degraded host gets one shot on a short leash:
        // retrying a black hole only doubles the stall. A healthy host keeps
        // the retry-once-on-flap behaviour so a single blip doesn't kill it.
        let short_leash = host == AD_CDN_HOST || self.is_degraded(&host);
        vec![SegmentCandidate {
            url: url.to_string(),
            timeout: self.timeout_for(&host),
            max_attempts: if short_leash { 1 } else { 2 },
            host,
        }]
    }

    pub(crate) fn record_success(&self, candidate: &SegmentCandidate) {
        let mut guard = match self.hosts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(&candidate.host);
    }

    pub(crate) fn record_failure(&self, candidate: &SegmentCandidate) {
        let mut guard = match self.hosts.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let health = guard.entry(candidate.host.clone()).or_default();
        health.consecutive_failures = health.consecutive_failures.saturating_add(1);
        health.last_failure_at = Some(Instant::now());
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

/// The fetch target for a manifest segment URL (global breaker instance).
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

    fn candidate_for(host: &str) -> SegmentCandidate {
        SegmentCandidate {
            url: format!("https://{host}/5.mp4"),
            host: host.to_string(),
            timeout: HEALTHY_SEGMENT_TIMEOUT,
            max_attempts: 2,
        }
    }

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
    }

    /// The ad edge is never rewritten to a sibling host: CloudFront signatures
    /// are host-bound and the rewrite returns 403. It is fetched from its own
    /// host, just on a short leash.
    #[test]
    fn ad_edge_is_fetched_on_its_own_host_with_a_short_leash() {
        let state = CdnHealthState::new();
        let candidates =
            state.build_candidates("https://sp-ad-cf.audio.tidal.com/0.mp4?Signature=abc");

        assert_eq!(candidates.len(), 1, "must never fan out to a sibling edge");
        assert_eq!(candidates[0].host, AD_CDN_HOST);
        assert_eq!(
            candidates[0].url, "https://sp-ad-cf.audio.tidal.com/0.mp4?Signature=abc",
            "the signed URL must be passed through untouched"
        );
        assert_eq!(candidates[0].timeout, DEGRADED_SEGMENT_TIMEOUT);
        assert_eq!(candidates[0].max_attempts, 1);
    }

    #[test]
    fn healthy_host_is_a_single_full_timeout_candidate_with_one_retry() {
        let state = CdnHealthState::new();
        let candidates =
            state.build_candidates("https://sp-pr-cf.audio.tidal.com/5.mp4?Signature=abc");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].host, "sp-pr-cf.audio.tidal.com");
        assert_eq!(candidates[0].timeout, HEALTHY_SEGMENT_TIMEOUT);
        assert_eq!(candidates[0].max_attempts, 2);
    }

    #[test]
    fn breaker_trips_to_short_timeout_after_consecutive_failures_and_clears_on_success() {
        let state = CdnHealthState::new();
        let host = "sp-pr-cf.audio.tidal.com";
        let candidate = candidate_for(host);

        assert!(!state.is_degraded(host));
        for _ in 0..TRIP_THRESHOLD {
            state.record_failure(&candidate);
        }
        assert!(state.is_degraded(host));
        assert_eq!(state.timeout_for(host), DEGRADED_SEGMENT_TIMEOUT);
        // A degraded host also loses its retry.
        let candidates = state.build_candidates(&format!("https://{host}/5.mp4"));
        assert_eq!(candidates[0].max_attempts, 1);

        state.record_success(&candidate);
        assert!(!state.is_degraded(host));
        assert_eq!(state.timeout_for(host), HEALTHY_SEGMENT_TIMEOUT);
        assert_eq!(
            state.build_candidates(&format!("https://{host}/5.mp4"))[0].max_attempts,
            2
        );
    }

    #[test]
    fn a_success_resets_the_failure_streak_so_blips_do_not_accumulate() {
        let state = CdnHealthState::new();
        let host = "sp-pr-cf.audio.tidal.com";
        let candidate = candidate_for(host);

        state.record_failure(&candidate);
        state.record_failure(&candidate);
        state.record_success(&candidate);
        // Streak reset: two more failures must not trip it (would be 4 total).
        state.record_failure(&candidate);
        state.record_failure(&candidate);
        assert!(!state.is_degraded(host));
    }
}
