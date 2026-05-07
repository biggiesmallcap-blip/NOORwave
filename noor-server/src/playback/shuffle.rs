use crate::db::models::Track;
use chrono::Utc;
use rand::Rng;
use rand::seq::SliceRandom;
use std::cmp::Ordering;
use std::collections::{HashMap, VecDeque};

const UNKNOWN_GENRE: &str = "unknown";
const UNKNOWN_ARTIST_KEY: &str = "__unknown_artist__";

#[derive(Debug, Clone)]
pub struct WeightedShuffleProfile {
    pub favorite_boost: f64,
    pub never_played_boost: f64,
    pub recent_play_penalty: f64,
    pub fidelity_weight: f64,
    pub source_weights: HashMap<String, f64>,
}

impl Default for WeightedShuffleProfile {
    fn default() -> Self {
        let mut source_weights = HashMap::new();
        source_weights.insert("local".to_string(), 1.1);
        source_weights.insert("tidal".to_string(), 1.0);
        source_weights.insert("ytmusic".to_string(), 0.95);
        source_weights.insert("soundcloud".to_string(), 0.9);

        Self {
            favorite_boost: 1.4,
            never_played_boost: 1.25,
            recent_play_penalty: 0.8,
            fidelity_weight: 0.003,
            source_weights,
        }
    }
}

impl WeightedShuffleProfile {
    pub fn weight_for(&self, track: &Track) -> f64 {
        let mut weight = 1.0;

        if track.is_favorite {
            weight *= self.favorite_boost.max(0.0);
        }

        if track.play_count == 0 {
            weight *= self.never_played_boost.max(0.0);
        } else if let Some(last_played) = track.last_played_at.as_deref() {
            // Time-decay: penalty fades from full (0.8×) at <1 day to none at 30+ days.
            // Tracks played months ago get no penalty — only truly recent plays are suppressed.
            let days_since = parse_days_since(last_played);
            let decay = (days_since / 30.0).min(1.0); // 0.0 = just played, 1.0 = 30+ days ago
            let penalty = self.recent_play_penalty + (1.0 - self.recent_play_penalty) * decay;
            weight *= penalty.max(0.0);
        }

        weight += (track.fidelity_score.max(0) as f64) * self.fidelity_weight.max(0.0);

        if let Some(source_weight) = self.source_weights.get(&track.source) {
            weight *= source_weight.max(0.0);
        }

        weight.max(0.000_001)
    }
}

/// Parse an ISO-8601 timestamp string and return how many days ago it was.
/// Returns 999.0 on parse failure so stale/unknown timestamps get no penalty.
fn parse_days_since(timestamp: &str) -> f64 {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
        return 999.0;
    };
    let elapsed = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
    elapsed.num_seconds().max(0) as f64 / 86_400.0
}

#[derive(Debug, Clone, Default)]
struct Bucket {
    key: String,
    tracks: VecDeque<Track>,
}

pub fn true_shuffle(tracks: &[Track]) -> Vec<Track> {
    let mut rng = rand::thread_rng();
    true_shuffle_with_rng(tracks, &mut rng)
}

pub fn true_shuffle_with_rng<R: Rng + ?Sized>(tracks: &[Track], rng: &mut R) -> Vec<Track> {
    let mut shuffled = tracks.to_vec();
    fisher_yates_shuffle(&mut shuffled, rng);
    shuffled
}

pub fn weighted_shuffle(tracks: &[Track], profile: &WeightedShuffleProfile) -> Vec<Track> {
    let mut rng = rand::thread_rng();
    weighted_shuffle_with_rng(tracks, profile, &mut rng)
}

pub fn weighted_shuffle_with_rng<R: Rng + ?Sized>(
    tracks: &[Track],
    profile: &WeightedShuffleProfile,
    rng: &mut R,
) -> Vec<Track> {
    let mut weighted = tracks
        .iter()
        .cloned()
        .map(|track| {
            let weight = profile.weight_for(&track);
            let uniform = rng.gen_range(f64::EPSILON..1.0);
            let key = -uniform.ln() / weight;
            (key, track)
        })
        .collect::<Vec<_>>();

    weighted.sort_by(|(left, _), (right, _)| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    weighted.into_iter().map(|(_, track)| track).collect()
}

pub fn artist_spread_shuffle(tracks: &[Track]) -> Vec<Track> {
    let mut rng = rand::thread_rng();
    artist_spread_shuffle_with_rng(tracks, &mut rng)
}

pub fn artist_spread_shuffle_with_rng<R: Rng + ?Sized>(
    tracks: &[Track],
    rng: &mut R,
) -> Vec<Track> {
    let mut buckets = bucket_tracks_by_artist(tracks);
    distribute_buckets(&mut buckets, rng)
}

pub fn genre_shuffle(tracks: &[Track], track_genres: &HashMap<i64, Vec<String>>) -> Vec<Track> {
    let mut rng = rand::thread_rng();
    genre_shuffle_with_rng(tracks, track_genres, &mut rng)
}

pub fn genre_shuffle_with_rng<R: Rng + ?Sized>(
    tracks: &[Track],
    track_genres: &HashMap<i64, Vec<String>>,
    rng: &mut R,
) -> Vec<Track> {
    let mut buckets = bucket_tracks_by_genre(tracks, track_genres);
    let genre_spread = distribute_buckets(&mut buckets, rng);
    let artist_spread = artist_spread_shuffle_with_rng(&genre_spread, rng);
    stabilize_adjacent_keys(artist_spread, |track| {
        track_genres
            .get(&track.id)
            .and_then(|genres| genres.first())
            .map(|genre| normalize_bucket_key(genre))
            .unwrap_or_else(|| UNKNOWN_GENRE.to_string())
    })
}

fn fisher_yates_shuffle<R: Rng + ?Sized>(tracks: &mut [Track], rng: &mut R) {
    for index in (1..tracks.len()).rev() {
        let swap_index = rng.gen_range(0..=index);
        tracks.swap(index, swap_index);
    }
}

fn distribute_buckets<R: Rng + ?Sized>(buckets: &mut [Bucket], rng: &mut R) -> Vec<Track> {
    for bucket in buckets.iter_mut() {
        let slice = bucket.tracks.make_contiguous();
        slice.shuffle(rng);
    }

    buckets.shuffle(rng);

    let total = buckets
        .iter()
        .map(|bucket| bucket.tracks.len())
        .sum::<usize>();
    let mut sequence = Vec::with_capacity(total);
    let mut last_key: Option<String> = None;

    while sequence.len() < total {
        let Some(index) = pick_bucket_index(buckets, last_key.as_deref(), rng) else {
            break;
        };

        if let Some(track) = buckets[index].tracks.pop_front() {
            last_key = Some(buckets[index].key.clone());
            sequence.push(track);
        }
    }

    sequence
}

/// Pick the next bucket to drain. Eligible buckets (non-empty, key != last_key)
/// are sampled in proportion to their remaining track count — large buckets get
/// picked more often without being deterministic, so the queue isn't always led
/// by the dominant key. Falls back to any non-empty bucket when no other key
/// has tracks left.
fn pick_bucket_index<R: Rng + ?Sized>(
    buckets: &[Bucket],
    last_key: Option<&str>,
    rng: &mut R,
) -> Option<usize> {
    let mut total_weight: usize = 0;
    let mut eligible: Vec<(usize, usize)> = Vec::new();
    let mut fallback: Option<usize> = None;

    for (index, bucket) in buckets.iter().enumerate() {
        if bucket.tracks.is_empty() {
            continue;
        }
        if fallback.is_none() {
            fallback = Some(index);
        }
        if last_key != Some(bucket.key.as_str()) {
            total_weight += bucket.tracks.len();
            eligible.push((index, bucket.tracks.len()));
        }
    }

    if total_weight == 0 {
        return fallback;
    }

    let mut threshold = rng.gen_range(0..total_weight);
    for (index, weight) in &eligible {
        if threshold < *weight {
            return Some(*index);
        }
        threshold -= *weight;
    }
    // Defensive: numerical fall-through (shouldn't be reachable when total_weight > 0).
    eligible.first().map(|(i, _)| *i).or(fallback)
}

fn bucket_tracks_by_artist(tracks: &[Track]) -> Vec<Bucket> {
    let mut groups: HashMap<String, Vec<Track>> = HashMap::new();

    for track in tracks.iter().cloned() {
        let artist_key = if track.artist_id != 0 {
            format!("artist:{}", track.artist_id)
        } else {
            track
                .artist_name
                .clone()
                .filter(|name| !name.trim().is_empty())
                .map(|name| format!("artist-name:{name}"))
                .unwrap_or_else(|| UNKNOWN_ARTIST_KEY.to_string())
        };

        groups.entry(artist_key).or_default().push(track);
    }

    groups
        .into_iter()
        .map(|(key, tracks)| Bucket {
            key,
            tracks: VecDeque::from(tracks),
        })
        .collect()
}

fn bucket_tracks_by_genre(
    tracks: &[Track],
    track_genres: &HashMap<i64, Vec<String>>,
) -> Vec<Bucket> {
    let mut groups: HashMap<String, Vec<Track>> = HashMap::new();

    for track in tracks.iter().cloned() {
        let genre_key = track_genres
            .get(&track.id)
            .and_then(|genres| genres.first())
            .map(|genre| normalize_bucket_key(genre))
            .unwrap_or_else(|| UNKNOWN_GENRE.to_string());

        groups.entry(genre_key).or_default().push(track);
    }

    groups
        .into_iter()
        .map(|(key, tracks)| Bucket {
            key,
            tracks: VecDeque::from(tracks),
        })
        .collect()
}

fn normalize_bucket_key(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        UNKNOWN_GENRE.to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// Forward-pass swap to break same-key adjacency, iterated to a fixed point
/// (capped at 4 passes). A single pass can leave new same-key pairs behind
/// when a swap target sits next to its own same-key neighbour; iterating lets
/// the next pass mop those up. Same-key tail runs with no different-key track
/// anywhere ahead are unfixable by forward swap and are left in place.
fn stabilize_adjacent_keys<F>(mut tracks: Vec<Track>, key_fn: F) -> Vec<Track>
where
    F: Fn(&Track) -> String,
{
    if tracks.len() < 2 {
        return tracks;
    }

    for _ in 0..4 {
        let mut changed = false;
        for idx in 1..tracks.len() {
            let previous_key = key_fn(&tracks[idx - 1]);
            let current_key = key_fn(&tracks[idx]);
            if previous_key != current_key {
                continue;
            }

            if let Some(swap_idx) = ((idx + 1)..tracks.len()).find(|candidate_idx| {
                let candidate_key = key_fn(&tracks[*candidate_idx]);
                candidate_key != previous_key
            }) {
                tracks.swap(idx, swap_idx);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    tracks
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use std::collections::HashSet;

    fn track(
        id: i64,
        artist_id: i64,
        artist_name: &str,
        favorite: bool,
        play_count: i32,
        last_played_at: Option<&str>,
        fidelity_score: i32,
    ) -> Track {
        Track {
            id,
            title: format!("Track {id}"),
            artist_id,
            artist_name: Some(artist_name.to_string()),
            album_id: None,
            album_title: None,
            disc_number: Some(1),
            track_number: Some(id as i32),
            duration_ms: Some(180_000),
            isrc: None,
            tidal_id: Some(id),
            ytmusic_id: None,
            soundcloud_id: None,
            best_quality: Some("LOSSLESS".to_string()),
            best_source: Some("tidal".to_string()),
            fidelity_score,
            is_favorite: favorite,
            play_count,
            last_played_at: last_played_at.map(str::to_string),
            date_added: Some("2026-01-01T00:00:00Z".to_string()),
            source: "tidal".to_string(),
            artwork_url: None,
        }
    }

    #[test]
    fn fisher_yates_keeps_all_tracks() {
        let tracks = vec![
            track(1, 1, "A", false, 1, None, 10),
            track(2, 2, "B", false, 1, None, 10),
            track(3, 3, "C", false, 1, None, 10),
            track(4, 4, "D", false, 1, None, 10),
        ];
        let mut rng = StdRng::seed_from_u64(7);
        let shuffled = true_shuffle_with_rng(&tracks, &mut rng);

        assert_eq!(shuffled.len(), tracks.len());

        let original = tracks.iter().map(|track| track.id).collect::<HashSet<_>>();
        let result = shuffled
            .iter()
            .map(|track| track.id)
            .collect::<HashSet<_>>();
        assert_eq!(result, original);
        assert_ne!(
            shuffled.iter().map(|track| track.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn weighted_shuffle_prefers_higher_weight_tracks() {
        let low = track(1, 1, "A", false, 10, Some("2026-02-01T00:00:00Z"), 20);
        let high = track(2, 2, "B", true, 0, None, 100);
        let profile = WeightedShuffleProfile::default();
        let mut rng = StdRng::seed_from_u64(42);

        let shuffled = weighted_shuffle_with_rng(&[low.clone(), high.clone()], &profile, &mut rng);

        assert_eq!(profile.weight_for(&high) > profile.weight_for(&low), true);
        assert_eq!(shuffled.first().map(|track| track.id), Some(high.id));
    }

    #[test]
    fn artist_spread_avoids_consecutive_repeats_when_possible() {
        let tracks = vec![
            track(1, 1, "A", false, 1, None, 10),
            track(2, 1, "A", false, 1, None, 10),
            track(3, 2, "B", false, 1, None, 10),
            track(4, 3, "C", false, 1, None, 10),
        ];
        let mut rng = StdRng::seed_from_u64(9);
        let shuffled = artist_spread_shuffle_with_rng(&tracks, &mut rng);

        for pair in shuffled.windows(2) {
            assert_ne!(pair[0].artist_id, pair[1].artist_id);
        }
    }

    #[test]
    fn pick_bucket_does_not_always_lead_with_dominant_key() {
        // Library skewed heavily to one genre. The greedy "pick largest" rule
        // would always lead with that genre's track; weighted random picking
        // should sometimes start elsewhere even when one bucket is largest.
        let tracks: Vec<Track> = (1..=10)
            .map(|id| track(id, id, "art", false, 1, None, 10))
            .collect();
        let mut genres = HashMap::new();
        for id in 1..=8 {
            genres.insert(id, vec!["House".to_string()]);
        }
        for id in 9..=10 {
            genres.insert(id, vec!["Ambient".to_string()]);
        }

        let mut leading_genre_counts = std::collections::HashMap::new();
        for seed in 0u64..200 {
            let mut rng = StdRng::seed_from_u64(seed);
            let shuffled = genre_shuffle_with_rng(&tracks, &genres, &mut rng);
            let leader = genres
                .get(&shuffled[0].id)
                .and_then(|g| g.first())
                .cloned()
                .unwrap_or_default();
            *leading_genre_counts.entry(leader).or_insert(0u32) += 1;
        }
        let ambient_leads = leading_genre_counts.get("Ambient").copied().unwrap_or(0);
        assert!(
            ambient_leads >= 5,
            "Ambient should occasionally lead even when smaller — got {ambient_leads}/200"
        );
    }

    #[test]
    fn stabilize_iterates_to_fixed_point() {
        // Construct an arrangement where a single forward pass leaves new
        // same-key pairs behind. tracks: keys A B A A A B B, with track ids
        // chosen so key_fn maps cleanly.
        let tracks = vec![
            track(1, 1, "A", false, 1, None, 10), // A
            track(2, 2, "B", false, 1, None, 10), // B
            track(3, 1, "A", false, 1, None, 10), // A
            track(4, 1, "A", false, 1, None, 10), // A
            track(5, 1, "A", false, 1, None, 10), // A
            track(6, 2, "B", false, 1, None, 10), // B
            track(7, 2, "B", false, 1, None, 10), // B
        ];
        let key_fn = |t: &Track| t.artist_name.clone().unwrap_or_default();
        let stabilized = stabilize_adjacent_keys(tracks, key_fn.clone());

        let keys: Vec<String> = stabilized.iter().map(key_fn).collect();
        // A valid arrangement of 4×A + 3×B has at most one same-A adjacency
        // (e.g. A B A B A B A). Our fixed-point iteration should reach that
        // shape; before the fix, the single pass left two A-A adjacencies.
        let same_pairs = keys
            .windows(2)
            .filter(|w| w[0] == w[1])
            .count();
        assert!(
            same_pairs <= 1,
            "expected at most one same-key adjacency in {:?}, got {same_pairs}",
            keys
        );
    }

    #[test]
    fn genre_shuffle_spreads_primary_genres_before_artist_pass() {
        let tracks = vec![
            track(1, 1, "A", false, 1, None, 10),
            track(2, 2, "B", false, 1, None, 10),
            track(3, 3, "C", false, 1, None, 10),
            track(4, 4, "D", false, 1, None, 10),
        ];

        let genres = HashMap::from([
            (1, vec!["House".to_string()]),
            (2, vec!["House".to_string()]),
            (3, vec!["Ambient".to_string()]),
            (4, vec!["Ambient".to_string()]),
        ]);

        let mut rng = StdRng::seed_from_u64(3);
        let shuffled = genre_shuffle_with_rng(&tracks, &genres, &mut rng);

        let genre_order = shuffled
            .iter()
            .map(|track| {
                genres
                    .get(&track.id)
                    .and_then(|values| values.first())
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();

        assert_eq!(genre_order.len(), 4);
        assert_ne!(genre_order[0], genre_order[1]);
    }
}
