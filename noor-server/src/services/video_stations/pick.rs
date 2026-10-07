//! Pure ordering for station refills: no database access. Pipeline: drop
//! excluded and twice-skipped videos, keep one cut per song, order by the
//! station's rule (once-skipped videos go last), then space artists out.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};

use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

use crate::services::video_radio::video_song_key;
use crate::services::video_sets::VideoCandidate;

pub const NEAR: f64 = 0.3;
pub const MID: f64 = 0.08;
const RING_WEIGHTS: [f64; 3] = [0.6, 0.3, 0.1];
const RELEVANCE_FLOOR: f64 = 0.05;
const ARTIST_CAP_PER_BATCH: usize = 2;
const HOP_WINDOW: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct Candidate {
    pub video: VideoCandidate,
    pub watched: bool,
    pub skips: u32,
    /// Crawler relevance of the video's artist to the listener, 0 when unknown.
    pub relevance: f64,
    pub liked_artist: bool,
    /// Listener plays of the video's artist (Deep cuts order).
    pub artist_plays: i64,
    /// Best chart position (Charts order).
    pub chart_rank: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Each slot draws near/mid/far 60/30/10, then leans within the ring.
    Rings,
    Uniform,
    /// Most-played liked artists first, round-robin, most popular video first.
    DeepCuts,
    Popularity,
    /// Weighted shuffle by relevance x popularity.
    Lean,
    /// Lean, alternating liked and unknown artists.
    LeanMixed,
    /// Prefer a video sharing an artist with the previous one.
    ArtistHop,
    Chart,
}

#[derive(Debug, Clone)]
pub struct PickInput<'a> {
    pub order: Order,
    pub unwatched_only: bool,
    pub prefer_live_cuts: bool,
    pub spacing: bool,
    pub excluded: &'a HashSet<i64>,
    pub seed: u64,
    pub limit: usize,
}

fn song_key(video: &VideoCandidate) -> String {
    video_song_key(video.artist_id, video.artist_name.as_deref(), &video.title)
}

/// Distinct songs a station can still play without repeating anything.
pub fn unwatched_songs(candidates: &[Candidate]) -> usize {
    candidates
        .iter()
        .filter(|c| !c.watched && c.skips < 2)
        .map(|c| song_key(&c.video))
        .collect::<HashSet<_>>()
        .len()
}

pub fn pick(candidates: Vec<Candidate>, input: &PickInput) -> Vec<VideoCandidate> {
    let usable: Vec<Candidate> = candidates
        .into_iter()
        .filter(|c| c.skips < 2 && !input.excluded.contains(&c.video.tidal_id))
        .collect();
    let (unwatched, watched): (Vec<_>, Vec<_>) = usable.into_iter().partition(|c| !c.watched);
    let mut out = choose(one_cut_per_song(unwatched, input.prefer_live_cuts), input);
    if out.len() < input.limit && !input.unwatched_only {
        let taken: HashSet<String> = out.iter().map(song_key).collect();
        let rest: Vec<Candidate> = one_cut_per_song(watched, input.prefer_live_cuts)
            .into_iter()
            .filter(|c| !taken.contains(&song_key(&c.video)))
            .collect();
        let fill = PickInput {
            limit: input.limit - out.len(),
            ..input.clone()
        };
        out.extend(choose(rest, &fill));
    }
    out
}

fn cut_rank(c: &Candidate, prefer_live: bool) -> (bool, i32, Reverse<i64>) {
    let wanted = if prefer_live { "Live" } else { "Music Video" };
    (
        c.video.video_type.as_deref() == Some(wanted),
        c.video.popularity.unwrap_or(-1),
        Reverse(c.video.tidal_id),
    )
}

fn one_cut_per_song(candidates: Vec<Candidate>, prefer_live: bool) -> Vec<Candidate> {
    let mut best: HashMap<String, Candidate> = HashMap::new();
    let mut keys = Vec::new();
    for candidate in candidates {
        let key = song_key(&candidate.video);
        match best.get(&key) {
            Some(existing)
                if cut_rank(existing, prefer_live) >= cut_rank(&candidate, prefer_live) => {}
            Some(_) => {
                best.insert(key, candidate);
            }
            None => {
                keys.push(key.clone());
                best.insert(key, candidate);
            }
        }
    }
    keys.into_iter()
        .filter_map(|key| best.remove(&key))
        .collect()
}

fn choose(candidates: Vec<Candidate>, input: &PickInput) -> Vec<VideoCandidate> {
    if candidates.is_empty() || input.limit == 0 {
        return Vec::new();
    }
    let (fresh, demoted): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|c| c.skips == 0);
    let mut rng = StdRng::seed_from_u64(input.seed);
    let mut ordered = order(fresh, input.order, &mut rng);
    ordered.extend(order(demoted, input.order, &mut rng));
    let videos: Vec<VideoCandidate> = ordered.into_iter().map(|c| c.video).collect();
    if input.spacing {
        space(videos, input.limit)
    } else {
        videos.into_iter().take(input.limit).collect()
    }
}

fn by_popularity(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    b.video
        .popularity
        .unwrap_or(-1)
        .cmp(&a.video.popularity.unwrap_or(-1))
        .then(a.video.tidal_id.cmp(&b.video.tidal_id))
}

fn order(candidates: Vec<Candidate>, order: Order, rng: &mut StdRng) -> Vec<Candidate> {
    match order {
        Order::Uniform => {
            let mut shuffled = candidates;
            shuffled.shuffle(rng);
            shuffled
        }
        Order::Lean => lean(candidates, rng),
        Order::LeanMixed => alternate_liked(candidates, rng),
        Order::Rings => rings(candidates, rng),
        Order::Popularity => {
            let mut sorted = candidates;
            sorted.sort_by(by_popularity);
            sorted
        }
        Order::Chart => {
            let mut sorted = candidates;
            sorted.sort_by(|a, b| {
                a.chart_rank
                    .unwrap_or(i64::MAX)
                    .cmp(&b.chart_rank.unwrap_or(i64::MAX))
                    .then(a.video.tidal_id.cmp(&b.video.tidal_id))
            });
            sorted
        }
        Order::DeepCuts => deep_cuts(candidates),
        Order::ArtistHop => {
            artist_hop(lean(candidates, rng).into_iter().take(HOP_WINDOW).collect())
        }
    }
}

fn weight(c: &Candidate) -> f64 {
    let popularity = c
        .video
        .popularity
        .map_or(1.0, |p| 0.5 + f64::from(p.clamp(0, 100)) / 200.0);
    c.relevance.max(RELEVANCE_FLOOR) * popularity
}

/// Weighted shuffle without replacement (Efraimidis-Spirakis keys).
fn lean(candidates: Vec<Candidate>, rng: &mut StdRng) -> Vec<Candidate> {
    let mut keyed: Vec<(f64, Candidate)> = candidates
        .into_iter()
        .map(|c| {
            let u: f64 = rng.random_range(f64::EPSILON..1.0);
            (u.powf(1.0 / weight(&c)), c)
        })
        .collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    keyed.into_iter().map(|(_, c)| c).collect()
}

fn alternate_liked(candidates: Vec<Candidate>, rng: &mut StdRng) -> Vec<Candidate> {
    let (liked, other): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|c| c.liked_artist);
    let mut liked = VecDeque::from(lean(liked, rng));
    let mut other = VecDeque::from(lean(other, rng));
    let mut out = Vec::with_capacity(liked.len() + other.len());
    while !liked.is_empty() || !other.is_empty() {
        out.extend(other.pop_front());
        out.extend(liked.pop_front());
    }
    out
}

fn ring_of(relevance: f64) -> usize {
    if relevance >= NEAR {
        0
    } else if relevance >= MID {
        1
    } else {
        2
    }
}

fn rings(candidates: Vec<Candidate>, rng: &mut StdRng) -> Vec<Candidate> {
    let mut buckets: [Vec<Candidate>; 3] = Default::default();
    for c in candidates {
        buckets[ring_of(c.relevance)].push(c);
    }
    let mut queues: Vec<VecDeque<Candidate>> = buckets
        .into_iter()
        .map(|bucket| VecDeque::from(lean(bucket, rng)))
        .collect();
    let mut out = Vec::new();
    while queues.iter().any(|q| !q.is_empty()) {
        let roll: f64 = rng.random();
        let mut ring = if roll < RING_WEIGHTS[0] {
            0
        } else if roll < RING_WEIGHTS[0] + RING_WEIGHTS[1] {
            1
        } else {
            2
        };
        while queues[ring].is_empty() {
            ring = (ring + 1) % 3;
        }
        out.extend(queues[ring].pop_front());
    }
    out
}

fn deep_cuts(candidates: Vec<Candidate>) -> Vec<Candidate> {
    let mut by_artist: HashMap<i64, Vec<Candidate>> = HashMap::new();
    for c in candidates {
        let artist = c.video.artist_id.unwrap_or(-c.video.tidal_id);
        by_artist.entry(artist).or_default().push(c);
    }
    let mut artists: Vec<(i64, i64, VecDeque<Candidate>)> = by_artist
        .into_iter()
        .map(|(artist, mut videos)| {
            videos.sort_by(by_popularity);
            let plays = videos.first().map_or(0, |c| c.artist_plays);
            (artist, plays, VecDeque::from(videos))
        })
        .collect();
    artists.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out = Vec::new();
    while artists.iter().any(|(_, _, videos)| !videos.is_empty()) {
        for (_, _, videos) in artists.iter_mut() {
            out.extend(videos.pop_front());
        }
    }
    out
}

fn artists_of(video: &VideoCandidate) -> HashSet<i64> {
    video
        .artist_id
        .into_iter()
        .chain(video.featured_artist_ids.iter().copied())
        .collect()
}

fn artist_hop(ordered: Vec<Candidate>) -> Vec<Candidate> {
    let mut rest: VecDeque<Candidate> = ordered.into();
    let mut out: Vec<Candidate> = Vec::new();
    loop {
        let hop = out.last().and_then(|prev| {
            let linked = artists_of(&prev.video);
            rest.iter().position(|c| {
                c.video.artist_id != prev.video.artist_id
                    && !artists_of(&c.video).is_disjoint(&linked)
            })
        });
        let next = match hop {
            Some(index) => rest.remove(index),
            None => rest.pop_front(),
        };
        let Some(next) = next else {
            break;
        };
        out.push(next);
    }
    out
}

/// Never the same artist twice in a row and at most two per batch. Relaxes
/// the cap, then adjacency, before leaving a slot empty.
fn space(ordered: Vec<VideoCandidate>, limit: usize) -> Vec<VideoCandidate> {
    let artist = |v: &VideoCandidate| v.artist_id.unwrap_or(-v.tidal_id);
    let mut pool: VecDeque<VideoCandidate> = ordered.into();
    let mut out: Vec<VideoCandidate> = Vec::new();
    let mut counts: HashMap<i64, usize> = HashMap::new();
    while out.len() < limit && !pool.is_empty() {
        let last = out.last().map(artist);
        let index = pool
            .iter()
            .position(|v| {
                Some(artist(v)) != last
                    && counts.get(&artist(v)).copied().unwrap_or(0) < ARTIST_CAP_PER_BATCH
            })
            .or_else(|| pool.iter().position(|v| Some(artist(v)) != last))
            .unwrap_or(0);
        let Some(video) = pool.remove(index) else {
            break;
        };
        *counts.entry(artist(&video)).or_default() += 1;
        out.push(video);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: i64, artist: i64) -> Candidate {
        Candidate {
            video: VideoCandidate {
                tidal_id: id,
                title: format!("Song {id}"),
                artist_id: Some(artist),
                artist_name: Some(format!("Artist {artist}")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn input(order: Order, limit: usize, excluded: &HashSet<i64>) -> PickInput<'_> {
        PickInput {
            order,
            unwatched_only: false,
            prefer_live_cuts: false,
            spacing: false,
            excluded,
            seed: 7,
            limit,
        }
    }

    fn ids(videos: &[VideoCandidate]) -> Vec<i64> {
        videos.iter().map(|v| v.tidal_id).collect()
    }

    #[test]
    fn wild_card_rings_follow_sixty_thirty_ten() {
        let mut all = Vec::new();
        for i in 0..300 {
            let mut c = cand(i, 1000 + i);
            c.relevance = match i / 100 {
                0 => 0.5,
                1 => 0.1,
                _ => 0.0,
            };
            all.push(c);
        }
        let none = HashSet::new();
        let picked = pick(all, &input(Order::Rings, 100, &none));
        let near = picked.iter().filter(|v| v.tidal_id < 100).count();
        let mid = picked
            .iter()
            .filter(|v| (100..200).contains(&v.tidal_id))
            .count();
        let far = picked.iter().filter(|v| v.tidal_id >= 200).count();
        assert_eq!(near + mid + far, 100);
        assert!((45..=75).contains(&near), "near {near}");
        assert!((18..=42).contains(&mid), "mid {mid}");
        assert!((3..=20).contains(&far), "far {far}");
    }

    #[test]
    fn one_cut_per_song_prefers_the_official_video_or_the_live_take() {
        let mut live = cand(1, 10);
        live.video.title = "Song (Live)".into();
        live.video.video_type = Some("Live".into());
        let mut official = cand(2, 10);
        official.video.title = "Song (Official Video)".into();
        official.video.video_type = Some("Music Video".into());
        official.video.popularity = Some(10);
        let mut popular = cand(3, 10);
        popular.video.title = "Song".into();
        popular.video.popularity = Some(90);
        let all = vec![live, official, popular];
        let none = HashSet::new();
        assert_eq!(
            ids(&pick(all.clone(), &input(Order::Popularity, 12, &none))),
            vec![2]
        );
        let mut live_input = input(Order::Popularity, 12, &none);
        live_input.prefer_live_cuts = true;
        assert_eq!(ids(&pick(all, &live_input)), vec![1]);
    }

    #[test]
    fn spacing_avoids_back_to_back_and_caps_artists() {
        let mut all: Vec<Candidate> = (0..6).map(|i| cand(i, 1)).collect();
        all.extend((6..9).map(|i| cand(i, 2)));
        all.extend((9..12).map(|i| cand(i, 3)));
        all.extend((12..15).map(|i| cand(i, 4)));
        let none = HashSet::new();
        let mut spaced = input(Order::Popularity, 6, &none);
        spaced.spacing = true;
        let picked = pick(all, &spaced);
        for pair in picked.windows(2) {
            assert_ne!(pair[0].artist_id, pair[1].artist_id);
        }
        let by_one = picked.iter().filter(|v| v.artist_id == Some(1)).count();
        assert!(by_one <= 2, "artist 1 played {by_one} times");
        assert_eq!(picked.len(), 6);
    }

    #[test]
    fn skips_demote_once_and_ban_twice() {
        let mut once = cand(1, 1);
        once.skips = 1;
        once.video.popularity = Some(99);
        let mut twice = cand(2, 2);
        twice.skips = 2;
        let mut normal = cand(3, 3);
        normal.video.popularity = Some(1);
        let none = HashSet::new();
        let picked = pick(
            vec![once, twice, normal],
            &input(Order::Popularity, 12, &none),
        );
        assert_eq!(ids(&picked), vec![3, 1]);
    }

    #[test]
    fn unwatched_first_and_watched_only_as_fallback() {
        let mut seen = cand(1, 1);
        seen.watched = true;
        let fresh = cand(2, 2);
        let none = HashSet::new();
        let mut strict = input(Order::Popularity, 12, &none);
        strict.unwatched_only = true;
        assert_eq!(
            ids(&pick(vec![seen.clone(), fresh.clone()], &strict)),
            vec![2]
        );
        assert_eq!(
            ids(&pick(
                vec![seen, fresh],
                &input(Order::Popularity, 12, &none)
            )),
            vec![2, 1]
        );
    }

    #[test]
    fn excluded_videos_never_come_back() {
        let excluded = HashSet::from([1, 2]);
        let picked = pick(
            vec![cand(1, 1), cand(2, 2), cand(3, 3)],
            &input(Order::Uniform, 12, &excluded),
        );
        assert_eq!(ids(&picked), vec![3]);
    }

    #[test]
    fn random_orders_are_seeded() {
        let all: Vec<Candidate> = (0..50).map(|i| cand(i, 100 + i)).collect();
        let none = HashSet::new();
        let a = pick(all.clone(), &input(Order::Lean, 20, &none));
        let b = pick(all.clone(), &input(Order::Lean, 20, &none));
        let mut other_seed = input(Order::Lean, 20, &none);
        other_seed.seed = 8;
        let c = pick(all, &other_seed);
        assert_eq!(ids(&a), ids(&b));
        assert_ne!(ids(&a), ids(&c));
    }

    #[test]
    fn deep_cuts_round_robin_from_most_played_artists() {
        let mut all = Vec::new();
        for (id, artist, plays, popularity) in [
            (1, 10, 5, 50),
            (2, 10, 5, 90),
            (3, 20, 9, 10),
            (4, 20, 9, 20),
        ] {
            let mut c = cand(id, artist);
            c.artist_plays = plays;
            c.video.popularity = Some(popularity);
            all.push(c);
        }
        let none = HashSet::new();
        assert_eq!(
            ids(&pick(all, &input(Order::DeepCuts, 12, &none))),
            vec![4, 2, 3, 1]
        );
    }

    #[test]
    fn charts_play_in_chart_order() {
        let mut all = Vec::new();
        for (id, rank) in [(1, 3), (2, 1), (3, 2)] {
            let mut c = cand(id, id);
            c.chart_rank = Some(rank);
            all.push(c);
        }
        let none = HashSet::new();
        assert_eq!(
            ids(&pick(all, &input(Order::Chart, 12, &none))),
            vec![2, 3, 1]
        );
    }

    #[test]
    fn duets_hop_through_featured_artists() {
        let mut duet = cand(1, 10);
        duet.video.featured_artist_ids = vec![20];
        duet.relevance = 1.0;
        duet.video.popularity = Some(100);
        let guest = cand(2, 20);
        let stranger = cand(3, 30);
        let none = HashSet::new();
        let picked = pick(
            vec![stranger, guest, duet],
            &input(Order::ArtistHop, 12, &none),
        );
        let first = picked[0].tidal_id;
        if first == 1 {
            assert_eq!(picked[1].tidal_id, 2, "the guest follows the duet");
        }
        assert_eq!(picked.len(), 3);
    }

    #[test]
    fn unwatched_songs_counts_distinct_playable_songs() {
        let mut seen = cand(1, 1);
        seen.watched = true;
        let mut banned = cand(2, 2);
        banned.skips = 2;
        let mut alternate = cand(3, 3);
        alternate.video.title = "Song 4 (Live)".into();
        let all = vec![seen, banned, alternate, cand(4, 3), cand(5, 5)];
        assert_eq!(unwatched_songs(&all), 2);
    }
}
