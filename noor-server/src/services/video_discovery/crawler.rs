//! The background loop that owns every video-discovery call to TIDAL. It plans
//! from the ledger, spends a governed budget on the most valuable jobs, and
//! jumps the queue for a station that is running thin.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Notify;

use super::artist_state::{self, CheckResult};
use super::governor::{self, Governor, Mode};
use super::harvest::{self, HarvestContext};
use super::scheduler::{self, Calibration, Job, JobClass, JobKind, MixPolicy, PlanInput};
use super::source::{DiscoverySource, LiveSource};
use super::{expand, graph, names, roots};
use crate::SharedState;
use crate::db::Database;
use crate::metadata::lastfm::LastFmClient;
use crate::services::tidal::auth::error_looks_like_auth;
use crate::services::tidal::client::TidalClient;
use crate::services::video_sets::VideoCandidate;
use crate::services::{library_videos, video_radio};

const BOOT_DELAY: Duration = Duration::from_secs(120);
const REPLAN_EVERY: Duration = Duration::from_secs(120);
const PLAN_BATCH: usize = 50;
const IDLE_NAP: Duration = Duration::from_secs(300);
const AUTH_PAUSE: Duration = Duration::from_secs(300);
const STATION_JOB_LIMIT: usize = 6;
const MAX_MIXES: usize = 4;
const MAX_EDITORIAL_PLAYLISTS: usize = 10;
const MIXES_KEY: &str = "video_discovery.mixes_at";
const EDITORIAL_KEY: &str = "video_discovery.editorial_at";
const EDITORIAL_FULL_KEY: &str = "video_discovery.editorial_full_at";
const POPULARITY_BACKFILL_KEY: &str = "video_discovery.popularity_backfill_v1";

// --- Urgent requests ------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Urgent {
    Station(i64),
    Video(i64),
}

#[derive(Debug, Default)]
pub struct UrgentQueue {
    queue: VecDeque<Urgent>,
    building: BTreeSet<Urgent>,
}

impl UrgentQueue {
    pub const fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            building: BTreeSet::new(),
        }
    }

    /// Queue once; a request already being built is not queued again.
    pub fn request(&mut self, urgent: Urgent) -> bool {
        if !self.building.insert(urgent) {
            return false;
        }
        self.queue.push_back(urgent);
        true
    }

    pub fn next(&mut self) -> Option<Urgent> {
        self.queue.pop_front()
    }

    pub fn finish(&mut self, urgent: Urgent) {
        self.building.remove(&urgent);
    }

    pub fn is_building(&self, urgent: Urgent) -> bool {
        self.building.contains(&urgent)
    }
}

static URGENT: Mutex<UrgentQueue> = Mutex::new(UrgentQueue::new());

fn wake_signal() -> &'static Notify {
    static WAKE: OnceLock<Notify> = OnceLock::new();
    WAKE.get_or_init(Notify::new)
}

pub fn request(urgent: Urgent) {
    let queued = URGENT
        .lock()
        .map(|mut q| q.request(urgent))
        .unwrap_or(false);
    if queued {
        wake_signal().notify_one();
    }
}

pub fn is_building(urgent: Urgent) -> bool {
    URGENT
        .lock()
        .map(|q| q.is_building(urgent))
        .unwrap_or(false)
}

fn next_urgent() -> Option<Urgent> {
    URGENT.lock().ok()?.next()
}

fn finish(urgent: Urgent) {
    if let Ok(mut queue) = URGENT.lock() {
        queue.finish(urgent);
    }
}

pub async fn wait_until_built(urgent: Urgent, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while is_building(urgent) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// --- Stats ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
pub struct HourStats {
    pub jobs: BTreeMap<&'static str, u64>,
    pub calls: u64,
    pub new_videos: u64,
    pub empty_checks: u64,
    pub failures: u64,
}

static STATS: Mutex<Option<HourStats>> = Mutex::new(None);
static BUDGET: Mutex<(usize, usize, &str)> = Mutex::new((0, 0, "idle"));

fn record_stats(job: &Job, report: &JobReport) {
    if let Ok(mut stats) = STATS.lock() {
        let stats = stats.get_or_insert_with(HourStats::default);
        *stats.jobs.entry(scheduler::label(&job.kind)).or_default() += 1;
        stats.calls += report.calls as u64;
        stats.new_videos += report.new_videos as u64;
        stats.empty_checks += u64::from(report.empty);
        stats.failures += u64::from(report.failed);
    }
}

fn publish_budget(governor: &Governor, mode: Mode) {
    if let Ok(mut budget) = BUDGET.lock() {
        *budget = (
            governor.calls_last_hour(),
            governor.calls_today(),
            mode.as_str(),
        );
    }
}

fn log_hour_summary() {
    let Some(stats) = STATS.lock().ok().and_then(|mut s| s.take()) else {
        return;
    };
    tracing::info!(
        target: "noor.video_discovery",
        calls = stats.calls,
        new_videos = stats.new_videos,
        empty_checks = stats.empty_checks,
        failures = stats.failures,
        jobs = ?stats.jobs,
        "video discovery hour"
    );
}

// --- Job execution --------------------------------------------------------

#[derive(Debug, Default)]
pub struct JobReport {
    pub calls: usize,
    pub new_videos: usize,
    pub empty: bool,
    pub failed: bool,
    pub auth_failed: bool,
    pub follow_up: Option<Job>,
}

pub async fn execute<S: DiscoverySource>(db: &Database, src: &S, job: &Job) -> JobReport {
    let result = match &job.kind {
        JobKind::Probe | JobKind::Recheck => catalog_page(db, src, job, 0).await,
        JobKind::Page { offset } => catalog_page(db, src, job, *offset).await,
        JobKind::Expand => {
            let station = roots::station_active(job.artist_id);
            expand::expand(db, src, job.artist_id, station)
                .await
                .map(|outcome| JobReport {
                    calls: outcome.tidal_calls,
                    failed: !outcome.ok,
                    ..Default::default()
                })
        }
        JobKind::ResolveVideo { video_id } => resolve_video(db, src, *video_id).await,
        JobKind::GenreSearch { genre } => genre_search(db, src, genre).await,
        JobKind::HarvestMixes => harvest_mixes(db, src).await,
        JobKind::HarvestEditorial { playlists } => harvest_editorial(db, src, *playlists).await,
        JobKind::ArtistMix { mix_id } => artist_mix(db, src, job.artist_id, mix_id).await,
    };
    result.unwrap_or_else(|error| {
        tracing::debug!(target: "noor.video_discovery", %error, job = scheduler::label(&job.kind), "job failed");
        JobReport {
            calls: job.estimated_calls(),
            failed: true,
            auth_failed: error_looks_like_auth(&error),
            ..Default::default()
        }
    })
}

async fn catalog_page<S: DiscoverySource>(
    db: &Database,
    src: &S,
    job: &Job,
    offset: i64,
) -> Result<JobReport> {
    let id = job.artist_id;
    let page = match src.artist_videos(id, offset).await {
        Ok(page) => page,
        Err(error) if error_looks_like_auth(&error) => return Err(error),
        Err(_) => {
            if offset == 0 {
                db.with_conn(|conn| artist_state::record_check(conn, id, CheckResult::Failed))?;
            }
            return Ok(JobReport {
                calls: 1,
                failed: true,
                ..Default::default()
            });
        }
    };
    let name = db.with_conn(|conn| {
        Ok(artist_state::get(conn, id)?
            .map(|s| s.name)
            .unwrap_or_default())
    })?;
    let candidates: Vec<VideoCandidate> = page.videos.iter().map(VideoCandidate::from).collect();
    let received = candidates.len() as i64;
    let summary = db.with_conn(|conn| {
        let summary = harvest::ingest(
            conn,
            &candidates,
            HarvestContext::ArtistPage {
                artist_id: id,
                name: &name,
            },
        )?;
        artist_state::record_page(conn, id, offset, received, page.total)?;
        if offset == 0 {
            let result = if received == 0 {
                CheckResult::Empty
            } else {
                CheckResult::Found {
                    new_videos: summary.new_videos as i64,
                }
            };
            artist_state::record_check(conn, id, result)?;
        }
        library_videos::match_liked_for_tidal_artist(conn, id, &page.videos)?;
        Ok(summary)
    })?;
    let fetched = offset + received;
    let more = received > 0 && page.total.is_some_and(|total| total > fetched);
    let follow_up = (more && job.class >= JobClass::Priority).then(|| Job {
        artist_id: id,
        kind: JobKind::Page { offset: fetched },
        class: job.class,
        value: job.value,
    });
    Ok(JobReport {
        calls: 1,
        new_videos: summary.new_videos,
        empty: offset == 0 && received == 0,
        follow_up,
        ..Default::default()
    })
}

async fn resolve_video<S: DiscoverySource>(
    db: &Database,
    src: &S,
    video_id: i64,
) -> Result<JobReport> {
    let video = src.video(video_id).await?;
    let summary = db.with_conn(|conn| harvest::ingest(conn, &[video], HarvestContext::Search))?;
    Ok(JobReport {
        calls: 1,
        new_videos: summary.new_videos,
        ..Default::default()
    })
}

async fn genre_search<S: DiscoverySource>(
    db: &Database,
    src: &S,
    genre: &str,
) -> Result<JobReport> {
    db.with_conn(|conn| video_radio::mark_genre_scanned(conn, genre))?;
    let found = src.search_videos(&format!("{genre} music video")).await?;
    let summary = db.with_conn(|conn| harvest::ingest(conn, &found, HarvestContext::Search))?;
    Ok(JobReport {
        calls: 1,
        new_videos: summary.new_videos,
        ..Default::default()
    })
}

async fn harvest_mixes<S: DiscoverySource>(db: &Database, src: &S) -> Result<JobReport> {
    db.with_conn(|conn| set_marker(conn, MIXES_KEY))?;
    let ids = src.video_mix_ids().await?;
    let mut report = JobReport {
        calls: 1,
        ..Default::default()
    };
    for id in ids.into_iter().take(MAX_MIXES) {
        report.calls += 1;
        if let Ok(videos) = src.mix_videos(&id).await {
            let key = format!("mix:{id}");
            report.new_videos += db
                .with_conn(|conn| {
                    harvest::ingest(conn, &videos, HarvestContext::List { key: &key })
                })?
                .new_videos;
        }
    }
    Ok(report)
}

async fn harvest_editorial<S: DiscoverySource>(
    db: &Database,
    src: &S,
    playlists: bool,
) -> Result<JobReport> {
    db.with_conn(|conn| {
        set_marker(conn, EDITORIAL_KEY)?;
        if playlists {
            set_marker(conn, EDITORIAL_FULL_KEY)?;
        }
        Ok(())
    })?;
    let modules = src.editorial_modules().await?;
    let mut report = JobReport {
        calls: 1,
        ..Default::default()
    };
    let mut playlist_ids = Vec::new();
    for module in &modules {
        let key = format!("page:videos:{}", module.key);
        report.new_videos += db
            .with_conn(|conn| {
                harvest::ingest(conn, &module.videos, HarvestContext::List { key: &key })
            })?
            .new_videos;
        playlist_ids.extend(module.playlist_ids.iter().cloned());
    }
    if playlists {
        playlist_ids.sort();
        playlist_ids.dedup();
        for uuid in playlist_ids.into_iter().take(MAX_EDITORIAL_PLAYLISTS) {
            report.calls += 1;
            if let Ok(videos) = src.playlist_videos(&uuid).await {
                let key = format!("playlist:{uuid}");
                report.new_videos += db
                    .with_conn(|conn| {
                        harvest::ingest(conn, &videos, HarvestContext::List { key: &key })
                    })?
                    .new_videos;
            }
        }
    }
    Ok(report)
}

const ARTIST_MIX_KEY: &str = "video_discovery.artist_mix";
const MIX_TRIALS: usize = 5;

/// Verdict stored as `on`, `off`, or `trial:<tries>`.
pub fn mix_policy(conn: &Connection) -> Result<MixPolicy> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [ARTIST_MIX_KEY],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match value.as_deref() {
        Some("on") => MixPolicy::On,
        Some("off") => MixPolicy::Off,
        Some(other) => {
            let tries = other
                .strip_prefix("trial:")
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(0);
            MixPolicy::Trial {
                remaining: MIX_TRIALS.saturating_sub(tries),
            }
        }
        None => MixPolicy::Trial {
            remaining: MIX_TRIALS,
        },
    })
}

pub fn record_mix_trial(conn: &Connection, found_videos: bool) -> Result<()> {
    let verdict = match mix_policy(conn)? {
        MixPolicy::On | MixPolicy::Off => return Ok(()),
        MixPolicy::Trial { .. } if found_videos => "on".to_string(),
        MixPolicy::Trial { remaining } => {
            let tries = MIX_TRIALS - remaining + 1;
            if tries >= MIX_TRIALS {
                "off".to_string()
            } else {
                format!("trial:{tries}")
            }
        }
    };
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![ARTIST_MIX_KEY, verdict],
    )?;
    Ok(())
}

async fn artist_mix<S: DiscoverySource>(
    db: &Database,
    src: &S,
    artist_id: i64,
    mix_id: &str,
) -> Result<JobReport> {
    let videos = src.mix_videos(mix_id).await?;
    let key = format!("artistmix:{mix_id}");
    let summary = db.with_conn(|conn| {
        artist_state::record_mix_check(conn, artist_id)?;
        record_mix_trial(conn, !videos.is_empty())?;
        harvest::ingest(conn, &videos, HarvestContext::List { key: &key })
    })?;
    Ok(JobReport {
        calls: 1,
        new_videos: summary.new_videos,
        empty: videos.is_empty(),
        ..Default::default()
    })
}

// --- Markers and scheduled harvests --------------------------------------

fn marker_age_hours(conn: &Connection, key: &str) -> Result<Option<f64>> {
    Ok(conn
        .query_row(
            "SELECT (julianday('now') - julianday(value)) * 24 FROM server_config WHERE key = ?1",
            [key],
            |row| row.get::<_, Option<f64>>(0),
        )
        .optional()?
        .flatten())
}

fn set_marker(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [key],
    )?;
    Ok(())
}

pub fn scheduled_jobs(conn: &Connection) -> Result<Vec<Job>> {
    let due = |key: &str, hours: f64| -> Result<bool> {
        Ok(marker_age_hours(conn, key)?.is_none_or(|age| age >= hours))
    };
    let job = |kind| Job {
        artist_id: 0,
        kind,
        class: JobClass::Priority,
        value: 1.0,
    };
    let mut jobs = Vec::new();
    if due(MIXES_KEY, 24.0)? {
        jobs.push(job(JobKind::HarvestMixes));
    }
    if due(EDITORIAL_FULL_KEY, 168.0)? {
        jobs.push(job(JobKind::HarvestEditorial { playlists: true }));
    } else if due(EDITORIAL_KEY, 24.0)? {
        jobs.push(job(JobKind::HarvestEditorial { playlists: false }));
    }
    Ok(jobs)
}

// --- Planning -------------------------------------------------------------

pub fn build_plan(conn: &Connection, limit: usize) -> Result<Vec<Job>> {
    let graph = graph::cached(conn)?;
    let liked = roots::liked_roots(conn)?;
    let liked_ids: HashSet<i64> = liked.iter().map(|r| r.artist_id).collect();
    let enjoyed = roots::enjoyed_roots(conn, &liked_ids)?;
    let deep_roots: Vec<roots::Root> = enjoyed
        .iter()
        .chain(roots::station_roots().iter())
        .copied()
        .collect();
    let pairs = |roots: &[roots::Root]| {
        roots
            .iter()
            .map(|r| (r.artist_id, r.weight))
            .collect::<Vec<_>>()
    };
    let mut relevance = graph.propagate(&pairs(&liked), 2);
    let deep = graph.propagate(&pairs(&deep_roots), 3);
    for (id, rel) in &deep {
        relevance
            .entry(*id)
            .and_modify(|existing| {
                existing.score = existing.score.max(rel.score);
                existing.hops = existing.hops.min(rel.hops);
            })
            .or_insert(*rel);
    }
    let root_ids: HashSet<i64> = liked
        .iter()
        .chain(deep_roots.iter())
        .map(|r| r.artist_id)
        .collect();
    let states = artist_state::load_all(conn)?;
    let calibration = Calibration::load(conn)?;
    let mut jobs = scheduled_jobs(conn)?;
    jobs.extend(scheduler::plan_with_mixes(
        &PlanInput {
            relevance: &relevance,
            deep: &deep,
            states: &states,
            roots: &root_ids,
            priority: &root_ids,
            calibration: &calibration,
        },
        mix_policy(conn)?,
        limit,
    ));
    Ok(jobs)
}

pub fn build_station_plan(conn: &Connection, seed: i64) -> Result<Vec<Job>> {
    let graph = graph::refresh_if_dirty(conn)?;
    let relevance = graph.propagate(&[(seed, 1.0)], 3);
    let states = artist_state::load_all(conn)?;
    let calibration = Calibration::load(conn)?;
    let mut jobs =
        scheduler::station_jobs(seed, &relevance, &states, &calibration, STATION_JOB_LIMIT);
    if jobs.len() < STATION_JOB_LIMIT
        && let Some(genre) = video_radio::seed_genre(conn, seed)?
        && video_radio::genre_due(conn, &genre)?
    {
        jobs.push(Job {
            artist_id: seed,
            kind: JobKind::GenreSearch { genre },
            class: JobClass::Urgent,
            value: 0.5,
        });
    }
    Ok(jobs)
}

/// Run blocking ledger work on its own connection; in-memory test databases
/// fall back to the shared one.
async fn blocking<T, F>(db: &Database, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T> + Send + 'static,
{
    let db = db.clone();
    tokio::task::spawn_blocking(move || match db.open_isolated() {
        Ok(conn) => f(&conn),
        Err(_) => db.with_conn(f),
    })
    .await?
}

// --- Bootstrap ------------------------------------------------------------

/// One pass over cached TIDAL search payloads: their artist objects carry
/// popularity and mix ids for tens of thousands of artists.
pub fn backfill_popularity(conn: &Connection) -> Result<usize> {
    if marker_age_hours(conn, POPULARITY_BACKFILL_KEY)?.is_some() {
        return Ok(0);
    }
    let mut found: HashMap<i64, (String, Option<i32>, Option<String>)> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT payload FROM tidal_search_cache")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let payload: String = row.get(0)?;
            let Ok(value) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            let Some(artists) = value.get("artists").and_then(Value::as_array) else {
                continue;
            };
            for artist in artists {
                let (Some(id), Some(name)) = (
                    artist.get("id").and_then(Value::as_i64),
                    artist.get("name").and_then(Value::as_str),
                ) else {
                    continue;
                };
                let extra: HashMap<String, Value> = artist
                    .get("extra")
                    .and_then(Value::as_object)
                    .map(|map| map.clone().into_iter().collect())
                    .unwrap_or_default();
                let (popularity, mix_id) = harvest::artist_facts(&extra);
                let entry = found
                    .entry(id)
                    .or_insert_with(|| (name.to_string(), None, None));
                entry.1 = entry.1.max(popularity);
                if entry.2.is_none() {
                    entry.2 = mix_id;
                }
            }
        }
    }
    let tx = conn.unchecked_transaction()?;
    for (id, (name, popularity, mix_id)) in &found {
        artist_state::upsert_identity(&tx, *id, name, *popularity, mix_id.as_deref())?;
    }
    set_marker(&tx, POPULARITY_BACKFILL_KEY)?;
    tx.commit()?;
    Ok(found.len())
}

async fn bootstrap(db: &Database) {
    let result = blocking(db, |conn| {
        let keyed = names::bootstrap_library_names(conn)?;
        let backfilled = backfill_popularity(conn)?;
        Ok((keyed, backfilled))
    })
    .await;
    match result {
        Ok((keyed, backfilled)) => {
            tracing::info!(target: "noor.video_discovery", keyed, backfilled, "video discovery bootstrap complete")
        }
        Err(error) => {
            tracing::warn!(target: "noor.video_discovery", %error, "video discovery bootstrap failed")
        }
    }
}

// --- Loop -----------------------------------------------------------------

fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

async fn nap(duration: Duration) {
    tokio::select! {
        _ = tokio::time::sleep(duration) => {}
        _ = wake_signal().notified() => {}
    }
}

fn drain_urgent() {
    while let Some(urgent) = next_urgent() {
        finish(urgent);
    }
}

async fn live_source(state: &SharedState) -> Option<LiveSource> {
    let (tokens, tidal_http, http, db) = {
        let s = state.read().await;
        (
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
            s.http_client.clone(),
            s.db.clone(),
        )
    };
    let tokens = tokens?;
    let tidal = TidalClient::with_http(tidal_http, tokens.access_token, tokens.country_code)
        .for_background_work();
    Some(LiveSource::new(tidal, LastFmClient::load(http, &db)))
}

async fn run_governed<S: DiscoverySource>(
    db: &Database,
    src: &S,
    governor: &mut Governor,
    mode: Mode,
    job: &Job,
    urgent: bool,
) -> JobReport {
    if let Some(wait) = governor.wait(Instant::now(), today(), mode, job.estimated_calls(), urgent)
    {
        tokio::time::sleep(wait).await;
    }
    let report = execute(db, src, job).await;
    governor.record(
        Instant::now(),
        today(),
        mode,
        report.calls.max(1),
        rand::random::<f64>(),
    );
    publish_budget(governor, mode);
    record_stats(job, &report);
    report
}

async fn run_urgent<S: DiscoverySource>(
    db: &Database,
    src: &S,
    governor: &mut Governor,
    mode: Mode,
    urgent: Urgent,
) {
    match urgent {
        Urgent::Video(video_id) => {
            let job = Job {
                artist_id: 0,
                kind: JobKind::ResolveVideo { video_id },
                class: JobClass::Urgent,
                value: 1.0,
            };
            run_governed(db, src, governor, mode, &job, true).await;
        }
        Urgent::Station(seed) => {
            // Round two sees the neighbors round one's expansion discovered.
            for round in 0..2 {
                let jobs = blocking(db, move |conn| build_station_plan(conn, seed))
                    .await
                    .unwrap_or_default();
                let jobs: Vec<Job> = if round == 0 {
                    jobs
                } else {
                    jobs.into_iter()
                        .filter(|job| job.kind == JobKind::Probe)
                        .collect()
                };
                if jobs.is_empty() {
                    break;
                }
                for job in &jobs {
                    let report = run_governed(db, src, governor, mode, job, true).await;
                    if report.auth_failed {
                        finish(urgent);
                        return;
                    }
                }
            }
        }
    }
    finish(urgent);
}

pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        run(state).await;
    });
}

async fn run(state: SharedState) {
    let (db, audio) = {
        let s = state.read().await;
        (s.db.clone(), s.audio_active.clone())
    };
    bootstrap(&db).await;
    let mut governor = Governor::new();
    let mut queue: VecDeque<Job> = VecDeque::new();
    let mut planned_at: Option<Instant> = None;
    let mut summary_at = Instant::now();
    loop {
        if summary_at.elapsed() >= Duration::from_secs(3600) {
            log_hour_summary();
            summary_at = Instant::now();
        }
        let Some(src) = live_source(&state).await else {
            drain_urgent();
            nap(Duration::from_secs(60)).await;
            continue;
        };
        if crate::services::tidal::backoff::global().check().is_err() {
            nap(Duration::from_secs(30)).await;
            continue;
        }
        let mode = governor::current_mode(audio.load(Ordering::Relaxed));
        if let Some(urgent) = next_urgent() {
            run_urgent(&db, &src, &mut governor, mode, urgent).await;
            continue;
        }
        if queue.is_empty() || planned_at.is_none_or(|at| at.elapsed() >= REPLAN_EVERY) {
            queue = blocking(&db, |conn| build_plan(conn, PLAN_BATCH))
                .await
                .unwrap_or_else(|error| {
                    tracing::warn!(target: "noor.video_discovery", %error, "planning failed");
                    Vec::new()
                })
                .into();
            planned_at = Some(Instant::now());
            if queue.is_empty() {
                nap(IDLE_NAP).await;
                continue;
            }
        }
        let Some(job) = queue.front().cloned() else {
            continue;
        };
        if let Some(wait) =
            governor.wait(Instant::now(), today(), mode, job.estimated_calls(), false)
        {
            nap(wait).await;
            continue;
        }
        queue.pop_front();
        let report = run_governed(&db, &src, &mut governor, mode, &job, false).await;
        if report.auth_failed {
            queue.clear();
            nap(AUTH_PAUSE).await;
            continue;
        }
        if let Some(next) = report.follow_up {
            queue.push_front(next);
        }
    }
}

// --- Status ---------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DiscoveryStatus {
    pub mode: String,
    pub calls_last_hour: usize,
    pub calls_today: usize,
    pub artists_known: i64,
    pub artists_checked: i64,
    pub artists_with_videos: i64,
    pub catalog_videos: i64,
    pub checked_last_day: i64,
    pub probe_yield_last_day: Option<f64>,
    pub artist_mix: String,
    pub hour: HourStats,
}

pub fn status(conn: &Connection) -> Result<DiscoveryStatus> {
    let (known, checked, with_videos, checked_day, found_day): (i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                SUM(last_checked_at IS NOT NULL),
                SUM(seen_main = 1 OR fetched_count > 0),
                SUM(last_checked_at > datetime('now', '-1 day')),
                SUM(last_checked_at > datetime('now', '-1 day') AND (seen_main = 1 OR fetched_count > 0))
           FROM video_artist_state",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
            ))
        },
    )?;
    let catalog: i64 =
        conn.query_row("SELECT COUNT(*) FROM video_catalog", [], |row| row.get(0))?;
    let (calls_hour, calls_day, mode) = BUDGET.lock().map(|b| *b).unwrap_or((0, 0, "idle"));
    let hour = STATS
        .lock()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_default();
    Ok(DiscoveryStatus {
        mode: mode.to_string(),
        calls_last_hour: calls_hour,
        calls_today: calls_day,
        artists_known: known,
        artists_checked: checked,
        artists_with_videos: with_videos,
        catalog_videos: catalog,
        checked_last_day: checked_day,
        artist_mix: format!("{:?}", mix_policy(conn)?),
        probe_yield_last_day: (checked_day > 0).then(|| found_day as f64 / checked_day as f64),
        hour,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_discovery::source::fake::{FakeSource, artist_video, candidate};

    fn db() -> Database {
        let db = Database::open_in_memory().unwrap();
        db.run_migrations().unwrap();
        db
    }

    fn job(artist_id: i64, kind: JobKind, class: JobClass) -> Job {
        Job {
            artist_id,
            kind,
            class,
            value: 0.5,
        }
    }

    #[tokio::test]
    async fn a_priority_probe_pages_the_whole_library() {
        let db = db();
        let videos = (1..=60)
            .map(|i| artist_video(1000 + i, 10, "Ten", &format!("Song {i}")))
            .collect();
        let src = FakeSource {
            artist_videos: HashMap::from([(10, videos)]),
            ..Default::default()
        };
        let first = execute(&db, &src, &job(10, JobKind::Probe, JobClass::Priority)).await;
        assert_eq!(first.new_videos, 50);
        let next = first.follow_up.expect("a second page");
        assert_eq!(next.kind, JobKind::Page { offset: 50 });
        let second = execute(&db, &src, &next).await;
        assert_eq!(second.new_videos, 10);
        assert!(second.follow_up.is_none());
        let state = db
            .with_conn(|conn| artist_state::get(conn, 10))
            .unwrap()
            .unwrap();
        assert_eq!((state.fetched_count, state.total_videos), (60, Some(60)));
    }

    #[tokio::test]
    async fn an_ordinary_probe_does_not_chase_pages() {
        let db = db();
        let videos = (1..=60)
            .map(|i| artist_video(2000 + i, 11, "Eleven", &format!("Song {i}")))
            .collect();
        let src = FakeSource {
            artist_videos: HashMap::from([(11, videos)]),
            ..Default::default()
        };
        let report = execute(&db, &src, &job(11, JobKind::Probe, JobClass::Normal)).await;
        assert!(report.follow_up.is_none());
    }

    #[tokio::test]
    async fn empty_and_failed_probes_are_recorded_differently() {
        let db = db();
        let src = FakeSource {
            failing_artists: HashSet::from([21]),
            ..Default::default()
        };
        let empty = execute(&db, &src, &job(20, JobKind::Probe, JobClass::Normal)).await;
        assert!(empty.empty && !empty.failed);
        let failed = execute(&db, &src, &job(21, JobKind::Probe, JobClass::Normal)).await;
        assert!(failed.failed && !failed.auth_failed);
        let (empty_state, failed_state) = db
            .with_conn(|conn| {
                Ok((
                    artist_state::get(conn, 20)?.unwrap(),
                    artist_state::get(conn, 21)?.unwrap(),
                ))
            })
            .unwrap();
        assert_eq!(empty_state.empty_streak, 1);
        assert!(!empty_state.never_checked());
        assert!(failed_state.never_checked());
    }

    #[test]
    fn the_plan_reaches_popular_neighbors_before_obscure_ones() {
        let db = db();
        db.with_conn(|conn| {
            conn.execute_batch(
                "INSERT INTO artists (id, tidal_id, name) VALUES (1, 100, 'Liked');
                 INSERT INTO tracks (id, artist_id, title, is_favorite) VALUES (1, 1, 'Song', 1);
                 INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, rank, weight)
                     VALUES (100, 200, 'Popular', 'tidal', 0, 0.9), (100, 300, 'Obscure', 'tidal', 1, 0.9);",
            )?;
            artist_state::upsert_identity(conn, 200, "Popular", Some(85), None)?;
            artist_state::upsert_identity(conn, 300, "Obscure", Some(20), None)?;
            set_marker(conn, MIXES_KEY)?;
            set_marker(conn, EDITORIAL_KEY)?;
            set_marker(conn, EDITORIAL_FULL_KEY)
        })
        .unwrap();
        let jobs = db.with_conn(|conn| build_plan(conn, 20)).unwrap();
        assert_eq!(
            jobs[0].class,
            JobClass::Priority,
            "the liked root's own work leads"
        );
        let probes: Vec<i64> = jobs
            .iter()
            .filter(|j| j.kind == JobKind::Probe && j.class == JobClass::Normal)
            .map(|j| j.artist_id)
            .collect();
        assert_eq!(probes, vec![200, 300]);
    }

    #[test]
    fn urgent_requests_build_once_until_finished() {
        let mut queue = UrgentQueue::new();
        assert!(queue.request(Urgent::Station(5)));
        assert!(!queue.request(Urgent::Station(5)));
        assert!(queue.is_building(Urgent::Station(5)));
        assert_eq!(queue.next(), Some(Urgent::Station(5)));
        assert_eq!(queue.next(), None);
        assert!(
            queue.is_building(Urgent::Station(5)),
            "still building while it runs"
        );
        queue.finish(Urgent::Station(5));
        assert!(!queue.is_building(Urgent::Station(5)));
    }

    #[test]
    fn a_new_station_seed_is_expanded_and_probed_first() {
        let db = db();
        let jobs = db.with_conn(|conn| build_station_plan(conn, 4040)).unwrap();
        assert_eq!(jobs[0].kind, JobKind::Expand);
        assert_eq!(jobs[1].kind, JobKind::Probe);
        assert!(jobs.iter().all(|j| j.class == JobClass::Urgent));
    }

    #[test]
    fn scheduled_harvests_wait_for_their_markers() {
        let db = db();
        db.with_conn(|conn| {
            let kinds: Vec<JobKind> = scheduled_jobs(conn)?.into_iter().map(|j| j.kind).collect();
            assert_eq!(
                kinds,
                vec![
                    JobKind::HarvestMixes,
                    JobKind::HarvestEditorial { playlists: true }
                ]
            );
            set_marker(conn, MIXES_KEY)?;
            set_marker(conn, EDITORIAL_KEY)?;
            set_marker(conn, EDITORIAL_FULL_KEY)?;
            assert!(scheduled_jobs(conn)?.is_empty());
            conn.execute(
                "UPDATE server_config SET value = datetime('now', '-25 hours') WHERE key = ?1",
                [EDITORIAL_KEY],
            )?;
            let kinds: Vec<JobKind> = scheduled_jobs(conn)?.into_iter().map(|j| j.kind).collect();
            assert_eq!(kinds, vec![JobKind::HarvestEditorial { playlists: false }]);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn popularity_backfills_once_from_cached_searches() {
        let db = db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO tidal_search_cache (query_hash, payload, fetched_at) VALUES ('h', ?1, 0)",
                [r#"{"artists":[{"id":6428079,"name":"Victoria Monet","extra":{"popularity":77,"mixes":{"ARTIST_MIX":"m1"}}}]}"#],
            )?;
            assert_eq!(backfill_popularity(conn)?, 1);
            assert_eq!(backfill_popularity(conn)?, 0);
            let state = artist_state::get(conn, 6428079)?.unwrap();
            assert_eq!((state.popularity, state.mix_id.as_deref()), (Some(77), Some("m1")));
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn video_mixes_link_artists_listed_together() {
        let db = db();
        let src = FakeSource {
            mixes: HashMap::from([("m1".to_string(), vec![candidate(1, 501), candidate(2, 502)])]),
            ..Default::default()
        };
        let report = execute(
            &db,
            &src,
            &job(0, JobKind::HarvestMixes, JobClass::Priority),
        )
        .await;
        assert_eq!(report.new_videos, 2);
        let linked: bool = db
            .with_conn(|conn| {
                Ok(conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM video_related_artists
                      WHERE seed_tidal_id = 501 AND related_tidal_id = 502 AND source = 'colist')",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert!(linked);
    }

    #[test]
    fn the_mix_verdict_turns_on_at_the_first_hit_and_off_after_five_misses() {
        let db = db();
        db.with_conn(|conn| {
            assert_eq!(mix_policy(conn)?, MixPolicy::Trial { remaining: 5 });
            for _ in 0..4 {
                record_mix_trial(conn, false)?;
            }
            assert_eq!(mix_policy(conn)?, MixPolicy::Trial { remaining: 1 });
            record_mix_trial(conn, false)?;
            assert_eq!(mix_policy(conn)?, MixPolicy::Off);
            conn.execute("DELETE FROM server_config WHERE key = ?1", [ARTIST_MIX_KEY])?;
            record_mix_trial(conn, true)?;
            assert_eq!(mix_policy(conn)?, MixPolicy::On);
            Ok(())
        })
        .unwrap();
    }
}
