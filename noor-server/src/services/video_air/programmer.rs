//! The Programmer: pure scheduling over channel pools, no database access.
//!
//! A pool is a list of artist buckets, each holding units: a song (one
//! video, its best cut for the channel) or a concert (its setlist). A pick
//! samples a ring by the explore weights, an artist in it by bucket weight
//! (sqrt of playable songs x scene fit x history x session), then a unit by
//! unit weight (quality x rotation x cut preference x freshness x skip
//! damping x hold rate). Format rules: no artist twice in 12 slots, no song
//! twice in a 6-hour block, one frontier pick in four led by scene appetite,
//! and a concert always plays whole. Rules relax (spacing first, then song
//! repeats) before a channel would go dark.

use std::collections::{HashSet, VecDeque};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::clock::{self, BLOCK_S, BLOCKS};

/// No artist twice in this many consecutive slots.
pub const SPACING: usize = 12;
/// Every fourth pick comes from the frontier when the pool has one.
pub const FRONTIER_EVERY: usize = 4;
const REJECTION_TRIES: usize = 24;
/// Frontier artists without a scene compete with this appetite.
const SCENELESS_APPETITE: f64 = 0.5;

#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// (video id, seconds) of the first video. A song is just this; pools
    /// hold well over 100k songs, so it stays off the heap.
    pub lead: (i64, i64),
    /// The rest of a concert's setlist, in play order.
    pub rest: Vec<(i64, i64)>,
    /// Song key, or concert key: repeats are judged on this.
    pub key: u64,
    pub weight: f64,
    /// Empty for songs: their chip is worked out only once scheduled.
    pub why: String,
}

impl Unit {
    pub fn song(video: i64, seconds: i64, key: u64, weight: f64) -> Self {
        Self {
            lead: (video, seconds),
            rest: Vec::new(),
            key,
            weight,
            why: String::new(),
        }
    }

    /// A setlist; `None` when it is empty.
    pub fn set(videos: &[(i64, i64)], key: u64, weight: f64, why: String) -> Option<Self> {
        let (lead, rest) = videos.split_first()?;
        Some(Self {
            lead: *lead,
            rest: rest.to_vec(),
            key,
            weight,
            why,
        })
    }

    /// (video id, seconds) in play order.
    pub fn videos(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        std::iter::once(self.lead).chain(self.rest.iter().copied())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub artist: i64,
    /// 0 near, 1 mid, 2 far.
    pub ring: usize,
    pub weight: f64,
    pub frontier: bool,
    pub scene: Option<i64>,
    pub units: Vec<Unit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Artist-first rotation under the format rules.
    Rotation,
    /// One artist's songs in a reshuffled loop (Spotlight).
    Cycle,
}

#[derive(Debug, Clone)]
pub struct Pool {
    /// Sorted by artist id; sampling never depends on hash order.
    pub buckets: Vec<Bucket>,
    pub rings: [f64; 3],
    pub frontier: bool,
    /// 0 turns artist spacing off.
    pub spacing: usize,
    /// Scene appetite for frontier picks, sorted by scene id.
    pub appetite: Vec<(i64, f64)>,
    pub flow: Flow,
    /// Units stay unique for the whole day rather than the block (concerts).
    pub unique_per_day: bool,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            buckets: Vec::new(),
            rings: [0.6, 0.3, 0.1],
            frontier: false,
            spacing: SPACING,
            appetite: Vec::new(),
            flow: Flow::Rotation,
            unique_per_day: false,
        }
    }
}

impl Pool {
    pub fn is_empty(&self) -> bool {
        self.buckets.iter().all(|bucket| bucket.units.is_empty())
    }
}

/// One scheduled video. `start_s` counts from local midnight of its day.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub slot: i64,
    pub start_s: i64,
    pub duration_s: i64,
    pub video_id: i64,
    pub why: String,
}

impl Program {
    pub fn end_s(&self) -> i64 {
        self.start_s + self.duration_s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strictness {
    /// Full spacing window.
    Spaced,
    /// Spacing relaxed to "not the same artist back to back".
    Adjacent,
    /// Anything with an unused unit.
    Any,
}

fn cumulative(items: impl Iterator<Item = (usize, f64)>) -> Vec<(f64, usize)> {
    let mut total = 0.0;
    items
        .filter(|(_, weight)| weight.is_finite() && *weight > 0.0)
        .map(|(index, weight)| {
            total += weight;
            (total, index)
        })
        .collect()
}

fn sample_cumulative(cum: &[(f64, usize)], rng: &mut StdRng) -> Option<usize> {
    let total = cum.last()?.0;
    let x = rng.random::<f64>() * total;
    let index = cum.partition_point(|(c, _)| *c <= x).min(cum.len() - 1);
    Some(cum[index].1)
}

/// Weighted choice; the first option when every weight is zero.
fn weighted_choice(options: &[(usize, f64)], rng: &mut StdRng) -> Option<usize> {
    let total: f64 = options
        .iter()
        .map(|(_, w)| if w.is_finite() { w.max(0.0) } else { 0.0 })
        .sum();
    if options.is_empty() {
        return None;
    }
    if total <= 0.0 {
        return Some(options[0].0);
    }
    let mut x = rng.random::<f64>() * total;
    for (index, weight) in options {
        let weight = if weight.is_finite() {
            weight.max(0.0)
        } else {
            0.0
        };
        if x < weight {
            return Some(*index);
        }
        x -= weight;
    }
    options.last().map(|(index, _)| *index)
}

struct FrontierScene {
    appetite: f64,
    cum: Vec<(f64, usize)>,
}

struct Samplers {
    rings: [Vec<(f64, usize)>; 3],
    frontier: Vec<FrontierScene>,
}

#[derive(Default)]
struct State {
    recent: VecDeque<i64>,
    used: HashSet<u64>,
    exhausted: HashSet<usize>,
    picks: usize,
    cycle: VecDeque<(usize, usize)>,
    last_key: Option<u64>,
}

impl State {
    fn eligible(&mut self, pool: &Pool, bucket: usize, strictness: Strictness) -> bool {
        if self.exhausted.contains(&bucket) {
            return false;
        }
        let entry = &pool.buckets[bucket];
        if pool.spacing > 0 {
            let blocked = match strictness {
                Strictness::Spaced => self.recent.contains(&entry.artist),
                Strictness::Adjacent => self.recent.back() == Some(&entry.artist),
                Strictness::Any => false,
            };
            if blocked {
                return false;
            }
        }
        if entry.units.iter().all(|unit| self.used.contains(&unit.key)) {
            self.exhausted.insert(bucket);
            return false;
        }
        true
    }
}

/// Stateful picker over one pool: keeps the spacing window, the block's used
/// songs and the frontier cadence between picks.
pub struct Picker<'a> {
    pool: &'a Pool,
    samplers: Samplers,
    state: State,
    /// A session refill never repeats what the session excluded and ends
    /// instead of starting over.
    session: bool,
}

impl<'a> Picker<'a> {
    pub fn new(pool: &'a Pool) -> Self {
        let rings = [0, 1, 2].map(|ring| {
            cumulative(
                pool.buckets
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.ring == ring && !b.units.is_empty())
                    .map(|(index, b)| (index, b.weight)),
            )
        });
        let mut frontier = Vec::new();
        if pool.frontier {
            let mut scenes: Vec<Option<i64>> = pool
                .buckets
                .iter()
                .filter(|b| b.frontier && !b.units.is_empty())
                .map(|b| b.scene)
                .collect();
            scenes.sort();
            scenes.dedup();
            for scene in scenes {
                let appetite = scene
                    .and_then(|id| {
                        pool.appetite
                            .binary_search_by_key(&id, |(s, _)| *s)
                            .ok()
                            .map(|index| pool.appetite[index].1)
                    })
                    .unwrap_or(SCENELESS_APPETITE);
                let cum = cumulative(
                    pool.buckets
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.frontier && b.scene == scene && !b.units.is_empty())
                        .map(|(index, b)| (index, b.weight)),
                );
                if !cum.is_empty() {
                    frontier.push(FrontierScene { appetite, cum });
                }
            }
        }
        Self {
            pool,
            samplers: Samplers { rings, frontier },
            state: State::default(),
            session: false,
        }
    }

    /// A refill after the schedule: `recent_artists` oldest first (the tail
    /// of the session), `used_keys` the songs and concerts already played.
    pub fn session(
        pool: &'a Pool,
        recent_artists: &[i64],
        used_keys: impl IntoIterator<Item = u64>,
    ) -> Self {
        let mut picker = Self::new(pool);
        picker.session = true;
        picker.state.used.extend(used_keys);
        for artist in recent_artists {
            picker.remember(*artist);
        }
        picker
    }

    fn remember(&mut self, artist: i64) {
        let window = self.pool.spacing.saturating_sub(1);
        if window == 0 {
            return;
        }
        self.state.recent.push_back(artist);
        while self.state.recent.len() > window {
            self.state.recent.pop_front();
        }
    }

    /// A new 6-hour block: songs may come back (concerts may not).
    pub fn new_block(&mut self) {
        if !self.pool.unique_per_day && !self.session {
            self.state.used.clear();
            self.state.exhausted.clear();
        }
    }

    /// The next unit as (bucket, unit) indices.
    pub fn next(&mut self, rng: &mut StdRng) -> Option<(usize, usize)> {
        match self.pool.flow {
            Flow::Rotation => self.next_rotation(rng),
            Flow::Cycle => self.next_cycle(rng),
        }
    }

    fn next_rotation(&mut self, rng: &mut StdRng) -> Option<(usize, usize)> {
        let frontier_turn = self.pool.frontier
            && !self.samplers.frontier.is_empty()
            && self.state.picks % FRONTIER_EVERY == FRONTIER_EVERY - 1;
        self.state.picks += 1;
        if frontier_turn && let Some(bucket) = self.sample_frontier(rng) {
            return Some(self.take(bucket, rng));
        }
        for strictness in [Strictness::Spaced, Strictness::Adjacent] {
            if let Some(bucket) = self.sample_rings(rng, strictness) {
                return Some(self.take(bucket, rng));
            }
        }
        // Only tiny pools get here. A daily schedule would rather repeat a
        // song this block than play one artist back to back, and never goes
        // dark; a session refill never repeats and ends when spent.
        if !self.session && !self.state.used.is_empty() {
            self.state.used.clear();
            self.state.exhausted.clear();
            if let Some(bucket) = self.sample_rings(rng, Strictness::Adjacent) {
                return Some(self.take(bucket, rng));
            }
        }
        self.sample_rings(rng, Strictness::Any)
            .map(|bucket| self.take(bucket, rng))
    }

    fn sample_in(
        pool: &Pool,
        state: &mut State,
        cum: &[(f64, usize)],
        rng: &mut StdRng,
        strictness: Strictness,
    ) -> Option<usize> {
        if cum.is_empty() {
            return None;
        }
        for _ in 0..REJECTION_TRIES {
            let bucket = sample_cumulative(cum, rng)?;
            if state.eligible(pool, bucket, strictness) {
                return Some(bucket);
            }
        }
        let mut previous = 0.0;
        let mut options = Vec::new();
        for (c, bucket) in cum {
            let weight = c - previous;
            previous = *c;
            if state.eligible(pool, *bucket, strictness) {
                options.push((*bucket, weight));
            }
        }
        weighted_choice(&options, rng)
    }

    fn sample_rings(&mut self, rng: &mut StdRng, strictness: Strictness) -> Option<usize> {
        let Picker {
            pool,
            samplers,
            state,
            ..
        } = self;
        let mut alive = [0, 1, 2].map(|ring| !samplers.rings[ring].is_empty());
        loop {
            let options: Vec<(usize, f64)> = (0..3)
                .filter(|ring| alive[*ring])
                .map(|ring| (ring, pool.rings[ring]))
                .collect();
            let ring = weighted_choice(&options, rng)?;
            if let Some(bucket) =
                Self::sample_in(pool, state, &samplers.rings[ring], rng, strictness)
            {
                return Some(bucket);
            }
            alive[ring] = false;
        }
    }

    fn sample_frontier(&mut self, rng: &mut StdRng) -> Option<usize> {
        let Picker {
            pool,
            samplers,
            state,
            ..
        } = self;
        let mut alive: Vec<bool> = vec![true; samplers.frontier.len()];
        loop {
            let options: Vec<(usize, f64)> = samplers
                .frontier
                .iter()
                .enumerate()
                .filter(|(index, _)| alive[*index])
                .map(|(index, scene)| (index, scene.appetite))
                .collect();
            let scene = weighted_choice(&options, rng)?;
            if let Some(bucket) = Self::sample_in(
                pool,
                state,
                &samplers.frontier[scene].cum,
                rng,
                Strictness::Spaced,
            ) {
                return Some(bucket);
            }
            alive[scene] = false;
        }
    }

    fn take(&mut self, bucket: usize, rng: &mut StdRng) -> (usize, usize) {
        let entry = &self.pool.buckets[bucket];
        let options: Vec<(usize, f64)> = entry
            .units
            .iter()
            .enumerate()
            .filter(|(_, unit)| !self.state.used.contains(&unit.key))
            .map(|(index, unit)| (index, unit.weight))
            .collect();
        let unit = weighted_choice(&options, rng).unwrap_or(0);
        let key = entry.units[unit].key;
        self.state.used.insert(key);
        self.state.last_key = Some(key);
        if entry
            .units
            .iter()
            .all(|unit| self.state.used.contains(&unit.key))
        {
            self.state.exhausted.insert(bucket);
        }
        let artist = entry.artist;
        self.remember(artist);
        (bucket, unit)
    }

    fn next_cycle(&mut self, rng: &mut StdRng) -> Option<(usize, usize)> {
        loop {
            while let Some((bucket, unit)) = self.state.cycle.pop_front() {
                let key = self.pool.buckets[bucket].units[unit].key;
                if self.state.used.contains(&key) {
                    continue;
                }
                self.state.used.insert(key);
                self.state.last_key = Some(key);
                return Some((bucket, unit));
            }
            // The loop is spent: reshuffle. A session only reshuffles what it
            // has not had, so it ends once every song aired.
            let mut keyed: Vec<(f64, (usize, usize))> = Vec::new();
            for (b, bucket) in self.pool.buckets.iter().enumerate() {
                for (u, unit) in bucket.units.iter().enumerate() {
                    if self.session && self.state.used.contains(&unit.key) {
                        continue;
                    }
                    let weight = if unit.weight.is_finite() && unit.weight > 0.0 {
                        unit.weight
                    } else {
                        1e-6
                    };
                    let draw: f64 = rng.random_range(f64::EPSILON..1.0);
                    keyed.push((draw.powf(1.0 / weight), (b, u)));
                }
            }
            if keyed.is_empty() {
                return None;
            }
            keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut order: VecDeque<(usize, usize)> = keyed.into_iter().map(|(_, at)| at).collect();
            let first_key = order
                .front()
                .map(|(b, u)| self.pool.buckets[*b].units[*u].key);
            if order.len() > 1 && first_key.is_some() && first_key == self.state.last_key {
                order.swap(0, 1);
            }
            if !self.session {
                self.state.used.clear();
            }
            self.state.cycle = order;
        }
    }
}

/// A channel's day: four 6-hour blocks laid back to back from midnight, each
/// seeded by (channel, day, block). A block starts where the previous one's
/// last program ended; the last program of the day may run past midnight
/// (tomorrow's schedule still starts at 0).
pub fn schedule_day(pool: &Pool, channel: &str, day: &str) -> Vec<Program> {
    let mut picker = Picker::new(pool);
    let mut programs: Vec<Program> = Vec::new();
    let mut cursor = 0i64;
    for block in 0..BLOCKS {
        let mut rng = StdRng::seed_from_u64(clock::seed(channel, day, block));
        picker.new_block();
        let end = (block as i64 + 1) * BLOCK_S;
        while cursor < end {
            let Some((bucket, unit)) = picker.next(&mut rng) else {
                return programs;
            };
            let unit = &pool.buckets[bucket].units[unit];
            for (video, seconds) in unit.videos() {
                let seconds = seconds.max(1);
                programs.push(Program {
                    slot: programs.len() as i64,
                    start_s: cursor,
                    duration_s: seconds,
                    video_id: video,
                    why: unit.why.clone(),
                });
                cursor += seconds;
            }
        }
    }
    programs
}

/// Up to `videos` videos (whole units, so a concert may run over) for a
/// session refill.
pub fn refill(picker: &mut Picker, rng: &mut StdRng, videos: usize) -> Vec<(i64, String)> {
    let pool = picker.pool;
    let mut out = Vec::new();
    while out.len() < videos {
        let Some((bucket, unit)) = picker.next(rng) else {
            break;
        };
        let unit = &pool.buckets[bucket].units[unit];
        out.extend(unit.videos().map(|(video, _)| (video, unit.why.clone())));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    pub(crate) fn song(id: i64, key: u64, seconds: i64) -> Unit {
        Unit::song(id, seconds, key, 1.0)
    }

    /// `artists` artists with `songs` songs each, rings by artist index.
    pub(crate) fn pool(artists: i64, songs: i64) -> Pool {
        let buckets = (0..artists)
            .map(|artist| Bucket {
                artist: 1000 + artist,
                ring: (artist % 3) as usize,
                weight: ((songs as f64).sqrt()) * (1.0 + (artist % 5) as f64),
                frontier: artist % 3 == 2,
                scene: Some(artist % 4),
                units: (0..songs)
                    .map(|n| {
                        let id = artist * 1000 + n;
                        song(id, id as u64, 180 + (id % 7) * 20)
                    })
                    .collect(),
            })
            .collect();
        Pool {
            buckets,
            ..Pool::default()
        }
    }

    fn artist_of(pool: &Pool) -> HashMap<i64, i64> {
        pool.buckets
            .iter()
            .flat_map(|b| {
                b.units
                    .iter()
                    .flat_map(move |u| u.videos().map(move |(v, _)| (v, b.artist)))
            })
            .collect()
    }

    fn key_of(pool: &Pool) -> HashMap<i64, u64> {
        pool.buckets
            .iter()
            .flat_map(|b| {
                b.units
                    .iter()
                    .flat_map(|u| u.videos().map(move |(v, _)| (v, u.key)))
            })
            .collect()
    }

    #[test]
    fn a_day_is_deterministic_per_seed() {
        let pool = pool(60, 12);
        let a = schedule_day(&pool, "wild", "2026-10-14");
        let b = schedule_day(&pool, "wild", "2026-10-14");
        assert_eq!(a, b);
        let other_day = schedule_day(&pool, "wild", "2026-10-15");
        let other_channel = schedule_day(&pool, "your", "2026-10-14");
        assert_ne!(a, other_day);
        assert_ne!(a, other_channel);
        assert!(a.last().unwrap().end_s() >= clock::DAY_S);
        for pair in a.windows(2) {
            assert_eq!(pair[0].end_s(), pair[1].start_s, "back to back, no gaps");
        }
    }

    #[test]
    fn ten_thousand_slots_keep_artist_spacing_and_block_songs_unique() {
        let mut pool = pool(80, 10);
        pool.frontier = true;
        pool.appetite = vec![(0, 1.0), (1, 0.5), (2, 0.2), (3, 0.05)];
        let artists = artist_of(&pool);
        let keys = key_of(&pool);
        let mut slots = 0usize;
        let mut day_index = 0;
        while slots < 10_000 {
            let day = format!("2026-11-{:02}", day_index % 28 + 1);
            let day = format!("{day}-{}", day_index / 28);
            day_index += 1;
            let programs = schedule_day(&pool, "wild", &day);
            slots += programs.len();
            let sequence: Vec<i64> = programs.iter().map(|p| artists[&p.video_id]).collect();
            for window in sequence.windows(SPACING) {
                let distinct: HashSet<i64> = window.iter().copied().collect();
                assert_eq!(
                    distinct.len(),
                    SPACING,
                    "artist repeated within 12 slots on {day}"
                );
            }
            let mut block_songs: Vec<HashSet<u64>> = vec![HashSet::new(); BLOCKS];
            let mut cursor_block = 0usize;
            for program in &programs {
                // A program belongs to the block that was filling when it
                // started: the block whose end it had not yet reached.
                while cursor_block + 1 < BLOCKS
                    && program.start_s >= (cursor_block as i64 + 1) * BLOCK_S
                {
                    cursor_block += 1;
                }
                assert!(
                    block_songs[cursor_block].insert(keys[&program.video_id]),
                    "song twice in block {cursor_block} on {day}"
                );
            }
        }
        assert!(slots >= 10_000);
    }

    #[test]
    fn rings_follow_the_explore_weights() {
        let mut pool = pool(90, 40);
        for bucket in &mut pool.buckets {
            bucket.weight = 1.0;
        }
        pool.rings = [0.8, 0.18, 0.02];
        let ring_of: HashMap<i64, usize> =
            pool.buckets.iter().map(|b| (b.artist, b.ring)).collect();
        let artists = artist_of(&pool);
        let programs = schedule_day(&pool, "your", "2026-10-14");
        let mut counts = [0usize; 3];
        for program in &programs {
            counts[ring_of[&artists[&program.video_id]]] += 1;
        }
        let total = programs.len() as f64;
        let near = counts[0] as f64 / total;
        let far = counts[2] as f64 / total;
        assert!((0.7..0.9).contains(&near), "near {near}");
        assert!(far < 0.08, "far {far}");
    }

    #[test]
    fn every_fourth_pick_is_frontier_when_there_is_one() {
        let mut pool = pool(60, 10);
        pool.frontier = true;
        pool.rings = [1.0, 0.0, 0.0];
        let frontier: HashSet<i64> = pool
            .buckets
            .iter()
            .filter(|b| b.frontier)
            .map(|b| b.artist)
            .collect();
        let artists = artist_of(&pool);
        let programs = schedule_day(&pool, "wild", "2026-10-14");
        for (index, program) in programs.iter().enumerate() {
            let is_frontier = frontier.contains(&artists[&program.video_id]);
            if index % FRONTIER_EVERY == FRONTIER_EVERY - 1 {
                assert!(is_frontier, "slot {index} should be frontier");
            } else {
                assert!(!is_frontier, "near ring only otherwise (slot {index})");
            }
        }
    }

    #[test]
    fn frontier_scenes_follow_appetite() {
        let mut pool = pool(120, 20);
        pool.frontier = true;
        pool.rings = [1.0, 0.0, 0.0];
        for bucket in &mut pool.buckets {
            bucket.weight = 1.0;
        }
        pool.appetite = vec![(0, 1.0), (1, 1.0), (2, 0.05), (3, 0.05)];
        let scene_of: HashMap<i64, Option<i64>> =
            pool.buckets.iter().map(|b| (b.artist, b.scene)).collect();
        let artists = artist_of(&pool);
        let mut hungry = 0;
        let mut quiet = 0;
        for day in 1..=6 {
            let programs = schedule_day(&pool, "wild", &format!("2026-10-{day:02}"));
            for (index, program) in programs.iter().enumerate() {
                if index % FRONTIER_EVERY != FRONTIER_EVERY - 1 {
                    continue;
                }
                match scene_of[&artists[&program.video_id]] {
                    Some(0) | Some(1) => hungry += 1,
                    _ => quiet += 1,
                }
            }
        }
        assert!(hungry > quiet * 5, "hungry {hungry} quiet {quiet}");
    }

    #[test]
    fn concerts_play_whole_and_in_setlist_order() {
        let buckets: Vec<Bucket> = (0..30)
            .map(|artist| Bucket {
                artist,
                ring: 0,
                weight: 1.0,
                frontier: false,
                scene: None,
                units: (0..3)
                    .map(|c| {
                        let id = artist * 10 + c;
                        let setlist: Vec<(i64, i64)> =
                            (0..(3 + c)).map(|n| (id * 100 + n, 240)).collect();
                        Unit::set(&setlist, id as u64, 1.0, format!("Concert {id}")).unwrap()
                    })
                    .collect(),
            })
            .collect();
        let pool = Pool {
            buckets,
            unique_per_day: true,
            ..Pool::default()
        };
        let programs = schedule_day(&pool, "concerts", "2026-10-14");
        let mut index = 0;
        let mut seen = HashSet::new();
        while index < programs.len() {
            let concert = programs[index].video_id / 100;
            assert!(seen.insert(concert), "a concert airs once a day");
            let length = 3 + (concert % 10);
            for n in 0..length {
                let program = &programs[index + n as usize];
                assert_eq!(
                    program.video_id,
                    concert * 100 + n,
                    "setlist order, unsplit"
                );
            }
            index += length as usize;
        }
        assert_eq!(index, programs.len(), "the last concert is whole too");
    }

    #[test]
    fn spotlight_cycles_every_song_before_repeating() {
        let pool = Pool {
            buckets: vec![Bucket {
                artist: 7,
                ring: 0,
                weight: 1.0,
                frontier: false,
                scene: None,
                units: (0..9).map(|n| song(n, n as u64, 200)).collect(),
            }],
            flow: Flow::Cycle,
            spacing: 0,
            ..Pool::default()
        };
        let programs = schedule_day(&pool, "spotlight", "2026-10-14");
        assert!(programs.last().unwrap().end_s() >= clock::DAY_S);
        for chunk in programs.chunks(9).filter(|c| c.len() == 9) {
            let distinct: HashSet<i64> = chunk.iter().map(|p| p.video_id).collect();
            assert_eq!(distinct.len(), 9);
        }
        for pair in programs.windows(2) {
            assert_ne!(
                pair[0].video_id, pair[1].video_id,
                "no repeat at a cycle seam"
            );
        }
    }

    #[test]
    fn a_tiny_pool_relaxes_instead_of_going_dark() {
        let pool = pool(3, 2);
        let artists = artist_of(&pool);
        let programs = schedule_day(&pool, "scene:1", "2026-10-14");
        assert!(programs.last().unwrap().end_s() >= clock::DAY_S);
        for pair in programs.windows(2) {
            assert_ne!(
                artists[&pair[0].video_id], artists[&pair[1].video_id],
                "never back to back while another artist has songs"
            );
        }
    }

    #[test]
    fn a_session_refill_skips_played_songs_and_ends_when_spent() {
        let pool = pool(4, 3);
        let played: Vec<u64> = vec![0, 1, 1000];
        let mut picker = Picker::session(&pool, &[1000, 1001], played.clone());
        let mut rng = StdRng::seed_from_u64(9);
        let first = refill(&mut picker, &mut rng, 4);
        assert_eq!(first.len(), 4);
        let keys = key_of(&pool);
        assert!(first.iter().all(|(v, _)| !played.contains(&keys[v])));
        let artists = artist_of(&pool);
        assert_ne!(
            artists[&first[0].0], 1001,
            "the session's last artist waits"
        );
        let rest = refill(&mut picker, &mut rng, 100);
        assert_eq!(first.len() + rest.len(), 12 - played.len());
        assert!(refill(&mut picker, &mut rng, 12).is_empty());
    }
}
