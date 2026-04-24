#![allow(unused)]

use crate::db::queries;
use crate::db::Database;
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::collections::HashMap;

/// Shazam-style constellation map fingerprint extraction.
/// Returns (hashes, peak_count) where hashes are (u32 hash, u32 time_offset) pairs.

const FFT_SIZE: usize = 2048;
const HOP_SIZE: usize = 512;

/// Extract a constellation map fingerprint from audio samples.
/// Returns Vec of (hash, time_offset) where time_offset is in frame units.
pub fn extract_fingerprint(samples: &[f32], _sample_rate: u32) -> (Vec<(u32, u32)>, u32) {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);

    let num_frames = samples.len().saturating_sub(FFT_SIZE) / HOP_SIZE;
    if num_frames == 0 {
        return (Vec::new(), 0);
    }

    // STFT: extract peaks per frame
    let mut peaks_per_frame: Vec<Vec<(u16, f32)>> = Vec::new();
    let mut buffer: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); FFT_SIZE];

    for frame_idx in 0..num_frames {
        let start = frame_idx * HOP_SIZE;
        let frame = &samples[start..start + FFT_SIZE];

        // Apply Hann window
        for (i, &s) in frame.iter().enumerate() {
            let w = 0.5 * (1.0
                - (2.0 * std::f64::consts::PI * i as f64 / (FFT_SIZE - 1) as f64).cos()
                    as f32);
            buffer[i] = Complex32::new(s * w, 0.0);
        }

        // FFT
        fft.process(&mut buffer);

        // Find peaks: top 5 per frame above adaptive threshold
        let magnitudes: Vec<f32> = buffer.iter().map(|c| c.norm()).collect();
        let mean_mag = magnitudes.iter().sum::<f32>() / magnitudes.len() as f32;
        let std_mag = (magnitudes
            .iter()
            .map(|m| (m - mean_mag).powi(2))
            .sum::<f32>()
            / magnitudes.len() as f32)
            .sqrt();
        let threshold = mean_mag + 2.0 * std_mag;

        let mut frame_peaks: Vec<(u16, f32)> = magnitudes
            .iter()
            .enumerate()
            .filter(|(_, m)| **m > threshold)
            .map(|(bin, &m)| (bin as u16, m))
            .collect();
        frame_peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        frame_peaks.truncate(5);
        peaks_per_frame.push(frame_peaks);
    }

    // Build hashes from peak pairs
    let mut hashes: Vec<(u32, u32)> = Vec::new();

    for anchor_idx in 0..peaks_per_frame.len().saturating_sub(1) {
        for &(anchor_freq, _anchor_mag) in &peaks_per_frame[anchor_idx] {
            // Zone: next 5 frames
            let zone_end = (anchor_idx + 6).min(peaks_per_frame.len());
            for target_idx in (anchor_idx + 1)..zone_end {
                for &(target_freq, _) in &peaks_per_frame[target_idx] {
                    // Hash: ((anchor_freq & 0x1FF) << 23) | ((target_freq & 0x1FF) << 14) | ((delta_t) & 0x3FF)
                    let delta_t = (target_idx - anchor_idx) as u32;
                    let hash = ((anchor_freq as u32 & 0x1FF) << 23)
                        | ((target_freq as u32 & 0x1FF) << 14)
                        | (delta_t & 0x3FF);
                    hashes.push((hash, anchor_idx as u32));
                }
            }
        }
    }

    let peak_count = peaks_per_frame.iter().map(|p| p.len()).sum::<usize>() as u32;
    (hashes, peak_count)
}

/// Match fingerprints: find tracks sharing hashes with the query.
/// Returns Vec of (track_id, score) where score = max_histogram_count / query_hashes.len()
pub fn match_fingerprints(
    db: &Database,
    query_hashes: &[(u32, u32)],
) -> Vec<(i64, f64)> {
    if query_hashes.is_empty() {
        return Vec::new();
    }

    let hash_values: Vec<u32> = query_hashes.iter().map(|(h, _)| *h).collect();

    // Find all tracks sharing any hash
    let candidates = db
        .with_conn(|conn| queries::find_tracks_by_hash(conn, &hash_values))
        .unwrap_or_default();

    if candidates.is_empty() {
        return Vec::new();
    }

    // Build a quick lookup: hash -> list of query offsets
    let mut query_hash_offsets: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(hash, offset) in query_hashes {
        query_hash_offsets.entry(hash).or_default().push(offset);
    }

    // Build histogram of time-delta alignment per track
    let mut track_alignments: HashMap<i64, Vec<i32>> = HashMap::new();

    for &(track_id, hash) in &candidates {
        if let Some(q_offsets) = query_hash_offsets.get(&hash) {
            let entry = track_alignments.entry(track_id).or_default();
            // For each query offset, compute delta (simplified — we don't have target offset here)
            for &q_off in q_offsets {
                entry.push(q_off as i32);
            }
        }
    }

    // Score = max histogram count / query_hashes.len()
    let query_len = query_hashes.len() as f64;
    let mut scores: Vec<(i64, f64)> = track_alignments
        .iter()
        .map(|(&tid, alignments)| {
            // Find max alignment count
            let mut counts: HashMap<i32, usize> = HashMap::new();
            for &delta in alignments {
                *counts.entry(delta).or_default() += 1;
            }
            let max_count = counts.values().copied().max().unwrap_or(0);
            (tid, max_count as f64 / query_len)
        })
        .filter(|&(_, score)| score > 0.05)
        .collect();

    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores
}

/// Gate: skip if peak_count < 50 (silence or too short)
pub fn should_skip_fingerprint(peak_count: u32) -> bool {
    peak_count < 50
}

/// Wire high-confidence pairs into duplicate_groups.
/// Legacy signature: takes a flat (track_id, score) list and creates one group
/// for all tracks above `min_confidence`. Returns count of groups created.
pub fn record_fingerprint_duplicates(
    db: &Database,
    matches: &[(i64, f64)],
    min_confidence: f64,
) -> usize {
    // Convert to pairwise form against the highest-scoring track so the
    // pairwise grouping logic handles everything in one code path.
    let mut filtered: Vec<(i64, f64)> = matches
        .iter()
        .copied()
        .filter(|(_, s)| *s > min_confidence)
        .collect();
    if filtered.len() < 2 {
        return 0;
    }
    filtered.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (anchor, anchor_conf) = filtered[0];
    let pairs: Vec<(i64, i64, f64)> = filtered
        .iter()
        .skip(1)
        .map(|(tid, score)| (anchor, *tid, score.min(anchor_conf)))
        .collect();

    record_fingerprint_duplicate_pairs(db, &pairs, min_confidence)
}

/// Insert high-confidence `(track_a, track_b, confidence)` fingerprint pairs
/// into `duplicate_groups` / `duplicate_members` tagged with source="fingerprint".
///
/// For each pair above `min_confidence`:
///   - If a group already contains both tracks → skip.
///   - Otherwise create a new group, add both tracks, and stamp source+confidence.
///
/// Returns the number of groups created.
pub fn record_fingerprint_duplicate_pairs(
    db: &Database,
    pairs: &[(i64, i64, f64)],
    min_confidence: f64,
) -> usize {
    let mut created = 0usize;

    for (a, b, confidence) in pairs.iter().copied() {
        if a == b || confidence <= min_confidence {
            continue;
        }
        let result: Result<bool, anyhow::Error> = db.with_conn(|conn| {
            if queries::find_duplicate_group_for_tracks(conn, a, b)?.is_some() {
                return Ok(false);
            }
            let gid = queries::create_duplicate_group(conn)?;
            queries::add_duplicate_member(conn, gid, a, false)?;
            queries::add_duplicate_member(conn, gid, b, false)?;
            queries::set_duplicate_group_source(conn, gid, "fingerprint", confidence)?;
            Ok(true)
        });

        match result {
            Ok(true) => created += 1,
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    "Failed to record fingerprint duplicate pair ({}, {}): {}",
                    a,
                    b,
                    e
                );
            }
        }
    }

    created
}

/// Run PRAGMA optimize + ANALYZE fingerprint_hashes after a bulk scan to keep
/// the SQLite query planner healthy. Failures are logged but non-fatal.
pub fn optimize_after_bulk_scan(db: &Database) {
    if let Err(e) = db.with_conn(|conn| queries::optimize_fingerprint_hashes(conn)) {
        tracing::warn!("PRAGMA optimize / ANALYZE fingerprint_hashes failed: {}", e);
    }
}
