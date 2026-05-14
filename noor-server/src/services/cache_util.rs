//! Shared time + encoding helpers for the service-layer caches.
//!
//! `services/tidal/cache.rs` and `services/sportify/cache.rs` both need the
//! same epoch-seconds clock, the same TTL freshness check, and the same
//! dependency-free hex encoder for query-hash keys. They keep their own
//! `hash_query` (the inputs differ - Sportify keys also include the search
//! kind) but everything else lives here so the two caches can't drift.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in whole seconds. Returns 0 if the system clock is
/// somehow before the epoch - callers treat that as "very stale", which is
/// the safe direction for a cache.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True while `fetched_at` is within `ttl` seconds of now. `saturating_sub`
/// keeps a future `fetched_at` (clock skew) from wrapping into "fresh forever".
pub fn fresh(fetched_at: i64, ttl: i64) -> bool {
    now_secs().saturating_sub(fetched_at) < ttl
}

/// Lowercase hex encoding. Hand-rolled to avoid pulling the `hex` crate into
/// the dependency tree just for cache-key formatting.
pub fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_within_ttl_and_stale_past_it() {
        let now = now_secs();
        assert!(fresh(now, 60), "just-fetched entry is fresh");
        assert!(fresh(now - 59, 60), "59s old with 60s ttl is fresh");
        assert!(!fresh(now - 61, 60), "61s old with 60s ttl is stale");
        assert!(!fresh(now, 0), "ttl of 0 is never fresh");
    }

    #[test]
    fn fresh_handles_future_fetched_at_without_wrapping() {
        // Clock skew: fetched_at in the future. saturating_sub floors at 0,
        // so the entry reads as fresh rather than "stale forever" or panicking.
        let now = now_secs();
        assert!(fresh(now + 1000, 60));
    }

    #[test]
    fn hex_encode_matches_known_values() {
        assert_eq!(hex_encode([]), "");
        assert_eq!(hex_encode([0x00]), "00");
        assert_eq!(hex_encode([0xff]), "ff");
        assert_eq!(hex_encode([0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_encode([0x01, 0x23, 0x45, 0x67]), "01234567");
    }
}
