//! Turns relevance and the artist ledger into a ranked job list. Pure.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use super::artist_state::ArtistState;
use super::graph::Relevance;

#[derive(Debug, Clone, PartialEq)]
pub enum JobKind {
    Probe,
    Recheck,
    Page { offset: i64 },
    Expand,
    ResolveVideo { video_id: i64 },
    GenreSearch { genre: String },
    HarvestMixes,
    HarvestEditorial { playlists: bool },
    ArtistMix { mix_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobClass {
    Normal,
    Priority,
    Urgent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub artist_id: i64,
    pub kind: JobKind,
    pub class: JobClass,
    pub value: f64,
}

impl Job {
    pub fn estimated_calls(&self) -> usize {
        match self.kind {
            JobKind::Expand => 4,
            JobKind::HarvestMixes => 5,
            JobKind::HarvestEditorial { playlists: true } => 11,
            _ => 1,
        }
    }
}

pub fn label(kind: &JobKind) -> &'static str {
    match kind {
        JobKind::Probe => "probe",
        JobKind::Recheck => "recheck",
        JobKind::Page { .. } => "page",
        JobKind::Expand => "expand",
        JobKind::ResolveVideo { .. } => "resolve_video",
        JobKind::GenreSearch { .. } => "genre_search",
        JobKind::HarvestMixes => "harvest_mixes",
        JobKind::HarvestEditorial { .. } => "harvest_editorial",
        JobKind::ArtistMix { .. } => "artist_mix",
    }
}

/// Popularity thresholds and the measured share of artists with videos.
pub const POPULARITY_DEFAULTS: [(i32, f64); 6] = [
    (80, 0.95),
    (70, 0.85),
    (60, 0.65),
    (50, 0.33),
    (40, 0.23),
    (10, 0.08),
];
pub const UNKNOWN_POPULARITY_P: f64 = 0.5;
const CALIBRATION_MIN_SAMPLES: usize = 50;
pub const PRIORITY_RECHECK_WITH_VIDEOS_DAYS: f64 = 7.0;
pub const PRIORITY_RECHECK_WITHOUT_VIDEOS_DAYS: f64 = 14.0;
pub const DEEP_PAGE_MIN_RELEVANCE: f64 = 0.3;
const EXPAND_CALLS: f64 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    pub by_bucket: [f64; 6],
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            by_bucket: POPULARITY_DEFAULTS.map(|(_, p)| p),
        }
    }
}

fn bucket(popularity: i32) -> Option<usize> {
    POPULARITY_DEFAULTS
        .iter()
        .position(|(threshold, _)| popularity >= *threshold)
}

impl Calibration {
    /// Measured rates replace defaults only where a bucket has enough checks.
    pub fn from_outcomes(outcomes: &[(i32, bool)]) -> Self {
        let mut counts = [(0usize, 0usize); 6];
        for (popularity, has_videos) in outcomes {
            if let Some(index) = bucket(*popularity) {
                counts[index].0 += 1;
                counts[index].1 += usize::from(*has_videos);
            }
        }
        let mut calibration = Self::default();
        for (index, (total, hits)) in counts.iter().enumerate() {
            if *total >= CALIBRATION_MIN_SAMPLES {
                calibration.by_bucket[index] = *hits as f64 / *total as f64;
            }
        }
        calibration
    }

    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT popularity, seen_main = 1 OR fetched_count > 0 FROM video_artist_state
              WHERE last_checked_at IS NOT NULL AND popularity > 0",
        )?;
        let outcomes = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<(i32, bool)>, _>>()?;
        Ok(Self::from_outcomes(&outcomes))
    }

    pub fn p_unprobed(&self, popularity: Option<i32>) -> f64 {
        match popularity {
            Some(p) if p > 0 => bucket(p).map_or(self.by_bucket[5], |index| self.by_bucket[index]),
            _ => UNKNOWN_POPULARITY_P,
        }
    }
}

pub fn p_has_videos(state: Option<&ArtistState>, calibration: &Calibration) -> f64 {
    match state {
        Some(s) if s.has_videos() => 1.0,
        Some(s) if !s.never_checked() => 0.03,
        Some(s) if s.seen_featured => 0.7,
        Some(s) => calibration.p_unprobed(s.popularity),
        None => UNKNOWN_POPULARITY_P,
    }
}

pub struct PlanInput<'a> {
    /// Relevance from every root (liked to 2 hops, others to 3).
    pub relevance: &'a HashMap<i64, Relevance>,
    /// Relevance from enjoyed and station roots only (3 hops).
    pub deep: &'a HashMap<i64, Relevance>,
    pub states: &'a HashMap<i64, ArtistState>,
    pub roots: &'a HashSet<i64>,
    /// Artists whose libraries are kept complete and fresh.
    pub priority: &'a HashSet<i64>,
    pub calibration: &'a Calibration,
}

fn catalog_job(
    artist_id: i64,
    relevance: f64,
    state: Option<&ArtistState>,
    priority: bool,
    p: f64,
) -> Option<Job> {
    let class = if priority {
        JobClass::Priority
    } else {
        JobClass::Normal
    };
    let job = |kind, value| {
        Some(Job {
            artist_id,
            kind,
            class,
            value,
        })
    };
    let Some(state) = state.filter(|s| !s.never_checked()) else {
        return job(JobKind::Probe, relevance * p);
    };
    let age = state.checked_age_days.unwrap_or(0.0);
    let priority_interval = if state.has_videos() {
        PRIORITY_RECHECK_WITH_VIDEOS_DAYS
    } else {
        PRIORITY_RECHECK_WITHOUT_VIDEOS_DAYS
    };
    if (priority && age >= priority_interval) || state.check_due {
        let interval = if priority {
            priority_interval
        } else if state.has_videos() {
            30.0
        } else {
            60.0
        };
        return job(
            JobKind::Recheck,
            relevance * (age / interval).min(1.0) * 0.4,
        );
    }
    if state
        .total_videos
        .is_some_and(|total| total > state.fetched_count)
        && (priority || relevance >= DEEP_PAGE_MIN_RELEVANCE)
    {
        return job(
            JobKind::Page {
                offset: state.fetched_count,
            },
            relevance * 0.5,
        );
    }
    None
}

fn expand_job(
    artist_id: i64,
    relevance: &Relevance,
    deep: Option<&Relevance>,
    state: Option<&ArtistState>,
    is_root: bool,
    p: f64,
) -> Option<Job> {
    if state.is_some_and(|s| !s.expand_due) {
        return None;
    }
    let within_depth = is_root || relevance.hops <= 1 || deep.is_some_and(|d| d.hops <= 2);
    if !within_depth {
        return None;
    }
    let has_videos = state.is_some_and(ArtistState::has_videos);
    let factor = if has_videos || is_root { 1.0 } else { 0.3 };
    Some(Job {
        artist_id,
        kind: JobKind::Expand,
        class: JobClass::Normal,
        value: relevance.score * p * factor / EXPAND_CALLS,
    })
}

fn rank(jobs: &mut [Job]) {
    jobs.sort_by(|a, b| {
        b.class
            .cmp(&a.class)
            .then(b.value.total_cmp(&a.value))
            .then(a.artist_id.cmp(&b.artist_id))
    });
}

pub fn plan(input: &PlanInput, limit: usize) -> Vec<Job> {
    let mut jobs = Vec::new();
    for (&artist_id, relevance) in input.relevance {
        let state = input.states.get(&artist_id);
        let priority = input.priority.contains(&artist_id);
        let p = p_has_videos(state, input.calibration);
        if let Some(job) = catalog_job(artist_id, relevance.score, state, priority, p) {
            jobs.push(job);
        }
        let is_root = input.roots.contains(&artist_id);
        if let Some(job) = expand_job(
            artist_id,
            relevance,
            input.deep.get(&artist_id),
            state,
            is_root,
            p,
        ) {
            jobs.push(job);
        }
    }
    rank(&mut jobs);
    jobs.truncate(limit);
    jobs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixPolicy {
    Off,
    Trial { remaining: usize },
    On,
}

pub const MIX_RECHECK_DAYS: f64 = 14.0;

/// `plan` plus artist-mix jobs for roots, gated by the trial verdict.
pub fn plan_with_mixes(input: &PlanInput, policy: MixPolicy, limit: usize) -> Vec<Job> {
    let mut jobs = plan(input, usize::MAX);
    let allowed = match policy {
        MixPolicy::Off => 0,
        MixPolicy::Trial { remaining } => remaining,
        MixPolicy::On => usize::MAX,
    };
    let mut mixes: Vec<Job> = input
        .roots
        .iter()
        .filter_map(|id| {
            let state = input.states.get(id)?;
            let mix_id = state.mix_id.clone()?;
            if state.mix_age_days.is_some_and(|age| age < MIX_RECHECK_DAYS) {
                return None;
            }
            let relevance = input.relevance.get(id).map_or(1.0, |r| r.score);
            Some(Job {
                artist_id: *id,
                kind: JobKind::ArtistMix { mix_id },
                class: JobClass::Normal,
                value: relevance * 0.9,
            })
        })
        .collect();
    rank(&mut mixes);
    jobs.extend(mixes.into_iter().take(allowed));
    rank(&mut jobs);
    jobs.truncate(limit);
    jobs
}

/// Work for a station that is running thin: expand and check the seed, then
/// probe its most promising unchecked neighbors.
pub fn station_jobs(
    seed: i64,
    relevance: &HashMap<i64, Relevance>,
    states: &HashMap<i64, ArtistState>,
    calibration: &Calibration,
    limit: usize,
) -> Vec<Job> {
    let urgent = |artist_id, kind, value| Job {
        artist_id,
        kind,
        class: JobClass::Urgent,
        value,
    };
    let mut jobs = Vec::new();
    let seed_state = states.get(&seed);
    if seed_state.is_none_or(|s| s.expand_due) {
        jobs.push(urgent(seed, JobKind::Expand, 1.0));
    }
    match seed_state {
        Some(s) if !s.never_checked() => {
            if s.check_due
                || s.checked_age_days
                    .is_some_and(|age| age >= PRIORITY_RECHECK_WITH_VIDEOS_DAYS)
            {
                jobs.push(urgent(seed, JobKind::Recheck, 1.0));
            }
        }
        _ => jobs.push(urgent(seed, JobKind::Probe, 1.0)),
    }
    let mut probes: Vec<Job> = relevance
        .iter()
        .filter(|(id, _)| **id != seed)
        .filter(|(id, _)| states.get(id).is_none_or(ArtistState::never_checked))
        .map(|(id, rel)| {
            urgent(
                *id,
                JobKind::Probe,
                rel.score * p_has_videos(states.get(id), calibration),
            )
        })
        .collect();
    rank(&mut probes);
    let room = limit.saturating_sub(jobs.len());
    jobs.extend(probes.into_iter().take(room));
    jobs.truncate(limit);
    jobs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(id: i64) -> ArtistState {
        ArtistState {
            artist_tidal_id: id,
            expand_due: true,
            check_due: true,
            ..Default::default()
        }
    }

    fn checked(id: i64, age: f64, has_videos: bool, due: bool) -> ArtistState {
        ArtistState {
            artist_tidal_id: id,
            checked_age_days: Some(age),
            check_due: due,
            fetched_count: if has_videos { 10 } else { 0 },
            seen_main: has_videos,
            ..Default::default()
        }
    }

    fn rel(score: f64, hops: u8) -> Relevance {
        Relevance { score, hops }
    }

    #[test]
    fn popularity_drives_the_unprobed_estimate() {
        let calibration = Calibration::default();
        assert_eq!(calibration.p_unprobed(Some(85)), 0.95);
        assert_eq!(calibration.p_unprobed(Some(55)), 0.33);
        assert_eq!(calibration.p_unprobed(Some(12)), 0.08);
        assert_eq!(calibration.p_unprobed(Some(5)), 0.08);
        assert_eq!(calibration.p_unprobed(None), 0.5);
        let mut featured = state(1);
        featured.seen_featured = true;
        assert_eq!(p_has_videos(Some(&featured), &calibration), 0.7);
        assert_eq!(
            p_has_videos(Some(&checked(2, 3.0, false, false)), &calibration),
            0.03
        );
    }

    #[test]
    fn calibration_needs_fifty_samples_per_bucket() {
        let few: Vec<(i32, bool)> = (0..49).map(|_| (90, false)).collect();
        assert_eq!(Calibration::from_outcomes(&few), Calibration::default());
        let many: Vec<(i32, bool)> = (0..100).map(|i| (90, i % 2 == 0)).collect();
        assert_eq!(Calibration::from_outcomes(&many).by_bucket[0], 0.5);
    }

    #[test]
    fn popular_unchecked_neighbors_outrank_obscure_ones() {
        let mut popular = state(2);
        popular.popularity = Some(85);
        let mut obscure = state(3);
        obscure.popularity = Some(20);
        let states = HashMap::from([(2, popular), (3, obscure)]);
        let relevance = HashMap::from([(2, rel(0.8, 1)), (3, rel(0.8, 1))]);
        let empty = HashSet::new();
        let calibration = Calibration::default();
        let jobs = plan(
            &PlanInput {
                relevance: &relevance,
                deep: &HashMap::new(),
                states: &states,
                roots: &empty,
                priority: &empty,
                calibration: &calibration,
            },
            10,
        );
        let probes: Vec<i64> = jobs
            .iter()
            .filter(|j| j.kind == JobKind::Probe)
            .map(|j| j.artist_id)
            .collect();
        assert_eq!(probes, vec![2, 3]);
    }

    #[test]
    fn priority_artists_are_rechecked_weekly_and_others_wait() {
        let states = HashMap::from([
            (1, checked(1, 8.0, true, false)),
            (2, checked(2, 8.0, true, false)),
            (3, checked(3, 15.0, false, false)),
            (4, checked(4, 13.0, false, false)),
        ]);
        let relevance: HashMap<i64, Relevance> = [1, 2, 3, 4]
            .into_iter()
            .map(|id| (id, rel(0.5, 1)))
            .collect();
        let priority = HashSet::from([1, 3, 4]);
        let calibration = Calibration::default();
        let jobs = plan(
            &PlanInput {
                relevance: &relevance,
                deep: &HashMap::new(),
                states: &states,
                roots: &HashSet::new(),
                priority: &priority,
                calibration: &calibration,
            },
            20,
        );
        let rechecks: HashSet<i64> = jobs
            .iter()
            .filter(|j| j.kind == JobKind::Recheck)
            .map(|j| j.artist_id)
            .collect();
        assert_eq!(rechecks, HashSet::from([1, 3]));
        assert!(
            jobs.iter()
                .filter(|j| j.kind == JobKind::Recheck)
                .all(|j| j.class == JobClass::Priority)
        );
    }

    #[test]
    fn priority_libraries_are_paged_to_completion() {
        let mut deep_catalog = checked(5, 1.0, true, false);
        deep_catalog.total_videos = Some(120);
        deep_catalog.fetched_count = 50;
        let mut shallow_other = deep_catalog.clone();
        shallow_other.artist_tidal_id = 6;
        let states = HashMap::from([(5, deep_catalog), (6, shallow_other)]);
        let relevance = HashMap::from([(5, rel(0.2, 1)), (6, rel(0.2, 1))]);
        let priority = HashSet::from([5]);
        let calibration = Calibration::default();
        let jobs = plan(
            &PlanInput {
                relevance: &relevance,
                deep: &HashMap::new(),
                states: &states,
                roots: &HashSet::new(),
                priority: &priority,
                calibration: &calibration,
            },
            20,
        );
        assert!(
            jobs.iter()
                .any(|j| j.artist_id == 5 && j.kind == JobKind::Page { offset: 50 })
        );
        assert!(
            !jobs
                .iter()
                .any(|j| j.artist_id == 6 && matches!(j.kind, JobKind::Page { .. }))
        );
    }

    #[test]
    fn expansion_stays_within_depth() {
        let states = HashMap::new();
        let relevance = HashMap::from([
            (1, rel(1.0, 0)),
            (2, rel(0.6, 1)),
            (3, rel(0.4, 2)),
            (4, rel(0.4, 2)),
        ]);
        let deep = HashMap::from([(4, rel(0.4, 2))]);
        let roots = HashSet::from([1]);
        let calibration = Calibration::default();
        let jobs = plan(
            &PlanInput {
                relevance: &relevance,
                deep: &deep,
                states: &states,
                roots: &roots,
                priority: &roots,
                calibration: &calibration,
            },
            20,
        );
        let expanded: HashSet<i64> = jobs
            .iter()
            .filter(|j| j.kind == JobKind::Expand)
            .map(|j| j.artist_id)
            .collect();
        assert_eq!(expanded, HashSet::from([1, 2, 4]));
    }

    #[test]
    fn a_thin_station_expands_and_probes_its_seed_first() {
        let relevance = HashMap::from([(50, rel(1.0, 0)), (51, rel(0.9, 1)), (52, rel(0.3, 1))]);
        let mut known = state(52);
        known.checked_age_days = Some(1.0);
        let states = HashMap::from([(52, known)]);
        let jobs = station_jobs(50, &relevance, &states, &Calibration::default(), 6);
        assert_eq!(
            jobs[0],
            Job {
                artist_id: 50,
                kind: JobKind::Expand,
                class: JobClass::Urgent,
                value: 1.0
            }
        );
        assert_eq!(jobs[1].kind, JobKind::Probe);
        assert_eq!(jobs[1].artist_id, 50);
        assert_eq!(jobs.iter().filter(|j| j.artist_id == 51).count(), 1);
        assert!(!jobs.iter().any(|j| j.artist_id == 52), "already checked");
    }

    #[test]
    fn artist_mixes_follow_the_trial_verdict() {
        let mut with_mix = state(1);
        with_mix.mix_id = Some("m1".into());
        with_mix.expand_due = false;
        with_mix.checked_age_days = Some(1.0);
        with_mix.check_due = false;
        let mut other = with_mix.clone();
        other.artist_tidal_id = 2;
        other.mix_id = Some("m2".into());
        let states = HashMap::from([(1, with_mix), (2, other)]);
        let relevance = HashMap::from([(1, rel(1.0, 0)), (2, rel(1.0, 0))]);
        let roots = HashSet::from([1, 2]);
        let calibration = Calibration::default();
        let count = |policy| {
            plan_with_mixes(
                &PlanInput {
                    relevance: &relevance,
                    deep: &HashMap::new(),
                    states: &states,
                    roots: &roots,
                    priority: &roots,
                    calibration: &calibration,
                },
                policy,
                20,
            )
            .iter()
            .filter(|j| matches!(j.kind, JobKind::ArtistMix { .. }))
            .count()
        };
        assert_eq!(count(MixPolicy::Off), 0);
        assert_eq!(count(MixPolicy::Trial { remaining: 1 }), 1);
        assert_eq!(count(MixPolicy::On), 2);
    }
}
