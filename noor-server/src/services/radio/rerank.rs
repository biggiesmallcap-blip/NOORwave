//! Blend interleave, diversity re-rank and the hard same-artist cap.

use super::*;

pub(super) fn blend_interleave(
    candidates: Vec<RadioCandidate>,
    blend: RadioBlend,
    limit: usize,
) -> Vec<RadioCandidate> {
    let (lib_w, lfm_w, eng_w) = blend.weights();
    let mut by_source: std::collections::HashMap<RadioSource, Vec<RadioCandidate>> =
        std::collections::HashMap::new();
    for c in candidates {
        by_source.entry(c.source).or_default().push(c);
    }
    for v in by_source.values_mut() {
        v.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let lib_avail = by_source.get(&RadioSource::Library).map_or(0, |v| v.len());
    let lfm_avail = by_source.get(&RadioSource::Lastfm).map_or(0, |v| v.len());
    let eng_avail = by_source.get(&RadioSource::Engine).map_or(0, |v| v.len());
    let lib_take = ((limit as f64 * lib_w).round() as usize).min(lib_avail);
    let lfm_take = ((limit as f64 * lfm_w).round() as usize).min(lfm_avail);
    let eng_take = ((limit as f64 * eng_w).round() as usize).min(eng_avail);

    let mut lib_iter = by_source
        .remove(&RadioSource::Library)
        .unwrap_or_default()
        .into_iter()
        .take(lib_take);
    let mut lfm_iter = by_source
        .remove(&RadioSource::Lastfm)
        .unwrap_or_default()
        .into_iter()
        .take(lfm_take);
    let mut eng_iter = by_source
        .remove(&RadioSource::Engine)
        .unwrap_or_default()
        .into_iter()
        .take(eng_take);

    let mut out = Vec::with_capacity(limit);
    let mut lib_done = 0usize;
    let mut lfm_done = 0usize;
    let mut eng_done = 0usize;

    while out.len() < limit {
        let lib_behind = (lib_take as f64 - lib_done as f64) / lib_w.max(0.01);
        let lfm_behind = (lfm_take as f64 - lfm_done as f64) / lfm_w.max(0.01);
        let eng_behind = (eng_take as f64 - eng_done as f64) / eng_w.max(0.01);

        let pick = if lib_behind >= lfm_behind && lib_behind >= eng_behind {
            lib_iter.next().inspect(|_| {
                lib_done += 1;
            })
        } else if lfm_behind >= eng_behind {
            lfm_iter.next().inspect(|_| {
                lfm_done += 1;
            })
        } else {
            eng_iter.next().inspect(|_| {
                eng_done += 1;
            })
        };

        match pick {
            Some(c) => out.push(c),
            None => {
                if let Some(c) = lib_iter.next() {
                    lib_done += 1;
                    out.push(c);
                } else if let Some(c) = lfm_iter.next() {
                    lfm_done += 1;
                    out.push(c);
                } else if let Some(c) = eng_iter.next() {
                    eng_done += 1;
                    out.push(c);
                } else {
                    break;
                }
            }
        }
    }
    out
}

// Counters tallied by diversity_rerank for the radio_diagnostics row. Each
// counts how many *picks* triggered the corresponding penalty, not how many
// candidates were considered — so a value of 3 means 3 of the final queue's
// slots had to push through that penalty to be placed.
#[derive(Debug, Clone, Default)]
pub(super) struct RerankCounters {
    pub(super) same_artist_penalties: i64,
    pub(super) same_album_penalties: i64,
    pub(super) genre_saturation_penalties: i64,
    pub(super) repetition_skips: i64,
    pub(super) penalty_relaxations: i64,
}

// Pulls one primary genre token per candidate track. "Primary" = the root of
// the most-frequently-occurring genre path — `Electronic > House > Deep House`
// becomes "electronic". Lowercased so saturation-counting is case-insensitive.
pub(super) fn primary_genres_for_candidates(
    db: &Database,
    candidates: &[RadioCandidate],
) -> HashMap<i64, String> {
    let ids: Vec<i64> = candidates
        .iter()
        .map(|c| c.track_id)
        .filter(|id| *id > 0)
        .collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    let resolved_by_track = match db
        .with_conn(move |conn| crate::db::queries::get_genres_for_tracks_with_fallback(conn, &ids))
    {
        Ok(m) => m,
        Err(err) => {
            tracing::debug!(
                "radio: primary-genre lookup failed ({err:#}); diversity rerank will skip genre saturation"
            );
            return HashMap::new();
        }
    };
    let mut out = HashMap::with_capacity(resolved_by_track.len());
    for (track_id, rows) in resolved_by_track {
        let mut counts: HashMap<String, i32> = HashMap::new();
        for row in &rows {
            let root = row
                .path
                .split(" > ")
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !root.is_empty() {
                *counts.entry(root).or_default() += 1;
            }
        }
        if let Some((root, _)) = counts.into_iter().max_by_key(|(_, c)| *c) {
            out.insert(track_id, root);
        }
    }
    out
}

// Tracks which penalties fired during a single (candidate, queue) scoring.
// Used by diversity_rerank to increment the diagnostic counters once per
// final pick (not per scoring-attempt — we don't want relaxation passes to
// double-count).
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct PenaltyHits {
    pub(super) artist: bool,
    pub(super) album: bool,
    pub(super) genre_saturation: bool,
}

// Score a single candidate against the running queue with optional penalty
// dimensions disabled (for the relaxation pass). Returns (penalized_score,
// hit_flags). Lower is worse; a negative score means every penalty bit landed.
pub(super) fn score_candidate_for_slot(
    cand: &RadioCandidate,
    queue: &[RadioCandidate],
    profile: &crate::services::radio_config::RadioProfile,
    primary_genres: &HashMap<i64, String>,
    drop_artist: bool,
    drop_album: bool,
    drop_genre: bool,
) -> (f64, PenaltyHits) {
    let mut score = cand.similarity_score;
    let mut hits = PenaltyHits::default();
    let weight = profile.diversity_weight;

    if !drop_artist && profile.same_artist_penalty > 0.0 && weight > 0.0 {
        let lo = queue.len().saturating_sub(5);
        if queue[lo..]
            .iter()
            .any(|q| q.artist_name.eq_ignore_ascii_case(&cand.artist_name))
        {
            score -= profile.same_artist_penalty * weight;
            hits.artist = true;
        }
    }

    if !drop_album
        && profile.same_album_penalty > 0.0
        && weight > 0.0
        && let Some(my_album) = cand.album_title.as_deref()
    {
        let lo = queue.len().saturating_sub(8);
        if queue[lo..].iter().any(|q| {
            q.album_title
                .as_deref()
                .map(|a| a.eq_ignore_ascii_case(my_album))
                .unwrap_or(false)
        }) {
            score -= profile.same_album_penalty * weight;
            hits.album = true;
        }
    }

    if !drop_genre
        && profile.genre_saturation_penalty > 0.0
        && weight > 0.0
        && let Some(my_genre) = primary_genres.get(&cand.track_id)
    {
        let lo = queue.len().saturating_sub(10);
        let count = queue[lo..]
            .iter()
            .filter(|q| {
                primary_genres
                    .get(&q.track_id)
                    .map(|g| g == my_genre)
                    .unwrap_or(false)
            })
            .count();
        // Threshold: penalty fires only above 3 in the last 10. Saturating
        // sub avoids underflow for counts < 3.
        let excess = count.saturating_sub(3) as f64;
        if excess > 0.0 {
            score -= profile.genre_saturation_penalty * weight * excess;
            hits.genre_saturation = true;
        }
    }

    (score, hits)
}

// Constraint-based greedy slot-fill replacing blend_interleave. At each slot,
// scores every eligible candidate against the queue-so-far, applies penalties,
// optionally biases toward under-quota sources, and picks the argmax. If the
// best score after penalties is non-positive, relaxes one dimension at a time
// (genre → album → artist) until something positive emerges.
//
// Hard skips: tracks in `recent_track_ids` (skipped or recently-played) are
// dropped from the eligible pool entirely. If the pool empties before the
// queue fills, the function returns short — the caller logs `repetition_skips`
// in diagnostics. No fallback to a wider candidate pool here; the orchestrator
// upstream can decide whether that's acceptable.
pub(super) fn diversity_rerank(
    candidates: Vec<RadioCandidate>,
    profile: &crate::services::radio_config::RadioProfile,
    blend: RadioBlend,
    limit: usize,
    primary_genres: &HashMap<i64, String>,
    recent_track_ids: &HashSet<i64>,
    apply_source_quota: bool,
    counters: &mut RerankCounters,
) -> Vec<RadioCandidate> {
    let target_size = limit.min(candidates.len());
    let mut available = candidates;
    let mut queue: Vec<RadioCandidate> = Vec::with_capacity(target_size);
    let mut source_counts: HashMap<RadioSource, i64> = HashMap::new();
    let (lib_w, lfm_w, eng_w) = blend.weights();
    const SOURCE_QUOTA_BONUS: f64 = 1.05;

    while queue.len() < target_size && !available.is_empty() {
        // Filter to eligible: not in recent_track_ids. Track-id 0 (lastfm rows)
        // pass the filter regardless since they're not in the library set.
        let eligible_indices: Vec<usize> = (0..available.len())
            .filter(|&i| {
                let c = &available[i];
                c.track_id == 0 || !recent_track_ids.contains(&c.track_id)
            })
            .collect();
        if eligible_indices.is_empty() {
            counters.repetition_skips += (target_size - queue.len()) as i64;
            break;
        }

        // Try with full penalties first; relax progressively if every score
        // comes out non-positive.
        let mut chosen: Option<(usize, PenaltyHits)> = None;
        let mut relaxation_used = false;
        let relaxation_steps = [
            (false, false, false), // full penalties
            (false, false, true),  // drop genre_saturation
            (false, true, true),   // drop album too
            (true, true, true),    // drop artist too — no penalties left
        ];

        for (step_idx, &(drop_artist, drop_album, drop_genre)) in
            relaxation_steps.iter().enumerate()
        {
            let mut best: Option<(usize, f64, PenaltyHits)> = None;

            for &idx in &eligible_indices {
                let cand = &available[idx];
                let (mut score, hits) = score_candidate_for_slot(
                    cand,
                    &queue,
                    profile,
                    primary_genres,
                    drop_artist,
                    drop_album,
                    drop_genre,
                );
                if apply_source_quota {
                    let target_for_source = match cand.source {
                        RadioSource::Library => lib_w,
                        RadioSource::Lastfm | RadioSource::Tidal => lfm_w,
                        RadioSource::Engine => eng_w,
                    } * (queue.len() as f64);
                    let actual = *source_counts.get(&cand.source).unwrap_or(&0) as f64;
                    if actual < target_for_source {
                        score *= SOURCE_QUOTA_BONUS;
                    }
                }
                if best.map(|(_, b_score, _)| score > b_score).unwrap_or(true) {
                    best = Some((idx, score, hits));
                }
            }

            match best {
                Some((idx, score, hits))
                    if score > 0.0 || step_idx == relaxation_steps.len() - 1 =>
                {
                    chosen = Some((idx, hits));
                    if step_idx > 0 {
                        relaxation_used = true;
                    }
                    break;
                }
                Some(_) => {
                    // Score still <= 0; advance to next relaxation step.
                    continue;
                }
                None => break,
            }
        }

        let Some((pick_idx, hits)) = chosen else {
            counters.repetition_skips += (target_size - queue.len()) as i64;
            break;
        };
        if relaxation_used {
            counters.penalty_relaxations += 1;
        }
        if hits.artist {
            counters.same_artist_penalties += 1;
        }
        if hits.album {
            counters.same_album_penalties += 1;
        }
        if hits.genre_saturation {
            counters.genre_saturation_penalties += 1;
        }
        let cand = available.swap_remove(pick_idx);
        *source_counts.entry(cand.source).or_default() += 1;
        queue.push(cand);
    }

    queue
}

// --- Hard same-artist diversity cap ------------------------------------------
//
// The soft same-artist penalty above lives behind the staged-rollout
// `radio_diversity_rerank_enabled` flag (default off), and even when enabled
// its relaxation ladder drops the artist dimension entirely when the pool is
// one-artist heavy. Real-world result: an unattended radio session played 12
// consecutive tracks by one artist. This pass is the guarantee the penalties
// cannot give: it runs after BOTH selection paths, remembers what recently
// PLAYED (not just the queue being built, so refills cannot restart a
// marathon), and hard-rejects picks that would form one. It only yields when
// the remaining pool offers no alternative at all - better repetition than
// silence.

/// Max consecutive same-artist slots, counting recently played history.
pub(super) const ARTIST_RUN_CAP: usize = 2;
/// Sliding window size for the occurrence cap.
pub(super) const ARTIST_WINDOW: usize = 10;
/// Max slots one artist may take within any ARTIST_WINDOW consecutive slots.
pub(super) const ARTIST_WINDOW_CAP: usize = 2;
/// Recently played artists seeding the cap history (window minus the slot
/// being decided).
pub(super) const ARTIST_HISTORY_MEMORY: usize = ARTIST_WINDOW - 1;
/// Only listens this recent count as "just played" - yesterday's tracks
/// should not constrain today's first radio queue.
pub(super) const ARTIST_HISTORY_MAX_AGE_HOURS: i64 = 2;

/// Case-folded artist identity for the cap. None for blank/unknown artists,
/// which never form runs (an unattributable candidate cannot be a marathon).
pub(super) fn artist_key(name: &str) -> Option<String> {
    let key = name.trim().to_ascii_lowercase();
    (!key.is_empty()).then_some(key)
}

pub(super) fn artist_allowed(candidate_artist: &str, history: &[String]) -> bool {
    let Some(key) = artist_key(candidate_artist) else {
        return true;
    };
    let run = history.iter().rev().take_while(|h| **h == key).count();
    if run >= ARTIST_RUN_CAP {
        return false;
    }
    let window_start = history.len().saturating_sub(ARTIST_WINDOW - 1);
    let in_window = history[window_start..]
        .iter()
        .filter(|h| **h == key)
        .count();
    in_window < ARTIST_WINDOW_CAP
}

/// Greedy stable pass over the ranked list: keep rank order, but skip any
/// candidate whose artist would exceed the run or window cap given what came
/// before it (recently played prefix + picks so far). Skipped candidates stay
/// eligible for later slots once the window moves past their artist.
pub(crate) fn enforce_artist_diversity(
    ranked: Vec<RadioCandidate>,
    recent_artists: &[String],
    limit: usize,
) -> Vec<RadioCandidate> {
    let mut history: Vec<String> = recent_artists
        .iter()
        .map(|name| artist_key(name).unwrap_or_default())
        .collect();
    let mut pending = ranked;
    let mut out: Vec<RadioCandidate> = Vec::with_capacity(limit.min(pending.len()));
    while out.len() < limit && !pending.is_empty() {
        let pick = pending
            .iter()
            .position(|cand| artist_allowed(&cand.artist_name, &history))
            // Degenerate pool: every remaining candidate violates the cap
            // (e.g. a one-artist library). Take the best-ranked anyway rather
            // than starve the radio.
            .unwrap_or(0);
        let cand = pending.remove(pick);
        history.push(artist_key(&cand.artist_name).unwrap_or_default());
        out.push(cand);
    }
    out
}
