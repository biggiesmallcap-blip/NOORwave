//! The dial: fixed channels 1-8 and one numbered channel per scene that has
//! qualified. A channel is a filter over the universe plus a lean; this
//! module turns it into a programmer `Pool`, writes its copy, and decides
//! whether it is on air or locked (and how close it is to unlocking).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::ExploreMode;
use super::clock;
use super::programmer::{Bucket, Flow, Pool, SPACING, Unit};
use super::taste::Taste;
use super::universe::{Cut, MAX_PROGRAM_S, PREMIERE_YEARS, Universe, VideoRow};
use crate::services::video_sets::build_seed;
use crate::services::video_stations::pick::{MID, NEAR};

/// Fixed channels need this many programmable hours to go on air.
pub const FIXED_NEED_HOURS: f64 = 20.0;
/// Scene channels: 30 programmable hours and 40 artists.
pub const SCENE_NEED_HOURS: f64 = 30.0;
pub const SCENE_NEED_ARTISTS: usize = 40;
/// Unnumbered scenes listed as "coming up" after the scene channels.
const COMING_UP: usize = 3;
/// Rewind waits until half the programmable songs know their year.
pub const ERA_NEED_COVERAGE: f64 = 0.5;
const UNITLESS_WEIGHT: f64 = 1e-6;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ChannelId {
    Your,
    Wild,
    Premieres,
    Live,
    Sessions,
    Concerts,
    Rewind,
    Spotlight,
    Scene(i64),
}

impl ChannelId {
    pub const FIXED: [ChannelId; 8] = [
        ChannelId::Your,
        ChannelId::Wild,
        ChannelId::Premieres,
        ChannelId::Live,
        ChannelId::Sessions,
        ChannelId::Concerts,
        ChannelId::Rewind,
        ChannelId::Spotlight,
    ];

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "your" => Some(Self::Your),
            "wild" => Some(Self::Wild),
            "premieres" => Some(Self::Premieres),
            "live" => Some(Self::Live),
            "sessions" => Some(Self::Sessions),
            "concerts" => Some(Self::Concerts),
            "rewind" => Some(Self::Rewind),
            "spotlight" => Some(Self::Spotlight),
            _ => raw
                .strip_prefix("scene:")
                .and_then(|id| id.parse::<i64>().ok())
                .filter(|id| *id >= 0)
                .map(Self::Scene),
        }
    }

    pub fn as_string(&self) -> String {
        match self {
            Self::Your => "your".into(),
            Self::Wild => "wild".into(),
            Self::Premieres => "premieres".into(),
            Self::Live => "live".into(),
            Self::Sessions => "sessions".into(),
            Self::Concerts => "concerts".into(),
            Self::Rewind => "rewind".into(),
            Self::Spotlight => "spotlight".into(),
            Self::Scene(id) => format!("scene:{id}"),
        }
    }

    fn fixed_no(&self) -> Option<i64> {
        Self::FIXED
            .iter()
            .position(|fixed| fixed == self)
            .map(|index| index as i64 + 1)
    }

    /// Channels that only exist once the intelligence build knows cuts,
    /// concerts or eras.
    fn needs_facets(&self) -> bool {
        matches!(
            self,
            Self::Live | Self::Sessions | Self::Concerts | Self::Rewind
        )
    }

    fn about_cut(&self) -> Option<Cut> {
        match self {
            Self::Live => Some(Cut::Live),
            Self::Sessions => Some(Cut::Session),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Unlock {
    pub hours: f64,
    pub need_hours: f64,
    /// Scenes also need enough artists; 0 needed where only hours count.
    #[serde(default)]
    pub artists: usize,
    #[serde(default)]
    pub need_artists: usize,
}

impl Unlock {
    fn hours(hours: f64, need_hours: f64) -> Self {
        Self {
            hours,
            need_hours,
            artists: 0,
            need_artists: 0,
        }
    }
}

/// A channel as the dial lists it (VideoChannel without now/next).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMeta {
    pub id: String,
    pub no: i64,
    pub title: String,
    pub subtitle: String,
    pub hue: Option<f64>,
    pub scene_id: Option<i64>,
    pub locked: bool,
    pub unlock: Option<Unlock>,
    /// Scene appetite (0.05..1.1) for ordering the "for you" strip; null on
    /// the fixed channels. Never changes the numbers.
    pub appetite: Option<f64>,
    /// The scene's family on the dial (hiphop, latin, rock, pop, country,
    /// soul, world, other); null on the fixed channels.
    #[serde(default)]
    pub family: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SpotlightPick {
    pub artist: i64,
    pub name: String,
}

/// What a day's build decided once for every channel.
#[derive(Debug, Clone, Default)]
pub struct DayContext {
    pub explore: ExploreMode,
    pub spotlight: Option<SpotlightPick>,
    pub rewind_decade: Option<i32>,
}

/// Session shaping for refills: multipliers per artist and scene, and videos
/// the session zapped. Empty for the daily build.
#[derive(Debug, Clone, Default)]
pub struct Boosts {
    pub artist: HashMap<i64, f64>,
    pub scene: HashMap<i64, f64>,
    pub skip_videos: HashSet<i64>,
}

impl Boosts {
    fn of(&self, artist: i64, scene: Option<i64>) -> f64 {
        let artist = self.artist.get(&artist).copied().unwrap_or(1.0);
        let scene = scene
            .and_then(|scene| self.scene.get(&scene).copied())
            .unwrap_or(1.0);
        artist * scene
    }
}

pub struct Planned {
    pub meta: ChannelMeta,
    pub pool: Option<Pool>,
}

pub struct Built {
    pub pool: Pool,
    /// Programmable hours the channel admits (every cut, before one per song).
    pub hours: f64,
    pub artists: usize,
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
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

/// Non-scene channels lean toward scenes the listener has appetite for.
fn general_fit(scene: Option<i64>, taste: &Taste) -> f64 {
    if taste.appetite.is_empty() {
        return 1.0;
    }
    match scene.and_then(|scene| taste.appetite.get(&scene)) {
        Some(appetite) => 0.5 + 0.5 * appetite.min(1.0),
        None => 0.6,
    }
}

/// Long-run zaps and holds on an artist; neutral without history.
fn history_fit(taste: &Taste, artist: i64) -> f64 {
    let Some(stats) = taste.artists.get(&artist) else {
        return 1.0;
    };
    let holds = f64::from(stats.holds.min(4));
    let zaps = f64::from(stats.zaps.min(8));
    ((1.0 + 0.15 * holds) / (1.0 + 0.25 * zaps)).clamp(0.4, 1.6)
}

fn freshness(taste: &Taste, video: i64) -> f64 {
    match taste.videos.get(&video) {
        Some(stats) if stats.plays > 0 => match stats.last_age_days {
            d if d < 3.0 => 0.2,
            d if d < 14.0 => 0.4,
            d if d < 60.0 => 0.7,
            _ => 0.9,
        },
        _ => 1.0,
    }
}

/// None when the video was zapped twice: it never airs again.
fn skip_damp(taste: &Taste, video: i64) -> Option<f64> {
    match taste.videos.get(&video).map_or(0, |stats| stats.zaps) {
        0 => Some(1.0),
        1 => Some(0.35),
        _ => None,
    }
}

fn hold_rate(taste: &Taste, video: i64) -> f64 {
    let holds = taste.videos.get(&video).map_or(0, |stats| stats.holds);
    1.0 + 0.25 * f64::from(holds.min(2))
}

fn plays_label(plays: i64) -> String {
    if plays == 1 {
        "1 play".into()
    } else {
        format!("{plays} plays")
    }
}

/// One reason chip, by priority: concert > near a root > scene > cut > era >
/// new. Chips that only restate the channel are skipped.
pub fn why(id: &ChannelId, video: &VideoRow, universe: &Universe, taste: &Taste) -> String {
    if let Some(concert) = video.concert
        && let Ok(index) = universe.concerts.binary_search_by_key(&concert, |c| c.id)
        && !universe.concerts[index].label.is_empty()
    {
        return universe.concerts[index].label.clone();
    }
    if let Some(root) = taste.roots.get(&video.artist) {
        if root.library {
            return if root.plays > 0 {
                format!("Your library, {}", plays_label(root.plays))
            } else {
                "Your library".into()
            };
        }
        if root.saved {
            return "Saved".into();
        }
        if root.enjoyed {
            return "Watched before".into();
        }
        if root.plays > 0 {
            return format!("{} in your history", plays_label(root.plays));
        }
    } else if taste.hops_of(video.artist) == Some(1)
        && let Some(via) = taste.via.get(&video.artist)
        && let Some(name) = taste.names.get(via)
    {
        let plays = taste.roots.get(via).map_or(0, |root| root.plays);
        return if plays > 0 {
            format!("Near {name}, {}", plays_label(plays))
        } else {
            format!("Near {name}")
        };
    }
    if let Some(scene) = video.scene
        && *id != ChannelId::Scene(scene)
        && let Some(row) = universe.scene(scene)
    {
        return row.name.clone();
    }
    if id.about_cut() != Some(video.cut)
        && let Some(chip) = video.cut.chip()
    {
        return chip.into();
    }
    if let Some(year) = universe.song_year(video) {
        return year.to_string();
    }
    if video
        .cut_year
        .is_some_and(|year| year >= universe.this_year)
    {
        return "New".into();
    }
    String::new()
}

/// The chip for a scheduled video; songs get theirs only once picked.
pub fn chip(id: &ChannelId, video_id: i64, universe: &Universe, taste: &Taste) -> String {
    universe
        .video(video_id)
        .map(|video| why(id, video, universe, taste))
        .unwrap_or_default()
}

fn program_ok(video: &VideoRow) -> bool {
    video.cut != Cut::Longform && video.duration_s <= MAX_PROGRAM_S
}

/// Whether a channel airs this video, and how well it fits.
fn admits(
    id: &ChannelId,
    video: &VideoRow,
    universe: &Universe,
    taste: &Taste,
    ctx: &DayContext,
) -> Option<f64> {
    if !program_ok(video) {
        return None;
    }
    let general = || general_fit(video.scene, taste);
    match id {
        ChannelId::Your => taste
            .hops_of(video.artist)
            .is_some_and(|hops| hops <= 1)
            .then(general),
        ChannelId::Wild => Some(general()),
        ChannelId::Premieres => (video.fresh
            && video
                .cut_year
                .is_some_and(|year| year >= universe.this_year - PREMIERE_YEARS))
        .then(general),
        ChannelId::Live => (video.cut == Cut::Live).then(general),
        ChannelId::Sessions => (video.cut == Cut::Session).then(general),
        ChannelId::Rewind => ctx
            .rewind_decade
            .zip(universe.song_year(video))
            .is_some_and(|(decade, year)| year >= decade && year < decade + 10)
            .then(general),
        ChannelId::Spotlight => ctx
            .spotlight
            .as_ref()
            .is_some_and(|pick| pick.artist == video.artist)
            .then_some(1.0),
        ChannelId::Concerts => None,
        ChannelId::Scene(scene) => {
            if video.scene == Some(*scene) {
                Some(1.0)
            } else if video.scene2 == Some(*scene) {
                Some(0.5)
            } else {
                None
            }
        }
    }
}

/// Heavy, medium and light rotation: the top fifth of a pool's songs by
/// quality air twice as often, the bottom two fifths half as often.
fn apply_rotation(buckets: &mut [Bucket], qualities: &[(usize, usize, f64)]) {
    if qualities.len() < 5 {
        return;
    }
    let mut sorted: Vec<f64> = qualities.iter().map(|(_, _, q)| *q).collect();
    sorted.sort_by(f64::total_cmp);
    let at = |share: f64| sorted[((sorted.len() as f64 * share) as usize).min(sorted.len() - 1)];
    let (heavy, medium) = (at(0.8), at(0.4));
    for (bucket, unit, quality) in qualities {
        let tier = if *quality >= heavy {
            2.0
        } else if *quality >= medium {
            1.0
        } else {
            0.5
        };
        buckets[*bucket].units[*unit].weight *= tier;
    }
}

fn shape(id: &ChannelId, ctx: &DayContext, taste: &Taste, buckets: Vec<Bucket>) -> Pool {
    let rings = match id {
        ChannelId::Your => ctx.explore.closer().rings(),
        ChannelId::Wild => ctx.explore.wider().rings(),
        _ => ctx.explore.rings(),
    };
    let mut appetite: Vec<(i64, f64)> = taste.appetite.iter().map(|(s, a)| (*s, *a)).collect();
    appetite.sort_by_key(|(scene, _)| *scene);
    Pool {
        buckets,
        rings,
        frontier: !matches!(
            id,
            ChannelId::Your | ChannelId::Spotlight | ChannelId::Concerts
        ),
        spacing: if *id == ChannelId::Spotlight {
            0
        } else {
            SPACING
        },
        appetite,
        flow: if *id == ChannelId::Spotlight {
            Flow::Cycle
        } else {
            Flow::Rotation
        },
        unique_per_day: *id == ChannelId::Concerts,
    }
}

fn bucket(
    artist: i64,
    scene: Option<i64>,
    fit: f64,
    units: Vec<Unit>,
    taste: &Taste,
    boosts: &Boosts,
) -> Bucket {
    let ring = ring_of(taste.relevance_of(artist));
    let weight = (units.len() as f64).sqrt()
        * fit
        * history_fit(taste, artist)
        * taste.steer_of(artist)
        * boosts.of(artist, scene);
    Bucket {
        artist,
        ring,
        weight: if weight.is_finite() {
            weight.max(UNITLESS_WEIGHT)
        } else {
            UNITLESS_WEIGHT
        },
        frontier: ring > 0
            && !taste.roots.contains_key(&artist)
            && !taste.watched_artists.contains(&artist),
        scene,
        units,
    }
}

fn candidate_indices(id: &ChannelId, universe: &Universe, ctx: &DayContext) -> Vec<usize> {
    match id {
        ChannelId::Scene(scene) => universe
            .scene_members
            .get(scene)
            .cloned()
            .unwrap_or_default(),
        ChannelId::Spotlight => {
            let Some(pick) = &ctx.spotlight else {
                return Vec::new();
            };
            let start = universe.videos.partition_point(|v| v.artist < pick.artist);
            let end = universe.videos.partition_point(|v| v.artist <= pick.artist);
            (start..end).collect()
        }
        _ => (0..universe.videos.len()).collect(),
    }
}

fn song_pool(
    id: &ChannelId,
    universe: &Universe,
    taste: &Taste,
    ctx: &DayContext,
    boosts: &Boosts,
) -> Built {
    let indices = candidate_indices(id, universe, ctx);
    let cut_channel = id.about_cut().is_some();
    let rank = |video: &VideoRow| {
        let preference = if cut_channel {
            1.0
        } else {
            video.cut.preference()
        };
        preference * (0.2 + video.quality)
    };
    let mut buckets: Vec<Bucket> = Vec::new();
    let mut qualities: Vec<(usize, usize, f64)> = Vec::new();
    let mut seconds = 0i64;
    // One artist at a time: admitted cuts sorted by song, then one unit per
    // song. Reused across artists so a 150k-video pool allocates little.
    let mut admitted: Vec<(u64, i64, usize, f64)> = Vec::new();
    let mut start = 0;
    while start < indices.len() {
        let artist = universe.videos[indices[start]].artist;
        let mut end = start;
        while end < indices.len() && universe.videos[indices[end]].artist == artist {
            end += 1;
        }
        admitted.clear();
        for index in &indices[start..end] {
            let video = &universe.videos[*index];
            let Some(fit) = admits(id, video, universe, taste, ctx) else {
                continue;
            };
            if boosts.skip_videos.contains(&video.id) || skip_damp(taste, video.id).is_none() {
                continue;
            }
            seconds += video.duration_s;
            admitted.push((video.song, video.id, *index, fit));
        }
        start = end;
        if admitted.is_empty() {
            continue;
        }
        admitted.sort_unstable_by_key(|(song, id, _, _)| (*song, *id));
        let mut units = Vec::new();
        let mut fit_max: f64 = 0.0;
        let mut scene = None;
        let at = buckets.len();
        for cuts in admitted.chunk_by(|a, b| a.0 == b.0) {
            let song = cuts[0].0;
            let best = universe.songs.get(&song).and_then(|row| row.best_video);
            let &(_, _, index, fit) = cuts
                .iter()
                .find(|(_, id, _, _)| Some(*id) == best)
                .or_else(|| {
                    cuts.iter().max_by(|a, b| {
                        rank(&universe.videos[a.2])
                            .total_cmp(&rank(&universe.videos[b.2]))
                            .then(b.1.cmp(&a.1))
                    })
                })
                .expect("a song has at least one cut");
            let video = &universe.videos[index];
            fit_max = fit_max.max(fit);
            scene = scene.or(video.scene);
            let preference = if cut_channel {
                1.0
            } else {
                video.cut.preference().max(0.05)
            };
            let weight = (0.2 + video.quality)
                * preference
                * freshness(taste, video.id)
                * skip_damp(taste, video.id).unwrap_or(1.0)
                * hold_rate(taste, video.id);
            qualities.push((at, units.len(), video.quality));
            units.push(Unit::song(video.id, video.duration_s, song, weight));
        }
        let scene = match id {
            ChannelId::Scene(own) => Some(*own),
            _ => scene,
        };
        buckets.push(bucket(artist, scene, fit_max, units, taste, boosts));
    }
    apply_rotation(&mut buckets, &qualities);
    let artists = buckets.len();
    Built {
        pool: shape(id, ctx, taste, buckets),
        hours: seconds as f64 / 3600.0,
        artists,
    }
}

pub fn concert_key(concert: i64) -> u64 {
    build_seed("concert", &concert.to_string())
}

fn concert_pool(universe: &Universe, taste: &Taste, ctx: &DayContext, boosts: &Boosts) -> Built {
    let mut by_artist: BTreeMap<i64, (Option<i64>, Vec<Unit>)> = BTreeMap::new();
    let mut seconds = 0i64;
    for concert in &universe.concerts {
        let members: Vec<&VideoRow> = concert
            .video_ids
            .iter()
            .filter_map(|id| universe.video(*id))
            .collect();
        if members.is_empty() {
            continue;
        }
        let total: i64 = members.iter().map(|v| v.duration_s).sum();
        seconds += total;
        let count = members.len() as f64;
        let quality = members.iter().map(|v| v.quality).sum::<f64>() / count;
        let fresh = members.iter().map(|v| freshness(taste, v.id)).sum::<f64>() / count;
        let label = if concert.label.is_empty() {
            "Concert".to_string()
        } else {
            concert.label.clone()
        };
        let entry = by_artist
            .entry(concert.artist)
            .or_insert((members[0].scene, Vec::new()));
        let setlist: Vec<(i64, i64)> = members.iter().map(|v| (v.id, v.duration_s)).collect();
        entry.1.extend(Unit::set(
            &setlist,
            concert_key(concert.id),
            (0.2 + quality) * fresh,
            label,
        ));
    }
    let buckets: Vec<Bucket> = by_artist
        .into_iter()
        .map(|(artist, (scene, units))| {
            bucket(
                artist,
                scene,
                general_fit(scene, taste),
                units,
                taste,
                boosts,
            )
        })
        .collect();
    let artists = buckets.len();
    Built {
        pool: shape(&ChannelId::Concerts, ctx, taste, buckets),
        hours: seconds as f64 / 3600.0,
        artists,
    }
}

pub fn build_pool(
    id: &ChannelId,
    universe: &Universe,
    taste: &Taste,
    ctx: &DayContext,
    boosts: &Boosts,
) -> Built {
    match id {
        ChannelId::Concerts => concert_pool(universe, taste, ctx, boosts),
        _ => song_pool(id, universe, taste, ctx, boosts),
    }
}

/// (songs with a year, songs, seconds of songs with a year, seconds) over
/// the programmable catalog.
fn era_stats(universe: &Universe) -> (usize, usize, i64, i64) {
    let mut songs: Vec<u64> = Vec::with_capacity(universe.videos.len());
    let (mut dated_s, mut total_s) = (0i64, 0i64);
    for video in universe.videos.iter().filter(|v| program_ok(v)) {
        total_s += video.duration_s;
        songs.push(video.song);
        if universe.song_year(video).is_some() {
            dated_s += video.duration_s;
        }
    }
    songs.sort_unstable();
    songs.dedup();
    let dated = songs
        .iter()
        .filter(|song| {
            universe
                .songs
                .get(song)
                .is_some_and(|row| row.year.is_some())
        })
        .count();
    (dated, songs.len(), dated_s, total_s)
}

fn decade_seconds(universe: &Universe) -> BTreeMap<i32, i64> {
    let mut decades: BTreeMap<i32, i64> = BTreeMap::new();
    for video in universe.videos.iter().filter(|v| program_ok(v)) {
        if let Some(year) = universe.song_year(video) {
            *decades.entry(year.div_euclid(10) * 10).or_default() += video.duration_s;
        }
    }
    decades
}

pub fn era_coverage(universe: &Universe) -> f64 {
    let (dated, songs, _, _) = era_stats(universe);
    if songs == 0 {
        0.0
    } else {
        dated as f64 / songs as f64
    }
}

/// The decade Rewind plays today: past decades with enough hours take turns
/// day by day. The decade we are in is never one of them: Rewind looks back,
/// and the current decade is what every other channel already plays. None
/// while era coverage is under half the pool.
pub fn rewind_decade(universe: &Universe, day: &str) -> Option<i32> {
    if !universe.facets || era_coverage(universe) < ERA_NEED_COVERAGE {
        return None;
    }
    let this_decade = day
        .get(..4)
        .and_then(|year| year.parse::<i32>().ok())
        .map_or(i32::MAX, |year| year.div_euclid(10) * 10);
    let decades: Vec<i32> = decade_seconds(universe)
        .into_iter()
        .filter(|(decade, _)| *decade < this_decade)
        .filter(|(_, seconds)| *seconds as f64 / 3600.0 >= FIXED_NEED_HOURS)
        .map(|(decade, _)| decade)
        .collect();
    if decades.is_empty() {
        return None;
    }
    let turn = clock::days_since_epoch(day)?.rem_euclid(decades.len() as i64);
    Some(decades[turn as usize])
}

fn rewind_progress(universe: &Universe) -> Unlock {
    let (dated, songs, dated_s, total_s) = era_stats(universe);
    if songs == 0 || (dated as f64) < ERA_NEED_COVERAGE * songs as f64 {
        return Unlock::hours(
            round1(dated_s as f64 / 3600.0),
            round1(ERA_NEED_COVERAGE * total_s as f64 / 3600.0),
        );
    }
    let best = decade_seconds(universe).into_values().max().unwrap_or(0);
    Unlock::hours(round1(best as f64 / 3600.0), FIXED_NEED_HOURS)
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

fn base_meta(id: &ChannelId, universe: &Universe, taste: &Taste, ctx: &DayContext) -> ChannelMeta {
    let fixed = |title: &str, subtitle: String| ChannelMeta {
        id: id.as_string(),
        no: id.fixed_no().unwrap_or(0),
        title: title.into(),
        subtitle,
        hue: None,
        scene_id: None,
        locked: false,
        unlock: None,
        appetite: None,
        family: None,
    };
    match id {
        ChannelId::Your => fixed("Your Channel", "Your library and one step out".into()),
        ChannelId::Wild => fixed("Wild Card", "Anywhere, led by your appetite".into()),
        ChannelId::Premieres => fixed("Premieres", "New videos from the last two weeks".into()),
        ChannelId::Live => fixed("Live Stage", "On stage, from every scene".into()),
        ChannelId::Sessions => fixed("Sessions", "Acoustic, stripped and in session".into()),
        ChannelId::Concerts => fixed("Concert Hall", "Whole concerts, back to back".into()),
        ChannelId::Rewind => fixed(
            "Rewind",
            ctx.rewind_decade.map_or_else(
                || "A decade a day, once enough songs know their year".into(),
                |decade| format!("The {decade}s"),
            ),
        ),
        ChannelId::Spotlight => fixed(
            "Spotlight",
            ctx.spotlight
                .as_ref()
                .map_or_else(|| "Today's artist".into(), |pick| pick.name.clone()),
        ),
        ChannelId::Scene(scene) => {
            let row = universe.scene(*scene);
            let exemplars: Vec<String> = row
                .map(|row| row.exemplars.iter().take(3).cloned().collect())
                .unwrap_or_default();
            let artists = universe.scene_artists.get(scene).copied().unwrap_or(0);
            ChannelMeta {
                id: id.as_string(),
                no: row.and_then(|row| row.channel_no).unwrap_or(0),
                title: row.map_or_else(|| format!("Scene {scene}"), |row| row.name.clone()),
                subtitle: if exemplars.is_empty() {
                    format!("{artists} artists")
                } else {
                    join_names(&exemplars)
                },
                hue: row.map(|row| row.hue),
                scene_id: Some(*scene),
                locked: false,
                unlock: None,
                appetite: taste
                    .appetite
                    .get(scene)
                    .map(|a| (a * 100.0).round() / 100.0),
                family: Some(
                    universe
                        .families
                        .get(scene)
                        .copied()
                        .unwrap_or(super::families::OTHER)
                        .to_string(),
                ),
            }
        }
    }
}

fn locked(mut meta: ChannelMeta, unlock: Option<Unlock>) -> Planned {
    meta.locked = true;
    meta.unlock = unlock;
    Planned { meta, pool: None }
}

pub fn plan_one(id: &ChannelId, universe: &Universe, taste: &Taste, ctx: &DayContext) -> Planned {
    let meta = base_meta(id, universe, taste, ctx);
    if id.needs_facets() && !universe.facets {
        return locked(meta, Some(Unlock::hours(0.0, FIXED_NEED_HOURS)));
    }
    if *id == ChannelId::Rewind && ctx.rewind_decade.is_none() {
        return locked(meta, Some(rewind_progress(universe)));
    }
    if *id == ChannelId::Spotlight && ctx.spotlight.is_none() {
        return locked(meta, None);
    }
    let built = build_pool(id, universe, taste, ctx, &Boosts::default());
    let (need_hours, need_artists) = match id {
        ChannelId::Scene(_) => (SCENE_NEED_HOURS, SCENE_NEED_ARTISTS),
        ChannelId::Spotlight => (0.0, 1),
        _ => (FIXED_NEED_HOURS, 1),
    };
    let on_air =
        !built.pool.is_empty() && built.hours >= need_hours && built.artists >= need_artists;
    if !on_air {
        let unlock = (*id != ChannelId::Spotlight).then_some(Unlock {
            hours: round1(built.hours),
            need_hours,
            artists: built.artists,
            need_artists: if matches!(id, ChannelId::Scene(_)) {
                need_artists
            } else {
                0
            },
        });
        return locked(meta, unlock);
    }
    Planned {
        meta,
        pool: Some(built.pool),
    }
}

/// Every channel of the dial in number order: fixed 1-8, then scenes that
/// hold a channel number (numbers are stable; appetite never reorders them).
pub fn plan(universe: &Universe, taste: &Taste, ctx: &DayContext) -> Vec<Planned> {
    let mut scenes: Vec<(i64, i64)> = universe
        .scenes
        .iter()
        .filter_map(|scene| scene.channel_no.map(|no| (no, scene.id)))
        .collect();
    scenes.sort();
    let mut planned: Vec<Planned> = ChannelId::FIXED
        .iter()
        .cloned()
        .chain(scenes.into_iter().map(|(_, id)| ChannelId::Scene(id)))
        .map(|id| plan_one(&id, universe, taste, ctx))
        .collect();
    planned.extend(coming_up(universe, taste, ctx));
    planned
}

/// Programmable hours and artists a scene would air with (the same rule
/// its channel uses), without building the pool.
fn scene_progress(universe: &Universe, taste: &Taste, scene: i64) -> (f64, usize) {
    let mut seconds = 0i64;
    let mut artists: Vec<i64> = Vec::new();
    for index in universe.scene_members.get(&scene).into_iter().flatten() {
        let video = &universe.videos[*index];
        let member = video.scene == Some(scene) || video.scene2 == Some(scene);
        if !member || !program_ok(video) || skip_damp(taste, video.id).is_none() {
            continue;
        }
        seconds += video.duration_s;
        artists.push(video.artist);
    }
    artists.sort_unstable();
    artists.dedup();
    (seconds as f64 / 3600.0, artists.len())
}

/// The scenes closest to a channel number, as locked "coming up" channels
/// with `no: 0`. Ranked by the further of the two thresholds; never
/// numbered here (numbers are handed out once, by the intelligence build).
fn coming_up(universe: &Universe, taste: &Taste, ctx: &DayContext) -> Vec<Planned> {
    let mut ranked: Vec<(f64, f64, usize, i64)> = universe
        .scenes
        .iter()
        .filter(|scene| scene.channel_no.is_none())
        .map(|scene| {
            let (hours, artists) = scene_progress(universe, taste, scene.id);
            let progress =
                (hours / SCENE_NEED_HOURS).min(artists as f64 / SCENE_NEED_ARTISTS as f64);
            (progress, hours, artists, scene.id)
        })
        .filter(|(progress, _, _, _)| *progress > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.3.cmp(&b.3)));
    ranked
        .into_iter()
        .take(COMING_UP)
        .map(|(_, hours, artists, scene)| {
            let meta = base_meta(&ChannelId::Scene(scene), universe, taste, ctx);
            locked(
                meta,
                Some(Unlock {
                    hours: round1(hours),
                    need_hours: SCENE_NEED_HOURS,
                    artists,
                    need_artists: SCENE_NEED_ARTISTS,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_air::universe::{ConcertRow, SceneRow, SongRow};
    use crate::services::video_discovery::graph::Relevance;

    pub(crate) fn video(id: i64, artist: i64, cut: Cut, song: u64) -> VideoRow {
        VideoRow {
            id,
            artist,
            scene: None,
            scene2: None,
            cut,
            song,
            concert: None,
            cut_year: Some(2010),
            duration_s: 240,
            quality: 0.5,
            fresh: false,
        }
    }

    fn universe(videos: Vec<VideoRow>) -> Universe {
        let mut universe = Universe {
            facets: true,
            videos,
            this_year: 2026,
            ..Default::default()
        };
        universe.index();
        universe
    }

    fn ctx() -> DayContext {
        DayContext::default()
    }

    #[test]
    fn channel_ids_round_trip_and_reject_junk() {
        for raw in [
            "your",
            "wild",
            "premieres",
            "live",
            "sessions",
            "concerts",
            "rewind",
            "spotlight",
            "scene:12",
        ] {
            assert_eq!(ChannelId::parse(raw).unwrap().as_string(), raw);
        }
        for raw in ["", "scene:", "scene:x", "scene:-1", "everything"] {
            assert!(ChannelId::parse(raw).is_none(), "{raw}");
        }
        assert_eq!(ChannelId::Your.fixed_no(), Some(1));
        assert_eq!(ChannelId::Spotlight.fixed_no(), Some(8));
    }

    #[test]
    fn a_scene_channel_unlocks_at_thirty_hours_and_forty_artists() {
        // 39 artists x 50 songs x 240 s = 130 h: enough hours, one artist short.
        let mut videos = Vec::new();
        for artist in 0..39 {
            for n in 0..50 {
                let mut v = video(
                    artist * 100 + n,
                    1000 + artist,
                    Cut::Official,
                    (artist * 100 + n) as u64,
                );
                v.scene = Some(5);
                videos.push(v);
            }
        }
        let mut u = universe(videos.clone());
        u.scenes = vec![SceneRow {
            id: 5,
            name: "Five".into(),
            hue: 120.0,
            channel_no: Some(10),
            exemplars: vec!["A".into(), "B".into(), "C".into()],
        }];
        let taste = Taste::default();
        let planned = plan(&u, &taste, &ctx());
        let scene = planned.iter().find(|p| p.meta.id == "scene:5").unwrap();
        assert!(scene.meta.locked, "39 artists");
        assert_eq!(scene.meta.no, 10);
        assert_eq!(scene.meta.subtitle, "A, B and C");
        assert_eq!(scene.meta.hue, Some(120.0));
        let unlock = scene.meta.unlock.unwrap();
        assert_eq!(unlock.need_hours, SCENE_NEED_HOURS);
        assert!(unlock.hours >= 100.0);

        // Forty artists with 7 songs each: 18.7 h, enough artists, too few hours.
        let mut few = Vec::new();
        for artist in 0..40 {
            for n in 0..7 {
                let mut v = video(
                    artist * 100 + n,
                    1000 + artist,
                    Cut::Official,
                    (artist * 100 + n) as u64,
                );
                v.scene = Some(5);
                few.push(v);
            }
        }
        let mut u2 = universe(few);
        u2.scenes = u.scenes.clone();
        let planned = plan(&u2, &taste, &ctx());
        let scene = planned.iter().find(|p| p.meta.id == "scene:5").unwrap();
        assert!(scene.meta.locked);
        assert!((scene.meta.unlock.unwrap().hours - 18.7).abs() < 0.05);

        // Add the fortieth artist to the first universe: on air.
        for n in 0..50 {
            let mut v = video(3900 + n, 1039, Cut::Official, (3900 + n) as u64);
            v.scene = Some(5);
            videos.push(v);
        }
        let mut u3 = universe(videos);
        u3.scenes = u.scenes.clone();
        let planned = plan(&u3, &taste, &ctx());
        let scene = planned.iter().find(|p| p.meta.id == "scene:5").unwrap();
        assert!(!scene.meta.locked);
        assert!(scene.pool.is_some());
    }

    #[test]
    fn scenes_without_a_number_are_not_channels_and_numbers_keep_order() {
        let mut u = universe(Vec::new());
        u.scenes = vec![
            SceneRow {
                id: 1,
                name: "One".into(),
                hue: 0.0,
                channel_no: Some(12),
                exemplars: vec![],
            },
            SceneRow {
                id: 2,
                name: "Two".into(),
                hue: 0.0,
                channel_no: None,
                exemplars: vec![],
            },
            SceneRow {
                id: 3,
                name: "Three".into(),
                hue: 0.0,
                channel_no: Some(10),
                exemplars: vec![],
            },
        ];
        let ids: Vec<String> = plan(&u, &Taste::default(), &ctx())
            .into_iter()
            .map(|p| p.meta.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "your",
                "wild",
                "premieres",
                "live",
                "sessions",
                "concerts",
                "rewind",
                "spotlight",
                "scene:3",
                "scene:1"
            ],
            "an unnumbered scene with nothing playable is not even coming up"
        );
    }

    fn scene_row(id: i64, channel_no: Option<i64>) -> SceneRow {
        SceneRow {
            id,
            name: format!("Scene {id}"),
            hue: 0.0,
            channel_no,
            exemplars: vec![],
        }
    }

    #[test]
    fn the_three_scenes_closest_to_a_number_are_coming_up() {
        // (scene, artists, videos each): 4-minute videos, so 15 per hour.
        let shapes = [
            (1, 50, 12), // numbered: 40 h, 50 artists
            (2, 20, 20), // 26.7 h, 20 artists -> 0.50
            (3, 45, 2),  // 6 h, 45 artists -> 0.20
            (4, 10, 40), // 26.7 h, 10 artists -> 0.25
            (5, 30, 10), // 20 h, 30 artists -> 0.67
            (6, 5, 2),   // 0.7 h -> 0.02
        ];
        let mut videos = Vec::new();
        for (scene, artists, each) in shapes {
            for artist in 0..artists {
                for n in 0..each {
                    let id = scene * 100_000 + artist * 100 + n;
                    let mut v = video(id, scene * 1000 + artist, Cut::Official, id as u64);
                    v.scene = Some(scene);
                    videos.push(v);
                }
            }
        }
        let mut u = universe(videos);
        u.scenes = vec![
            scene_row(1, Some(10)),
            scene_row(2, None),
            scene_row(3, None),
            scene_row(4, None),
            scene_row(5, None),
            scene_row(6, None),
        ];
        let planned = plan(&u, &Taste::default(), &ctx());
        let tail: Vec<&ChannelMeta> = planned.iter().skip(8).map(|p| &p.meta).collect();
        let ids: Vec<&str> = tail.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["scene:1", "scene:5", "scene:2", "scene:4"]);
        assert!(!tail[0].locked && tail[0].no == 10);
        for meta in &tail[1..] {
            assert!(meta.locked);
            assert_eq!(meta.no, 0, "not numbered yet");
            assert_eq!(meta.unlock.unwrap().need_hours, SCENE_NEED_HOURS);
            assert!(
                planned
                    .iter()
                    .find(|p| p.meta.id == meta.id)
                    .unwrap()
                    .pool
                    .is_none()
            );
        }
        assert!((tail[1].unlock.unwrap().hours - 20.0).abs() < 0.05);
        assert!((tail[2].unlock.unwrap().hours - 26.7).abs() < 0.05);
    }

    #[test]
    fn without_facets_cut_and_era_channels_wait() {
        let mut videos = Vec::new();
        for n in 0..400 {
            videos.push(video(n, 100 + n % 50, Cut::Other, n as u64));
        }
        let mut u = universe(videos);
        u.facets = false;
        let planned = plan(&u, &Taste::default(), &ctx());
        let by_id: HashMap<String, &Planned> =
            planned.iter().map(|p| (p.meta.id.clone(), p)).collect();
        for id in ["live", "sessions", "concerts", "rewind"] {
            assert!(by_id[id].meta.locked, "{id}");
            assert_eq!(by_id[id].meta.unlock.unwrap().hours, 0.0);
        }
        assert!(!by_id["wild"].meta.locked, "400 x 4 min is 26 h");
        assert!(by_id["your"].meta.locked, "no taste yet");
        assert!(by_id["spotlight"].meta.locked);
        assert!(by_id["spotlight"].meta.unlock.is_none());
    }

    #[test]
    fn rewind_waits_for_half_the_songs_to_know_their_year() {
        let mut videos = Vec::new();
        for n in 0..1000 {
            videos.push(video(n, 100 + n % 60, Cut::Official, n as u64));
        }
        let mut u = universe(videos);
        for n in 0..499u64 {
            u.songs.insert(
                n,
                SongRow {
                    best_video: None,
                    year: Some(1984),
                },
            );
        }
        assert!(rewind_decade(&u, "2026-10-14").is_none(), "49.9%");
        let planned = plan_one(&ChannelId::Rewind, &u, &Taste::default(), &ctx());
        assert!(planned.meta.locked);
        let unlock = planned.meta.unlock.unwrap();
        assert!(
            unlock.hours <= unlock.need_hours,
            "dated hours against half the pool"
        );
        assert!((unlock.need_hours - 33.3).abs() < 0.05);
        u.songs.insert(
            499,
            SongRow {
                best_video: None,
                year: Some(1984),
            },
        );
        assert_eq!(rewind_decade(&u, "2026-10-14"), Some(1980));
        let ctx = DayContext {
            rewind_decade: Some(1980),
            ..ctx()
        };
        let planned = plan_one(&ChannelId::Rewind, &u, &Taste::default(), &ctx);
        assert!(!planned.meta.locked);
        assert_eq!(planned.meta.subtitle, "The 1980s");
        let pool = planned.pool.unwrap();
        let songs: usize = pool.buckets.iter().map(|b| b.units.len()).sum();
        assert_eq!(songs, 500, "only 1980s songs");
    }

    #[test]
    fn rewind_never_plays_the_decade_we_are_in() {
        let mut videos = Vec::new();
        for n in 0..1000 {
            videos.push(video(n, 100 + n % 60, Cut::Official, n as u64));
        }
        let mut u = universe(videos);
        // Most dated songs are from this decade; a smaller past decade
        // still clears the hours bar on its own.
        for n in 0..1000u64 {
            let year = if n < 700 { 2023 } else { 1994 };
            u.songs.insert(
                n,
                SongRow {
                    best_video: None,
                    year: Some(year),
                },
            );
        }
        for day in ["2026-10-14", "2026-10-15", "2026-10-16"] {
            assert_eq!(rewind_decade(&u, day), Some(1990), "{day}");
        }
    }

    #[test]
    fn rewind_decades_take_turns_by_day() {
        let mut videos = Vec::new();
        for n in 0..2000 {
            videos.push(video(n, 100 + n % 60, Cut::Official, n as u64));
        }
        let mut u = universe(videos);
        for n in 0..2000u64 {
            let year = if n < 1000 { 1975 } else { 1995 };
            u.songs.insert(
                n,
                SongRow {
                    best_video: None,
                    year: Some(year),
                },
            );
        }
        let a = rewind_decade(&u, "2026-10-14").unwrap();
        let b = rewind_decade(&u, "2026-10-15").unwrap();
        assert_ne!(a, b);
        assert_eq!(rewind_decade(&u, "2026-10-16"), Some(a));
    }

    #[test]
    fn songs_air_their_best_cut_unless_the_channel_is_about_a_cut() {
        let mut official = video(1, 10, Cut::Official, 77);
        official.quality = 0.2;
        let mut live = video(2, 10, Cut::Live, 77);
        live.quality = 0.9;
        let mut lyric = video(3, 10, Cut::Lyric, 78);
        lyric.quality = 0.9;
        let mut u = universe(vec![official, live, lyric]);
        u.songs.insert(
            77,
            SongRow {
                best_video: Some(1),
                year: None,
            },
        );
        let taste = Taste::default();
        let wild = build_pool(&ChannelId::Wild, &u, &taste, &ctx(), &Boosts::default());
        let mut ids: Vec<i64> = wild.pool.buckets[0]
            .units
            .iter()
            .map(|u| u.lead.0)
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![1, 3],
            "the song's best cut, plus the lyric-only song"
        );
        let live_pool = build_pool(&ChannelId::Live, &u, &taste, &ctx(), &Boosts::default());
        assert_eq!(live_pool.pool.buckets[0].units[0].lead.0, 2);
    }

    #[test]
    fn long_form_twice_zapped_and_overlong_videos_stay_off_the_air() {
        let mut long = video(1, 10, Cut::Longform, 1);
        long.duration_s = 3600;
        let mut overlong = video(2, 10, Cut::Live, 2);
        overlong.duration_s = MAX_PROGRAM_S + 1;
        let zapped = video(3, 10, Cut::Official, 3);
        let fine = video(4, 10, Cut::Official, 4);
        let u = universe(vec![long, overlong, zapped, fine]);
        let mut taste = Taste::default();
        taste.videos.insert(
            3,
            super::super::taste::VideoStats {
                plays: 2,
                zaps: 2,
                holds: 0,
                last_age_days: 1.0,
            },
        );
        let built = build_pool(&ChannelId::Wild, &u, &taste, &ctx(), &Boosts::default());
        let ids: Vec<i64> = built.pool.buckets[0]
            .units
            .iter()
            .map(|u| u.lead.0)
            .collect();
        assert_eq!(ids, vec![4]);
    }

    #[test]
    fn your_channel_is_roots_and_one_hop_and_rings_follow_relevance() {
        let u = universe(vec![
            video(1, 10, Cut::Official, 1),
            video(2, 20, Cut::Official, 2),
            video(3, 30, Cut::Official, 3),
        ]);
        let mut taste = Taste::default();
        taste.relevance.insert(
            10,
            Relevance {
                score: 0.9,
                hops: 0,
            },
        );
        taste.relevance.insert(
            20,
            Relevance {
                score: 0.1,
                hops: 1,
            },
        );
        taste.relevance.insert(
            30,
            Relevance {
                score: 0.09,
                hops: 2,
            },
        );
        let built = build_pool(&ChannelId::Your, &u, &taste, &ctx(), &Boosts::default());
        let artists: Vec<(i64, usize)> = built
            .pool
            .buckets
            .iter()
            .map(|b| (b.artist, b.ring))
            .collect();
        assert_eq!(artists, vec![(10, 0), (20, 1)]);
        assert_eq!(
            built.pool.rings,
            ExploreMode::Familiar.rings(),
            "mixed knob, one step closer"
        );
        let wild = build_pool(&ChannelId::Wild, &u, &taste, &ctx(), &Boosts::default());
        assert_eq!(wild.pool.rings, ExploreMode::Adventurous.rings());
        assert!(wild.pool.frontier && !built.pool.frontier);
    }

    #[test]
    fn concert_hall_units_are_whole_setlists() {
        let mut videos: Vec<VideoRow> =
            (1..=4).map(|n| video(n, 10, Cut::Live, n as u64)).collect();
        for v in &mut videos {
            v.concert = Some(9);
        }
        let mut u = universe(videos);
        u.concerts = vec![ConcertRow {
            id: 9,
            artist: 10,
            label: "Live at Home 2015".into(),
            video_ids: vec![3, 1, 2, 99],
        }];
        let built = build_pool(
            &ChannelId::Concerts,
            &u,
            &Taste::default(),
            &ctx(),
            &Boosts::default(),
        );
        let unit = &built.pool.buckets[0].units[0];
        let ids: Vec<i64> = unit.videos().map(|(id, _)| id).collect();
        assert_eq!(ids, vec![3, 1, 2], "setlist order, unknown ids skipped");
        assert_eq!(unit.why, "Live at Home 2015");
        assert!(built.pool.unique_per_day);
    }

    #[test]
    fn why_chips_follow_their_priority() {
        let mut v = video(1, 10, Cut::Live, 1);
        v.scene = Some(3);
        let mut u = universe(vec![v.clone()]);
        u.scenes = vec![SceneRow {
            id: 3,
            name: "Bay Area Rap".into(),
            hue: 0.0,
            channel_no: Some(10),
            exemplars: vec![],
        }];
        u.songs.insert(
            1,
            SongRow {
                best_video: Some(1),
                year: Some(1994),
            },
        );
        let mut taste = Taste::default();
        let wild = ChannelId::Wild;
        assert_eq!(why(&wild, &v, &u, &taste), "Bay Area Rap");
        assert_eq!(
            why(&ChannelId::Scene(3), &v, &u, &taste),
            "Live",
            "scene chip skipped on its own channel"
        );
        let mut sceneless = v.clone();
        sceneless.scene = None;
        assert_eq!(why(&ChannelId::Live, &sceneless, &u, &taste), "1994");
        taste.relevance.insert(
            10,
            Relevance {
                score: 0.5,
                hops: 1,
            },
        );
        taste.via.insert(10, 77);
        taste.names.insert(77, "James Blake".into());
        taste.roots.insert(
            77,
            super::super::taste::Root {
                weight: 1.0,
                plays: 33,
                library: true,
                ..Default::default()
            },
        );
        assert_eq!(why(&wild, &v, &u, &taste), "Near James Blake, 33 plays");
        let mut in_concert = v.clone();
        in_concert.concert = Some(4);
        u.concerts = vec![ConcertRow {
            id: 4,
            artist: 10,
            label: "Live at Trianon 2015".into(),
            video_ids: vec![1],
        }];
        assert_eq!(why(&wild, &in_concert, &u, &taste), "Live at Trianon 2015");
        let mut plain = video(2, 50, Cut::Official, 2);
        plain.cut_year = Some(2026);
        assert_eq!(why(&wild, &plain, &u, &taste), "New");
    }
}
