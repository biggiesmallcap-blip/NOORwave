/// Rust port of `scripts/discovery_trainer.py` — pure-math embedding pipeline.
///
/// Algorithm:
/// 1. Behavioral embeddings via co-occurrence hashing (word2vec-style, but no neural net)
/// 2. Audio proxy features via hashed metadata tokens
/// 3. Fusion blend (behavioral for popular tracks, audio for rare tracks)
/// 4. Full O(n²) neighbor graph with artist/genre/album bonuses
/// 5. Evaluation: recall@10 + MRR@20 on held-out transition pairs
///
/// With rayon parallelism: ~10-30s for 32k tracks (was hours in Python).
use crate::db::queries::EmbeddingTrackRow;
use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

// ── Progress update struct ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrainingProgressUpdate {
    pub stage: String,
    pub progress: f32,
    pub message: String,
    pub current_track_id: Option<i64>,
    pub current_track_title: Option<String>,
    pub tracks_done: u32,
    pub tracks_total: u32,
}

impl TrainingProgressUpdate {
    pub fn stage_only(stage: &str, message: &str, progress: f32) -> Self {
        Self {
            stage: stage.to_string(),
            progress,
            message: message.to_string(),
            current_track_id: None,
            current_track_title: None,
            tracks_done: 0,
            tracks_total: 0,
        }
    }
}

// ── Input types (mirror Python's JSON structure) ──────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TrainerSequenceGroup {
    pub label: String,
    pub weight: f64,
    pub sequences: Vec<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainerInput {
    pub seed: u64,
    pub dimension: usize,
    pub window_size: usize,
    pub min_count: usize,
    pub top_k: usize,
    // When false, the audio-proxy stage is skipped entirely. Drives the Low
    // intensity tier — the cold-start penalty is real but the wall-clock
    // savings are too. Fusion still runs over whatever embeddings exist.
    #[serde(default = "default_include_audio_proxy")]
    pub include_audio_proxy: bool,
    pub tracks: Vec<EmbeddingTrackRow>,
    pub sequences: Vec<TrainerSequenceGroup>,
    pub heldout_pairs: Vec<(i64, i64)>,
}

#[allow(dead_code)]
fn default_include_audio_proxy() -> bool {
    true
}

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TrainerAudioFeature {
    pub vector: Vec<f64>,
    pub clip_start_ms: i64,
    pub clip_duration_ms: i64,
    pub feature_version: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainerNeighbor {
    pub track_id: i64,
    pub neighbor_track_id: i64,
    pub rank: i32,
    pub score: f64,
    pub behavioral_score: f64,
    pub audio_score: f64,
    pub metadata_score: f64,
    pub reason_tags: Vec<String>,
    pub primary_reason: Option<String>,
    pub confidence: f64,
    pub support_count: i64,
    pub play_count_seed: i64,
    pub play_count_candidate: i64,
    // Filled in by a second pass (compute_in_degree) after neighbor lists are
    // built — left at 0.0 / 0 in the inner per-pair loop.
    pub candidate_in_degree: i64,
    pub candidate_in_degree_percentile: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainerOutput {
    pub behavioral_embeddings: HashMap<i64, Vec<f64>>,
    pub audio_features: HashMap<i64, TrainerAudioFeature>,
    pub fusion_embeddings: HashMap<i64, Vec<f64>>,
    pub neighbors: Vec<TrainerNeighbor>,
    pub metrics: HashMap<String, f64>,
    pub reason_hit_rates: Vec<ReasonHitRate>,
}

// Per-reason held-out hit-rate breakdown. `insufficient_data = true` means the
// row is informational only — its hit_rate is too noisy to trust because we
// haven't yet seen enough impressions of this reason to draw a conclusion.
// Used downstream to surface which reason tags are predictive vs. cosmetic, so
// the next iteration of the metadata bonus weights can be calibrated to data.
#[derive(Debug, Clone, Serialize)]
pub struct ReasonHitRate {
    pub primary_reason: String,
    pub impressions: i64,
    pub hits: i64,
    pub hit_rate: f64,
    pub mean_rank: Option<f64>,
    pub mrr_contribution: f64,
    pub insufficient_data: bool,
}

// Threshold below which a reason's hit_rate is considered too noisy to act on.
// Picked empirically: at 20 impressions, a single hit/miss flip changes the
// observed rate by 5 percentage points — fine-grained enough to compare reasons
// directionally without rare tags ("punk_lullabye") looking miraculous off n=2.
pub const MIN_REASON_IMPRESSIONS: i64 = 20;

// ── Core math utilities ───────────────────────────────────────────────────────

fn normalize(vec: &[f64]) -> (Vec<f64>, f64) {
    let norm = vec.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm <= 1e-12 {
        return (vec![0.0; vec.len()], 0.0);
    }
    (vec.iter().map(|v| v / norm).collect(), norm)
}

fn cosine(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// SHA256-based hashed projection of tokens into a `dim`-dimensional vector.
/// Mirrors Python's `hashed_projection`.
fn hashed_projection(tokens: &[String], dim: usize) -> Vec<f64> {
    let mut vec = vec![0.0f64; dim];
    for token in tokens {
        let digest = Sha256::digest(token.as_bytes());
        let step = 2;
        let limit = usize::min(32, dim * 2);
        for offset in (0..limit).step_by(step) {
            let bucket = digest[offset] as usize % dim;
            let sign = if digest[offset + 1] % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            vec[bucket] += sign * 0.5;
        }
    }
    normalize(&vec).0
}

/// Extract metadata tokens from a track row (mirrors Python's `metadata_tokens`).
fn metadata_tokens(track: &EmbeddingTrackRow) -> Vec<String> {
    let mut tokens = Vec::new();
    let fields: Vec<Option<&str>> = vec![
        Some(&track.title),
        track.artist_name.as_deref(),
        track.album_title.as_deref(),
        track.best_quality.as_deref(),
        Some(&track.source),
    ];
    for value in fields.into_iter().flatten() {
        let normalized = value.to_lowercase().replace('/', " ").replace('-', " ");
        tokens.extend(normalized.split_whitespace().map(String::from));
    }
    if let Some(duration) = track.duration_ms {
        tokens.push(format!("dur_{}", duration / 30_000));
    }
    for genre in &track.genre_paths {
        let normalized = genre.to_lowercase().replace('>', " ");
        tokens.extend(normalized.split_whitespace().map(String::from));
    }
    // DSP features — bucketed so nearby values share tokens
    if let Some(bpm) = track.bpm {
        // 5-BPM buckets (e.g. bpm_140, bpm_145). Also add a coarser 10-BPM token
        // so tracks at 139 and 141 still share "bpm_140".
        tokens.push(format!("bpm_{}", ((bpm / 5.0).round() as i64) * 5));
        tokens.push(format!("bpm10_{}", ((bpm / 10.0).round() as i64) * 10));
    }
    if let Some(energy) = track.energy {
        // 10 buckets: 0.0-0.1 = e0, 0.1-0.2 = e1, …
        tokens.push(format!("energy_{}", (energy * 10.0).floor() as i64));
    }
    if let Some(ref key) = track.camelot_key {
        tokens.push(format!("key_{}", key.to_lowercase()));
        // Also add the adjacent keys on the Camelot wheel so harmonically compatible
        // tracks cluster together (e.g. 8B is compatible with 7B, 9B, 8A).
        if let Some(num_end) = key.find(|c: char| c.is_alphabetic()) {
            if let Ok(n) = key[..num_end].parse::<i64>() {
                let suffix = &key[num_end..].to_lowercase();
                tokens.push(format!("key_{}{}", ((n - 2).rem_euclid(12) + 1), suffix));
                tokens.push(format!("key_{}{}", (n % 12) + 1, suffix));
                // Relative major/minor (same number, opposite A/B)
                let alt = if suffix == "a" { "b" } else { "a" };
                tokens.push(format!("key_{}{}", n, alt));
            }
        }
    }
    tokens
}

// ── Stage 1: Behavioral embeddings (co-occurrence + hash projection) ──────────

// Helper: cheap atomic check used inside hot loops. Inlined call sites should
// keep the branch predictor happy when cancel stays false (the common case).
#[inline]
fn cancel_requested(cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>) -> bool {
    cancel
        .map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false)
}

// Returns (behavioral embeddings, raw co-occurrence counts).
// The count map is later used to derive support_count and confidence per edge —
// the embedding alone discards this signal because hashed projection collapses
// many co-occurrences into shared buckets.
//
// `cancel` is honored at two granularities: every 64 sequences during the
// co-occurrence build, and every iteration of the parallel hash-projection
// stage. When cancel fires the function returns partial state; the caller's
// `bail_if_cancelled` is responsible for recognizing that and aborting.
fn build_behavioral_embeddings(
    input: &TrainerInput,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> (HashMap<i64, Vec<f64>>, HashMap<i64, HashMap<i64, i64>>) {
    let dim = input.dimension;
    let window = input.window_size;
    let min_count = input.min_count;

    // Count track occurrences across all sequences
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for source in &input.sequences {
        for sequence in &source.sequences {
            for &track_id in sequence {
                *counts.entry(track_id).or_default() += 1;
            }
        }
    }

    let allowed: HashSet<i64> = counts
        .into_iter()
        .filter_map(|(id, count)| if count >= min_count { Some(id) } else { None })
        .collect();

    // Build co-occurrence scores: track_id -> { other_track_id -> weighted_score }
    // Plus raw counts for support/confidence computation downstream.
    let mut co: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    let mut co_count: HashMap<i64, HashMap<i64, i64>> = HashMap::new();
    let mut sequences_seen: usize = 0;
    'outer: for source in &input.sequences {
        let weight = source.weight;
        for sequence in &source.sequences {
            // Cancel check every 64 sequences keeps the atomic-load cost out
            // of the inner triple-nested loop while still feeling responsive
            // (a typical sequence is short enough that 64 of them process in
            // well under a second).
            sequences_seen += 1;
            if sequences_seen.is_multiple_of(64) && cancel_requested(cancel) {
                break 'outer;
            }
            let filtered: Vec<i64> = sequence
                .iter()
                .copied()
                .filter(|id| allowed.contains(id))
                .collect();
            let len = filtered.len();
            for i in 0..len {
                let track_id = filtered[i];
                let left = i.saturating_sub(window);
                let right = (i + window + 1).min(len);
                let entry = co.entry(track_id).or_default();
                let count_entry = co_count.entry(track_id).or_default();
                for j in left..right {
                    if i == j {
                        continue;
                    }
                    let other = filtered[j];
                    let distance = (i as isize - j as isize).unsigned_abs();
                    *entry.entry(other).or_default() += weight / (distance as f64).max(1.0);
                    *count_entry.entry(other).or_default() += 1;
                }
            }
        }
    }

    if cancel_requested(cancel) {
        return (HashMap::new(), co_count);
    }

    // Hash-project each track's neighbor scores into a dense vector. Cancel is
    // checked once per outer track via `find_any` semantics — if any worker
    // sees the flag, downstream `bail_if_cancelled` will catch it; this pass
    // just stops accumulating work as fast as it can.
    let embeddings = co
        .into_par_iter()
        .map(|(track_id, neighbors)| {
            if cancel_requested(cancel) {
                return (track_id, Vec::new());
            }
            let mut vec = vec![0.0f64; dim];
            for (&other, &score) in &neighbors {
                let key = format!("{track_id}:{other}");
                let digest = Sha256::digest(key.as_bytes());
                let limit = usize::min(32, dim * 2);
                for offset in (0..limit).step_by(2) {
                    let bucket = digest[offset] as usize % dim;
                    let sign = if digest[offset + 1] % 2 == 0 {
                        1.0
                    } else {
                        -1.0
                    };
                    vec[bucket] += sign * score;
                }
            }
            (track_id, normalize(&vec).0)
        })
        .collect();
    (embeddings, co_count)
}

// ── Stage 2: Audio proxy features (hashed metadata) ───────────────────────────

// Parallel + cancel-aware. Each track's audio-proxy vector is independent, so
// rayon parallelism is a free win that also makes the cancel check land
// quickly (workers stop accepting new tracks the moment the flag flips).
fn build_audio_proxy_features(
    tracks: &[EmbeddingTrackRow],
    dim: usize,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> HashMap<i64, TrainerAudioFeature> {
    tracks
        .par_iter()
        .filter_map(|track| {
            if cancel_requested(cancel) {
                return None;
            }
            let clip_duration = 20_000i64;
            let duration = track.duration_ms.unwrap_or(0);
            let clip_start = if duration >= 90_000 {
                30_000
            } else {
                (duration - clip_duration).max(0) / 2
            };
            let tokens = metadata_tokens(track);
            let vec = hashed_projection(&tokens, dim);
            Some((
                track.track_id,
                TrainerAudioFeature {
                    vector: vec,
                    clip_start_ms: clip_start,
                    clip_duration_ms: clip_duration,
                    feature_version: "metadata-audio-proxy-v1".to_string(),
                },
            ))
        })
        .collect()
}

// ── Stage 3: Fusion embedding (behavioral + audio blend) ──────────────────────

fn fuse_embeddings(
    tracks: &[EmbeddingTrackRow],
    behavioral: &HashMap<i64, Vec<f64>>,
    audio: &HashMap<i64, TrainerAudioFeature>,
) -> HashMap<i64, Vec<f64>> {
    tracks
        .iter()
        .filter_map(|track| {
            let b = behavioral.get(&track.track_id);
            let a = audio.get(&track.track_id).map(|f| &f.vector);
            let playlist = track.playlist_memberships;
            let plays = track.play_count;

            let vec: Vec<f64> = match (b, a) {
                (Some(b_vec), Some(a_vec)) => {
                    let (b_weight, a_weight) = if plays < 2 && playlist == 0 {
                        (0.35, 0.65)
                    } else {
                        (0.7, 0.3)
                    };
                    b_vec
                        .iter()
                        .zip(a_vec.iter())
                        .map(|(bv, av)| bv * b_weight + av * a_weight)
                        .collect()
                }
                (Some(b_vec), None) => b_vec.clone(),
                // No behavioral signal — fall back to pure audio proxy so the
                // track still appears in the graph instead of being silently dropped.
                (None, Some(a_vec)) => a_vec.clone(),
                (None, None) => return None,
            };
            Some((track.track_id, normalize(&vec).0))
        })
        .collect()
}

// ── Stage 4: Full O(n²) neighbor graph (parallelized with rayon) ─────────────

/// Pre-computed metadata for a single track — built once, read millions of times.
struct TrackMeta {
    track_id: i64,
    artist_lower: Option<String>,
    album: Option<String>,
    genre_tokens: HashSet<String>,
    bpm: Option<f64>,
    energy: Option<f64>,
    camelot_key: Option<String>,
}

// Sigmoid: 1 / (1 + e^-x). Used to map log1p(support_count) into [0, 1] for
// behavioral confidence; the -1.5 shift means an edge needs ~5 supporting events
// before crossing 0.5 confidence, which matches the heuristic in the plan.
fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

fn similarity_neighbors(
    tracks: &[EmbeddingTrackRow],
    behavioral: &HashMap<i64, Vec<f64>>,
    audio: &HashMap<i64, TrainerAudioFeature>,
    fusion: &HashMap<i64, Vec<f64>>,
    co_count: &HashMap<i64, HashMap<i64, i64>>,
    play_counts: &HashMap<i64, i64>,
    top_k: usize,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<TrainingProgressUpdate>>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Vec<TrainerNeighbor> {
    let total = fusion.len();
    if total == 0 {
        return Vec::new();
    }

    // Pre-compute all per-track metadata ONCE (not per-pair)
    // This avoids calling metadata_tokens() N² times and allocating HashSets in the hot loop.
    let track_metas: Vec<TrackMeta> = tracks
        .iter()
        .filter(|t| fusion.contains_key(&t.track_id))
        .map(|t| {
            let artist_lower = t.artist_name.as_ref().map(|s| s.to_lowercase());
            let album = t.album_title.clone();
            let genre_tokens: HashSet<String> = metadata_tokens(t)
                .into_iter()
                .filter(|tok| {
                    !tok.starts_with("dur_")
                        && !tok.starts_with("bpm")
                        && !tok.starts_with("energy_")
                        && !tok.starts_with("key_")
                })
                .collect();
            TrackMeta {
                track_id: t.track_id,
                artist_lower,
                album,
                genre_tokens,
                bpm: t.bpm,
                energy: t.energy,
                camelot_key: t.camelot_key.clone(),
            }
        })
        .collect();

    // Build flat indexed arrays for O(1) lookup in the hot loop.
    let fusion_vecs: Vec<&Vec<f64>> = track_metas
        .iter()
        .map(|m| fusion.get(&m.track_id).unwrap())
        .collect();
    let behavioral_vecs: Vec<&Vec<f64>> = track_metas
        .iter()
        .map(|m| {
            behavioral.get(&m.track_id).unwrap_or_else(|| {
                static EMPTY: Vec<f64> = Vec::new();
                &EMPTY
            })
        })
        .collect();
    let audio_vecs: Vec<&Vec<f64>> = track_metas
        .iter()
        .map(|m| {
            audio
                .get(&m.track_id)
                .map(|f| &f.vector)
                .unwrap_or_else(|| {
                    static EMPTY: Vec<f64> = Vec::new();
                    &EMPTY
                })
        })
        .collect();

    // Process each track in parallel, collect neighbors
    let neighbor_chunks: Vec<Vec<TrainerNeighbor>> = (0..track_metas.len())
        .into_par_iter()
        .map(|idx| {
            // Cancel check — cheap atomic load, runs every iteration so Stop is responsive.
            if let Some(flag) = cancel {
                if flag.load(std::sync::atomic::Ordering::Relaxed) {
                    return Vec::<TrainerNeighbor>::new();
                }
            }

            // Progress every 500 tracks
            if idx % 500 == 0 && total > 0 {
                if let Some(tx) = progress_tx {
                    let pct = (idx + 1) * 100 / total;
                    let progress = 0.70 + (pct as f32 / 100.0) * 0.25; // 0.70 to 0.95
                    let _ = tx.send(TrainingProgressUpdate {
                        stage: "neighbors".to_string(),
                        progress,
                        message: format!("{}/{} ({}%)", idx + 1, total, pct),
                        current_track_id: None,
                        current_track_title: None,
                        tracks_done: (idx + 1) as u32,
                        tracks_total: total as u32,
                    });
                }
            }

            let meta = &track_metas[idx];
            let vector = fusion_vecs[idx];
            let b_current = behavioral_vecs[idx];
            let a_current = audio_vecs[idx];
            let seed_co_counts = co_count.get(&meta.track_id);
            let play_count_seed = play_counts.get(&meta.track_id).copied().unwrap_or(0);

            // Per-pair candidate row: includes the new fields except in-degree
            // (filled in by the second pass). primary_reason here is the tag
            // with the largest contribution to this edge's score.
            #[allow(clippy::type_complexity)]
            let mut scores: Vec<(
                i64,
                f64,
                f64,
                f64,
                f64,
                Vec<String>,
                Option<String>,
                f64,
                i64,
                i64,
            )> = Vec::new();

            for (other_idx, other_vector) in fusion_vecs.iter().enumerate() {
                if idx == other_idx {
                    continue;
                }

                // First: cheap fusion cosine. Skip everything if ≤ 0.
                let score = cosine(vector, *other_vector);
                if score <= 0.0 {
                    continue;
                }

                // Behavioral + audio cosines (array index, not HashMap)
                let b_other = behavioral_vecs[other_idx];
                let behavioral_score = cosine(b_current, b_other);

                let a_other = audio_vecs[other_idx];
                let audio_score = cosine(a_current, a_other);

                // Metadata bonuses (pre-computed structs, no string parsing)
                let other_meta = &track_metas[other_idx];
                let mut metadata_score = 0.0f64;
                let mut reason_tags = Vec::new();
                // Track per-tag contribution magnitudes so we can pick a
                // primary_reason as argmax. Behavioral/audio contributions are
                // the cosine score itself (capped to a sensible band) so they
                // can compete with metadata-bonus magnitudes on equal footing.
                let mut contributions: Vec<(&'static str, f64)> = Vec::with_capacity(8);

                // Artist affinity
                if let (Some(cur), Some(oth)) = (&meta.artist_lower, &other_meta.artist_lower) {
                    if cur == oth {
                        metadata_score += 0.2;
                        reason_tags.push("artist_affinity".to_string());
                        contributions.push(("artist_affinity", 0.2));
                    }
                }

                // Genre branch (pre-computed HashSets, no tokenization)
                if !meta.genre_tokens.is_empty()
                    && !other_meta.genre_tokens.is_empty()
                    && meta
                        .genre_tokens
                        .intersection(&other_meta.genre_tokens)
                        .next()
                        .is_some()
                {
                    metadata_score += 0.18;
                    reason_tags.push("genre_branch".to_string());
                    contributions.push(("genre_branch", 0.18));
                }

                // Album context
                if let (Some(cur), Some(oth)) = (&meta.album, &other_meta.album) {
                    if cur == oth {
                        metadata_score += 0.12;
                        reason_tags.push("album_context".to_string());
                        contributions.push(("album_context", 0.12));
                    }
                }

                // BPM proximity — scaled bonus: 0.15 within 3 BPM, 0.08 within 8 BPM
                if let (Some(a_bpm), Some(b_bpm)) = (meta.bpm, other_meta.bpm) {
                    let diff = (a_bpm - b_bpm).abs();
                    if diff <= 3.0 {
                        metadata_score += 0.15;
                        reason_tags.push("bpm_match".to_string());
                        contributions.push(("bpm_match", 0.15));
                    } else if diff <= 8.0 {
                        metadata_score += 0.08;
                        reason_tags.push("bpm_match".to_string());
                        contributions.push(("bpm_match", 0.08));
                    }
                }

                // Camelot key compatibility
                if let (Some(a_key), Some(b_key)) = (&meta.camelot_key, &other_meta.camelot_key) {
                    if a_key == b_key {
                        metadata_score += 0.14;
                        reason_tags.push("harmonic_match".to_string());
                        contributions.push(("harmonic_match", 0.14));
                    } else {
                        // Adjacent keys on the wheel (±1 number, same or relative suffix)
                        let parse_key = |k: &str| -> Option<(i64, String)> {
                            let num_end = k.find(|c: char| c.is_alphabetic())?;
                            let n = k[..num_end].parse::<i64>().ok()?;
                            Some((n, k[num_end..].to_string()))
                        };
                        if let (Some((an, asuf)), Some((bn, bsuf))) =
                            (parse_key(a_key), parse_key(b_key))
                        {
                            let num_diff = (an - bn).abs();
                            let wheel_diff = num_diff.min(12 - num_diff);
                            if wheel_diff <= 1 && asuf == bsuf {
                                metadata_score += 0.10;
                                reason_tags.push("harmonic_match".to_string());
                                contributions.push(("harmonic_match", 0.10));
                            } else if an == bn && asuf != bsuf {
                                metadata_score += 0.08;
                                reason_tags.push("harmonic_match".to_string());
                                contributions.push(("harmonic_match", 0.08));
                            }
                        }
                    }
                }

                // Energy proximity
                if let (Some(ae), Some(be)) = (meta.energy, other_meta.energy) {
                    let diff = (ae - be).abs();
                    if diff <= 0.1 {
                        metadata_score += 0.08;
                        reason_tags.push("energy_match".to_string());
                        contributions.push(("energy_match", 0.08));
                    }
                }

                if behavioral_score > 0.35 {
                    reason_tags.push("behavioral".to_string());
                    contributions.push(("behavioral", behavioral_score));
                }
                if audio_score > 0.35 {
                    reason_tags.push("audio_texture".to_string());
                    contributions.push(("audio_texture", audio_score));
                }

                let total_score = score + metadata_score;
                reason_tags.sort();
                reason_tags.dedup();

                let primary_reason = contributions
                    .iter()
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(k, _)| k.to_string());

                let support_count = seed_co_counts
                    .and_then(|m| m.get(&other_meta.track_id))
                    .copied()
                    .unwrap_or(0);

                // Confidence: behavioral evidence dominates, with metadata-only
                // edges getting a 0.25 floor and fused edges floored at 0.4.
                // The sigmoid+log1p shape means support=0 → 0, support=1 → 0.27,
                // support=5 → 0.51, support=20 → 0.78, support=100 → 0.95.
                let behavioral_confidence = if support_count > 0 {
                    sigmoid((support_count as f64).ln_1p() - 1.5).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let has_audio = !a_current.is_empty() && !a_other.is_empty() && audio_score > 0.0;
                let confidence = if support_count > 0 && has_audio {
                    behavioral_confidence.max(0.4)
                } else if support_count > 0 {
                    behavioral_confidence
                } else {
                    0.25
                };

                let play_count_candidate =
                    play_counts.get(&other_meta.track_id).copied().unwrap_or(0);

                scores.push((
                    other_meta.track_id,
                    total_score,
                    behavioral_score,
                    audio_score,
                    metadata_score,
                    reason_tags,
                    primary_reason,
                    confidence,
                    support_count,
                    play_count_candidate,
                ));
            }

            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scores
                .into_iter()
                .take(top_k)
                .enumerate()
                .map(
                    |(
                        rank,
                        (
                            oid,
                            total_score,
                            bs,
                            as_,
                            ms,
                            tags,
                            primary,
                            conf,
                            support,
                            play_count_candidate,
                        ),
                    )| TrainerNeighbor {
                        track_id: meta.track_id,
                        neighbor_track_id: oid,
                        rank: (rank + 1) as i32,
                        score: total_score,
                        behavioral_score: bs,
                        audio_score: as_,
                        metadata_score: ms,
                        reason_tags: tags,
                        primary_reason: primary,
                        confidence: conf,
                        support_count: support,
                        play_count_seed,
                        play_count_candidate,
                        candidate_in_degree: 0,
                        candidate_in_degree_percentile: 0.0,
                    },
                )
                .collect::<Vec<_>>()
        })
        .collect();

    neighbor_chunks.into_iter().flatten().collect()
}

// Second pass over the assembled neighbor graph: counts how many seeds list
// each track as a neighbor (its in-degree) and computes percentile with average
// rank for ties — a track tied with k others all share the mean of their would-be
// positions, so percentile tuning in the radio re-ranker is stable across libraries
// of different sizes. Tracks never appearing as a neighbor are absent from
// `neighbors` entirely, so they're ignored here; they keep the zero defaults.
fn compute_in_degree(neighbors: &mut [TrainerNeighbor]) {
    if neighbors.is_empty() {
        return;
    }
    let mut counts: HashMap<i64, i64> = HashMap::new();
    for n in neighbors.iter() {
        *counts.entry(n.neighbor_track_id).or_default() += 1;
    }

    let mut ordered: Vec<(i64, i64)> = counts.iter().map(|(&id, &c)| (id, c)).collect();
    ordered.sort_by_key(|&(_, c)| c);
    let n = ordered.len() as f64;

    let mut percentile_map: HashMap<i64, f64> = HashMap::new();
    let mut i = 0;
    while i < ordered.len() {
        let cur_count = ordered[i].1;
        let mut j = i;
        while j < ordered.len() && ordered[j].1 == cur_count {
            j += 1;
        }
        // Average rank over the tied group [i..j) using 0-indexed positions.
        let avg_rank = (i + j - 1) as f64 / 2.0;
        let pct = if n > 0.0 { avg_rank / n } else { 0.0 };
        for k in i..j {
            percentile_map.insert(ordered[k].0, pct);
        }
        i = j;
    }

    for nb in neighbors.iter_mut() {
        let id = nb.neighbor_track_id;
        nb.candidate_in_degree = counts.get(&id).copied().unwrap_or(0);
        nb.candidate_in_degree_percentile = percentile_map.get(&id).copied().unwrap_or(0.0);
    }
}

// ── Stage 5: Evaluation ───────────────────────────────────────────────────────

fn evaluate(neighbors: &[TrainerNeighbor], heldout_pairs: &[(i64, i64)]) -> HashMap<String, f64> {
    if heldout_pairs.is_empty() {
        return HashMap::from([
            ("recall_at_10".to_string(), 0.0),
            ("mrr_at_20".to_string(), 0.0),
        ]);
    }

    // Group neighbors by source track_id
    let mut grouped: HashMap<i64, Vec<i64>> = HashMap::new();
    for n in neighbors {
        grouped
            .entry(n.track_id)
            .or_default()
            .push(n.neighbor_track_id);
    }

    let mut hits = 0usize;
    let mut reciprocal_rank = 0.0f64;

    for &(source, target) in heldout_pairs {
        let ranked = grouped.get(&source).map(|v| v.as_slice()).unwrap_or(&[]);
        let top_20 = &ranked[..ranked.len().min(20)];

        if top_20.iter().take(10).any(|&id| id == target) {
            hits += 1;
        }
        if let Some(pos) = top_20.iter().position(|&id| id == target) {
            reciprocal_rank += 1.0 / (pos as f64 + 1.0);
        }
    }

    let total = heldout_pairs.len() as f64;
    HashMap::from([
        ("recall_at_10".to_string(), hits as f64 / total),
        ("mrr_at_20".to_string(), reciprocal_rank / total),
    ])
}

// Buckets each top-20 prediction by its primary_reason and tracks impressions /
// hits / mean rank / MRR per bucket. The output tells you, e.g., "of all the
// edges we surfaced because of `harmonic_match`, 6% of them were the actual
// next track in held-out data" — directly answering whether the hardcoded
// metadata bonus weights match their actual predictive value.
fn compute_reason_hit_rates(
    neighbors: &[TrainerNeighbor],
    heldout_pairs: &[(i64, i64)],
) -> Vec<ReasonHitRate> {
    if heldout_pairs.is_empty() || neighbors.is_empty() {
        return Vec::new();
    }

    // Group neighbors by seed, in the order the trainer emitted them.
    // similarity_neighbors sorts by score descending and assigns rank, then
    // flatten preserves that order — so position-in-vec equals (rank - 1).
    let mut grouped: HashMap<i64, Vec<(i64, Option<String>)>> = HashMap::new();
    for n in neighbors {
        grouped
            .entry(n.track_id)
            .or_default()
            .push((n.neighbor_track_id, n.primary_reason.clone()));
    }

    // (impressions, hits, rank_sum_for_hits, mrr_contribution)
    let mut acc: HashMap<String, (i64, i64, f64, f64)> = HashMap::new();

    for &(source, target) in heldout_pairs {
        let Some(ranked) = grouped.get(&source) else {
            continue;
        };
        for (idx, (nid, primary)) in ranked.iter().take(20).enumerate() {
            let key = primary.clone().unwrap_or_else(|| "unspecified".to_string());
            let entry = acc.entry(key).or_insert((0, 0, 0.0, 0.0));
            entry.0 += 1;
            if *nid == target {
                entry.1 += 1;
                let rank = (idx + 1) as f64;
                entry.2 += rank;
                entry.3 += 1.0 / rank;
            }
        }
    }

    let mut out: Vec<ReasonHitRate> = acc
        .into_iter()
        .map(|(reason, (impressions, hits, rank_sum, mrr))| {
            let hit_rate = if impressions > 0 {
                hits as f64 / impressions as f64
            } else {
                0.0
            };
            let mean_rank = if hits > 0 {
                Some(rank_sum / hits as f64)
            } else {
                None
            };
            ReasonHitRate {
                primary_reason: reason,
                impressions,
                hits,
                hit_rate,
                mean_rank,
                mrr_contribution: mrr,
                insufficient_data: impressions < MIN_REASON_IMPRESSIONS,
            }
        })
        .collect();
    // Stable, descending by impressions — most-evidence reasons surface first
    // when an operator skims the table.
    out.sort_by(|a, b| b.impressions.cmp(&a.impressions));
    out
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Run the full discovery training pipeline in Rust.
/// Returns the complete trainer output ready for DB persistence.
///
/// `progress_tx` is optional — if provided, sends TrainingProgressUpdate structs.
pub fn run_discovery_training(
    input: TrainerInput,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<TrainingProgressUpdate>>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> TrainerOutput {
    let dim = input.dimension;
    let top_k = input.top_k;
    let tracks = input.tracks.clone();

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "behavioral",
            "Building co-occurrence embeddings",
            0.20,
        ));
    }

    // Stage 1 — also returns raw co-occurrence counts for support/confidence.
    let (behavioral, co_count) = build_behavioral_embeddings(&input, cancel);
    if cancel_requested(cancel) {
        return TrainerOutput {
            behavioral_embeddings: HashMap::new(),
            audio_features: HashMap::new(),
            fusion_embeddings: HashMap::new(),
            neighbors: Vec::new(),
            metrics: HashMap::new(),
            reason_hit_rates: Vec::new(),
        };
    }

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "audio",
            &format!("Building proxy features for {} tracks", tracks.len()),
            0.40,
        ));
    }

    // Stage 2 — skipped entirely on Low intensity. Fuse_embeddings handles an
    // empty audio map by falling back to behavioral-only vectors, so the
    // pipeline still produces a graph; cold tracks just have less to anchor on.
    let audio = if input.include_audio_proxy {
        build_audio_proxy_features(&tracks, dim, cancel)
    } else {
        if let Some(tx) = progress_tx {
            let _ = tx.send(TrainingProgressUpdate::stage_only(
                "audio",
                "Skipping audio-proxy stage (Low intensity)",
                0.55,
            ));
        }
        HashMap::new()
    };
    if cancel_requested(cancel) {
        return TrainerOutput {
            behavioral_embeddings: behavioral,
            audio_features: audio,
            fusion_embeddings: HashMap::new(),
            neighbors: Vec::new(),
            metrics: HashMap::new(),
            reason_hit_rates: Vec::new(),
        };
    }

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "fusion",
            "Blending behavioral + audio",
            0.55,
        ));
    }

    // Stage 3
    let fusion = fuse_embeddings(&tracks, &behavioral, &audio);

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "neighbors",
            &format!(
                "Building similarity graph for {} tracks (O(n²) parallel)",
                fusion.len()
            ),
            0.70,
        ));
    }

    let play_counts: HashMap<i64, i64> = tracks
        .iter()
        .map(|t| (t.track_id, t.play_count as i64))
        .collect();

    // Stage 4 — the bottleneck, now parallelized
    let mut neighbors = similarity_neighbors(
        &tracks,
        &behavioral,
        &audio,
        &fusion,
        &co_count,
        &play_counts,
        top_k,
        progress_tx,
        cancel,
    );

    // Stage 4b — second pass: hub-detection. Counts how many seeds each
    // candidate appears for, then assigns percentile (avg-rank for ties) so the
    // radio re-ranker can apply a hub penalty in [0, 1] regardless of library size.
    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "in_degree",
            &format!("Computing in-degree for {} edges", neighbors.len()),
            0.95,
        ));
    }
    compute_in_degree(&mut neighbors);

    // Stage 5 — recall/MRR overall, plus per-reason hit-rate breakdown.
    let mut metrics = evaluate(&neighbors, &input.heldout_pairs);
    let reason_hit_rates = compute_reason_hit_rates(&neighbors, &input.heldout_pairs);

    let playable = tracks.len() as f64;
    let embedded = fusion.len() as f64;
    metrics.insert(
        "coverage_ratio".to_string(),
        if playable > 0.0 {
            embedded / playable
        } else {
            0.0
        },
    );
    metrics.insert("playable_tracks".to_string(), playable);
    metrics.insert("embedded_tracks".to_string(), embedded);
    let total_sequences: usize = input.sequences.iter().map(|g| g.sequences.len()).sum();
    metrics.insert("sequence_count".to_string(), total_sequences as f64);
    metrics.insert(
        "neighbor_tracks".to_string(),
        neighbors
            .iter()
            .map(|n| n.track_id)
            .collect::<HashSet<_>>()
            .len() as f64,
    );

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "evaluate",
            &format!(
                "Done: coverage {:.1}%, recall@10 {:.1}%, {} neighbors",
                metrics["coverage_ratio"] * 100.0,
                metrics["recall_at_10"] * 100.0,
                neighbors.len()
            ),
            0.96,
        ));
    }

    TrainerOutput {
        behavioral_embeddings: behavioral,
        audio_features: audio,
        fusion_embeddings: fusion,
        neighbors,
        metrics,
        reason_hit_rates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn make_test_input(
        track_count: usize,
        dim: usize,
    ) -> (
        Vec<EmbeddingTrackRow>,
        HashMap<i64, Vec<f64>>,
        HashMap<i64, TrainerAudioFeature>,
        HashMap<i64, Vec<f64>>,
    ) {
        let tracks: Vec<EmbeddingTrackRow> = (0..track_count as i64)
            .map(|i| EmbeddingTrackRow {
                track_id: i,
                title: format!("track_{i}"),
                artist_name: Some(format!("artist_{}", i / 10)),
                album_title: None,
                duration_ms: Some(180_000),
                best_quality: None,
                source: "local".to_string(),
                play_count: 0,
                is_favorite: false,
                playlist_memberships: 0,
                genre_paths: Vec::new(),
                bpm: None,
                energy: None,
                camelot_key: None,
            })
            .collect();

        let unit = 1.0_f64 / (dim as f64).sqrt();
        let behavioral: HashMap<i64, Vec<f64>> = tracks
            .iter()
            .map(|t| (t.track_id, vec![unit; dim]))
            .collect();
        let audio: HashMap<i64, TrainerAudioFeature> = tracks
            .iter()
            .map(|t| {
                (
                    t.track_id,
                    TrainerAudioFeature {
                        vector: vec![unit; dim],
                        clip_start_ms: 0,
                        clip_duration_ms: 20_000,
                        feature_version: "test".to_string(),
                    },
                )
            })
            .collect();
        let fusion: HashMap<i64, Vec<f64>> = tracks
            .iter()
            .map(|t| (t.track_id, vec![unit; dim]))
            .collect();

        (tracks, behavioral, audio, fusion)
    }

    #[test]
    fn similarity_neighbors_aborts_when_cancel_flag_set() {
        let (tracks, behavioral, audio, fusion) = make_test_input(200, 32);
        let cancel = Arc::new(AtomicBool::new(true));
        let co_count = HashMap::new();
        let play_counts = HashMap::new();

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            &co_count,
            &play_counts,
            10,
            None,
            Some(&cancel),
        );

        assert!(
            result.is_empty(),
            "expected zero neighbors when cancel is pre-set, got {}",
            result.len(),
        );
    }

    #[test]
    fn similarity_neighbors_runs_normally_without_cancel() {
        let (tracks, behavioral, audio, fusion) = make_test_input(50, 32);
        let cancel = Arc::new(AtomicBool::new(false));
        let co_count = HashMap::new();
        let play_counts = HashMap::new();

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            &co_count,
            &play_counts,
            10,
            None,
            Some(&cancel),
        );

        // 50 tracks × top_k=10, all vectors identical → every track has 10 neighbors
        assert_eq!(result.len(), 500, "expected 50*10 = 500 neighbor rows");
    }

    #[test]
    fn confidence_floors_for_metadata_only_edges() {
        // No co_count entries → every edge is "pure metadata", confidence = 0.25.
        let (tracks, behavioral, audio, fusion) = make_test_input(20, 16);
        let cancel = Arc::new(AtomicBool::new(false));
        let co_count = HashMap::new();
        let play_counts = HashMap::new();

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            &co_count,
            &play_counts,
            5,
            None,
            Some(&cancel),
        );

        assert!(!result.is_empty());
        assert!(
            result.iter().all(|n| (n.confidence - 0.25).abs() < 1e-6),
            "all metadata-only edges should sit at the 0.25 confidence floor",
        );
        assert!(result.iter().all(|n| n.support_count == 0));
    }

    #[test]
    fn confidence_grows_with_support() {
        let (tracks, behavioral, audio, fusion) = make_test_input(10, 16);
        let cancel = Arc::new(AtomicBool::new(false));
        // Edge from track 0 → track 1 has 50 supporting events; everyone else has 0.
        let mut co_count: HashMap<i64, HashMap<i64, i64>> = HashMap::new();
        co_count
            .entry(tracks[0].track_id)
            .or_default()
            .insert(tracks[1].track_id, 50);
        let play_counts = HashMap::new();

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            &co_count,
            &play_counts,
            5,
            None,
            Some(&cancel),
        );

        let strongly_supported = result
            .iter()
            .find(|n| n.track_id == tracks[0].track_id && n.neighbor_track_id == tracks[1].track_id)
            .expect("edge present");
        assert!(
            strongly_supported.confidence > 0.7,
            "expected strong-evidence edge confidence > 0.7, got {}",
            strongly_supported.confidence,
        );
        assert_eq!(strongly_supported.support_count, 50);
    }

    #[test]
    fn reason_hit_rates_bucket_by_primary_reason() {
        let mk = |seed: i64, neighbor: i64, primary: &str| TrainerNeighbor {
            track_id: seed,
            neighbor_track_id: neighbor,
            rank: 1,
            score: 0.0,
            behavioral_score: 0.0,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_tags: vec![primary.to_string()],
            primary_reason: Some(primary.to_string()),
            confidence: 0.0,
            support_count: 0,
            play_count_seed: 0,
            play_count_candidate: 0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        };
        // Seed 1 has neighbors [10 behavioral, 20 harmonic_match, 30 behavioral].
        // Held-out target for seed 1 is 10 → behavioral hits, harmonic misses.
        let neighbors = vec![
            mk(1, 10, "behavioral"),
            mk(1, 20, "harmonic_match"),
            mk(1, 30, "behavioral"),
        ];
        let heldout = vec![(1, 10)];
        let rates = compute_reason_hit_rates(&neighbors, &heldout);

        let beh = rates
            .iter()
            .find(|r| r.primary_reason == "behavioral")
            .unwrap();
        assert_eq!(beh.impressions, 2);
        assert_eq!(beh.hits, 1);
        assert!((beh.hit_rate - 0.5).abs() < 1e-9);

        let harm = rates
            .iter()
            .find(|r| r.primary_reason == "harmonic_match")
            .unwrap();
        assert_eq!(harm.impressions, 1);
        assert_eq!(harm.hits, 0);
        assert_eq!(harm.hit_rate, 0.0);
    }

    #[test]
    fn reason_hit_rates_flag_insufficient_data() {
        // 1 impression < MIN_REASON_IMPRESSIONS (20) → insufficient_data = true.
        let mk = |seed: i64, neighbor: i64, primary: &str| TrainerNeighbor {
            track_id: seed,
            neighbor_track_id: neighbor,
            rank: 1,
            score: 0.0,
            behavioral_score: 0.0,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_tags: vec![primary.to_string()],
            primary_reason: Some(primary.to_string()),
            confidence: 0.0,
            support_count: 0,
            play_count_seed: 0,
            play_count_candidate: 0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        };
        let neighbors = vec![mk(1, 10, "rare_tag")];
        let heldout = vec![(1, 10)];
        let rates = compute_reason_hit_rates(&neighbors, &heldout);
        assert!(rates[0].insufficient_data);
    }

    #[test]
    fn compute_in_degree_uses_average_rank_for_ties() {
        // Hand-rolled neighbor list: candidate A has in-degree 1, B has 1, C has 3.
        let mk = |track_id: i64, neighbor_id: i64| TrainerNeighbor {
            track_id,
            neighbor_track_id: neighbor_id,
            rank: 1,
            score: 0.0,
            behavioral_score: 0.0,
            audio_score: 0.0,
            metadata_score: 0.0,
            reason_tags: vec![],
            primary_reason: None,
            confidence: 0.0,
            support_count: 0,
            play_count_seed: 0,
            play_count_candidate: 0,
            candidate_in_degree: 0,
            candidate_in_degree_percentile: 0.0,
        };
        let mut neighbors = vec![
            mk(1, 100), // A
            mk(2, 200), // B
            mk(3, 300), // C
            mk(4, 300), // C
            mk(5, 300), // C
        ];
        compute_in_degree(&mut neighbors);

        let pct_a = neighbors
            .iter()
            .find(|n| n.neighbor_track_id == 100)
            .unwrap()
            .candidate_in_degree_percentile;
        let pct_b = neighbors
            .iter()
            .find(|n| n.neighbor_track_id == 200)
            .unwrap()
            .candidate_in_degree_percentile;
        let pct_c = neighbors
            .iter()
            .find(|n| n.neighbor_track_id == 300)
            .unwrap()
            .candidate_in_degree_percentile;

        // 3 distinct tracks, ranks: A,B share count 1 (avg 0.5), C alone at count 3 (rank 2).
        // percentile = avg_rank / N where N=3.
        assert!((pct_a - 0.5 / 3.0).abs() < 1e-9, "A pct = {}", pct_a);
        assert!((pct_b - 0.5 / 3.0).abs() < 1e-9, "B pct = {}", pct_b);
        assert!((pct_c - 2.0 / 3.0).abs() < 1e-9, "C pct = {}", pct_c);

        let in_a = neighbors
            .iter()
            .find(|n| n.neighbor_track_id == 100)
            .unwrap()
            .candidate_in_degree;
        let in_c = neighbors
            .iter()
            .find(|n| n.neighbor_track_id == 300)
            .unwrap()
            .candidate_in_degree;
        assert_eq!(in_a, 1);
        assert_eq!(in_c, 3);
    }
}
