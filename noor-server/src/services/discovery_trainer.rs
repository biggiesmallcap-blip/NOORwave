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
    pub tracks: Vec<EmbeddingTrackRow>,
    pub sequences: Vec<TrainerSequenceGroup>,
    pub heldout_pairs: Vec<(i64, i64)>,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct TrainerOutput {
    pub behavioral_embeddings: HashMap<i64, Vec<f64>>,
    pub audio_features: HashMap<i64, TrainerAudioFeature>,
    pub fusion_embeddings: HashMap<i64, Vec<f64>>,
    pub neighbors: Vec<TrainerNeighbor>,
    pub metrics: HashMap<String, f64>,
}

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
            let sign = if digest[offset + 1] % 2 == 0 { 1.0 } else { -1.0 };
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
        let normalized = value
            .to_lowercase()
            .replace('/', " ")
            .replace('-', " ");
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

fn build_behavioral_embeddings(input: &TrainerInput) -> HashMap<i64, Vec<f64>> {
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
    let mut co: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    for source in &input.sequences {
        let weight = source.weight;
        for sequence in &source.sequences {
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
                for j in left..right {
                    if i == j {
                        continue;
                    }
                    let other = filtered[j];
                    let distance = (i as isize - j as isize).unsigned_abs();
                    *entry.entry(other).or_default() += weight / (distance as f64).max(1.0);
                }
            }
        }
    }

    // Hash-project each track's neighbor scores into a dense vector
    co.into_par_iter()
        .map(|(track_id, neighbors)| {
            let mut vec = vec![0.0f64; dim];
            for (&other, &score) in &neighbors {
                // Use a deterministic hash for the (track_id, other) pair
                let key = format!("{track_id}:{other}");
                let digest = Sha256::digest(key.as_bytes());
                let limit = usize::min(32, dim * 2);
                for offset in (0..limit).step_by(2) {
                    let bucket = digest[offset] as usize % dim;
                    let sign = if digest[offset + 1] % 2 == 0 { 1.0 } else { -1.0 };
                    vec[bucket] += sign * score;
                }
            }
            (track_id, normalize(&vec).0)
        })
        .collect()
}

// ── Stage 2: Audio proxy features (hashed metadata) ───────────────────────────

fn build_audio_proxy_features(
    tracks: &[EmbeddingTrackRow],
    dim: usize,
) -> HashMap<i64, TrainerAudioFeature> {
    tracks
        .iter()
        .map(|track| {
            let clip_duration = 20_000i64;
            let duration = track.duration_ms.unwrap_or(0);
            let clip_start = if duration >= 90_000 {
                30_000
            } else {
                (duration - clip_duration).max(0) / 2
            };
            let tokens = metadata_tokens(track);
            let vec = hashed_projection(&tokens, dim);
            (
                track.track_id,
                TrainerAudioFeature {
                    vector: vec,
                    clip_start_ms: clip_start,
                    clip_duration_ms: clip_duration,
                    feature_version: "metadata-audio-proxy-v1".to_string(),
                },
            )
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

fn similarity_neighbors(
    tracks: &[EmbeddingTrackRow],
    behavioral: &HashMap<i64, Vec<f64>>,
    audio: &HashMap<i64, TrainerAudioFeature>,
    fusion: &HashMap<i64, Vec<f64>>,
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
                .filter(|tok| !tok.starts_with("dur_") && !tok.starts_with("bpm") && !tok.starts_with("energy_") && !tok.starts_with("key_"))
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
        .map(|m| behavioral.get(&m.track_id).unwrap_or_else(|| {
            static EMPTY: Vec<f64> = Vec::new();
            &EMPTY
        }))
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

            let mut scores: Vec<(i64, f64, f64, f64, f64, Vec<String>)> = Vec::new();

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

                // Artist affinity
                if let (Some(cur), Some(oth)) = (&meta.artist_lower, &other_meta.artist_lower) {
                    if cur == oth {
                        metadata_score += 0.2;
                        reason_tags.push("artist_affinity".to_string());
                    }
                }

                // Genre branch (pre-computed HashSets, no tokenization)
                if !meta.genre_tokens.is_empty()
                    && !other_meta.genre_tokens.is_empty()
                    && meta.genre_tokens.intersection(&other_meta.genre_tokens).next().is_some()
                {
                    metadata_score += 0.18;
                    reason_tags.push("genre_branch".to_string());
                }

                // Album context
                if let (Some(cur), Some(oth)) = (&meta.album, &other_meta.album) {
                    if cur == oth {
                        metadata_score += 0.12;
                        reason_tags.push("album_context".to_string());
                    }
                }

                // BPM proximity — scaled bonus: 0.15 within 3 BPM, 0.08 within 8 BPM
                if let (Some(a_bpm), Some(b_bpm)) = (meta.bpm, other_meta.bpm) {
                    let diff = (a_bpm - b_bpm).abs();
                    if diff <= 3.0 {
                        metadata_score += 0.15;
                        reason_tags.push("bpm_match".to_string());
                    } else if diff <= 8.0 {
                        metadata_score += 0.08;
                        reason_tags.push("bpm_match".to_string());
                    }
                }

                // Camelot key compatibility
                if let (Some(a_key), Some(b_key)) = (&meta.camelot_key, &other_meta.camelot_key) {
                    if a_key == b_key {
                        metadata_score += 0.14;
                        reason_tags.push("harmonic_match".to_string());
                    } else {
                        // Adjacent keys on the wheel (±1 number, same or relative suffix)
                        let parse_key = |k: &str| -> Option<(i64, String)> {
                            let num_end = k.find(|c: char| c.is_alphabetic())?;
                            let n = k[..num_end].parse::<i64>().ok()?;
                            Some((n, k[num_end..].to_string()))
                        };
                        if let (Some((an, asuf)), Some((bn, bsuf))) = (parse_key(a_key), parse_key(b_key)) {
                            let num_diff = (an - bn).abs();
                            let wheel_diff = num_diff.min(12 - num_diff);
                            if wheel_diff <= 1 && asuf == bsuf {
                                metadata_score += 0.10;
                                reason_tags.push("harmonic_match".to_string());
                            } else if an == bn && asuf != bsuf {
                                metadata_score += 0.08;
                                reason_tags.push("harmonic_match".to_string());
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
                    }
                }

                if behavioral_score > 0.35 {
                    reason_tags.push("behavioral".to_string());
                }
                if audio_score > 0.35 {
                    reason_tags.push("audio_texture".to_string());
                }

                let total_score = score + metadata_score;
                reason_tags.sort();
                reason_tags.dedup();
                scores.push((
                    other_meta.track_id,
                    total_score,
                    behavioral_score,
                    audio_score,
                    metadata_score,
                    reason_tags,
                ));
            }

            scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scores
                .into_iter()
                .take(top_k)
                .enumerate()
                .map(|(rank, (oid, total_score, bs, as_, ms, tags))| TrainerNeighbor {
                    track_id: meta.track_id,
                    neighbor_track_id: oid,
                    rank: (rank + 1) as i32,
                    score: total_score,
                    behavioral_score: bs,
                    audio_score: as_,
                    metadata_score: ms,
                    reason_tags: tags,
                })
                .collect::<Vec<_>>()
        })
        .collect();

    neighbor_chunks.into_iter().flatten().collect()
}

// ── Stage 5: Evaluation ───────────────────────────────────────────────────────

fn evaluate(
    neighbors: &[TrainerNeighbor],
    heldout_pairs: &[(i64, i64)],
) -> HashMap<String, f64> {
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
        (
            "mrr_at_20".to_string(),
            reciprocal_rank / total,
        ),
    ])
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

    // Stage 1
    let behavioral = build_behavioral_embeddings(&input);

    if let Some(tx) = progress_tx {
        let _ = tx.send(TrainingProgressUpdate::stage_only(
            "audio",
            &format!("Building proxy features for {} tracks", tracks.len()),
            0.40,
        ));
    }

    // Stage 2
    let audio = build_audio_proxy_features(&tracks, dim);

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
            &format!("Building similarity graph for {} tracks (O(n²) parallel)", fusion.len()),
            0.70,
        ));
    }

    // Stage 4 — the bottleneck, now parallelized
    let neighbors = similarity_neighbors(&tracks, &behavioral, &audio, &fusion, top_k, progress_tx, cancel);

    // Stage 5
    let mut metrics = evaluate(&neighbors, &input.heldout_pairs);

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn make_test_input(track_count: usize, dim: usize) -> (
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

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
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

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            10,
            None,
            Some(&cancel),
        );

        // 50 tracks × top_k=10, all vectors identical → every track has 10 neighbors
        assert_eq!(result.len(), 500, "expected 50*10 = 500 neighbor rows");
    }
}
