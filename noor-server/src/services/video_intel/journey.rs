//! Journeys across the Atlas (GET /api/videos/atlas/journey).
//!
//! - Collabs: everyone an artist shares a video with (a video by A that
//!   features B, or the reverse), shown on the artist card and the map.
//! - Somewhere new: from where the listener is to a scene they have barely
//!   seen, over the related-artist graph, preferring artists they have not
//!   watched, ending at one of the destination's top artists.
//!
//! The searches are pure over cached data: the collab graph and the map
//! facts are rebuilt only when the intel build signature changes; the
//! related-artist graph is `scenes::cached_graph`. The route assembles
//! names, videos and why lines.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rusqlite::Connection;

use super::archive;
use super::light::{self, LightMap};
use super::scenes::{self, ArtistGraph, EDGE_SOURCES, MIN_PROGRAMMABLE_S, VARIOUS_ARTISTS_ID};

/// Each leg of a journey runs four to eight stops, the start included,
/// and aims for at least six: the cheapest path would otherwise take the
/// fewest stops allowed, and a journey of four is over before it starts.
pub const NEW_MIN_STOPS: usize = 4;
pub const LEG_AIM_STOPS: usize = 6;
pub const NEW_MAX_STOPS: usize = 8;
/// The last leg to a chosen artist may run longer: two artists can sit far
/// apart, and the path still has to reach that one.
pub const TO_MAX_STOPS: usize = 13;
/// Destinations need this many artists; smaller scenes are too thin to visit.
pub const DESTINATION_MIN_ARTISTS: i64 = 8;
/// The path ends at one of this many top-ranked artists of the destination.
pub const DESTINATION_TOP: usize = 10;
/// Destinations tried in order until one has a path.
const DESTINATION_TRIES: usize = 6;
const NEW_HOP_TOLL: f64 = 0.15;
/// Extra cost of a stop the listener already knows: watched or saved (lit),
/// or in the library or next to it (warm).
const LIT_TOLL: f64 = 1.0;
const WARM_TOLL: f64 = 0.3;
/// A programmable link outweighs any popularity.
const PROGRAMMABLE_BONUS: i64 = 100;

// --------------------------------------------------------------- caches

/// (database file, build signature, value)
type Slot<T> = Mutex<Option<(String, String, Arc<T>)>>;
static COLLAB: Slot<CollabGraph> = Mutex::new(None);
static FACTS: Slot<MapFacts> = Mutex::new(None);

/// The cached value for the current build, loading it when the build
/// changed. In-memory databases (tests) are never cached.
fn cached<T>(
    slot: &Slot<T>,
    conn: &Connection,
    load: impl FnOnce() -> Result<T>,
) -> Result<Arc<T>> {
    let Some(key) = scenes::cache_key(conn) else {
        return Ok(Arc::new(load()?));
    };
    let signature = archive::signature(conn)?;
    if let Ok(guard) = slot.lock()
        && let Some((cached_key, cached_signature, value)) = guard.as_ref()
        && *cached_key == key
        && *cached_signature == signature
    {
        return Ok(value.clone());
    }
    let value = Arc::new(load()?);
    if let Ok(mut guard) = slot.lock() {
        *guard = Some((key, signature, value.clone()));
    }
    Ok(value)
}

/// The collab graph of the current build.
pub fn cached_collab(conn: &Connection) -> Result<Arc<CollabGraph>> {
    cached(&COLLAB, conn, || CollabGraph::load(conn))
}

/// The map facts of the current build.
pub fn cached_facts(conn: &Connection) -> Result<Arc<MapFacts>> {
    cached(&FACTS, conn, || {
        let graph = scenes::cached_graph(conn)?;
        MapFacts::load(conn, &graph)
    })
}

// --------------------------------------------------------------- collab

/// One video two artists are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub video_id: i64,
    /// The video's own artist; the other end of the pair is featured.
    pub main: i64,
    pub programmable: bool,
    pub official: bool,
    pub popularity: i64,
}

impl Link {
    /// Best first within a pair: programmable, the official cut, popularity.
    fn order(&self, other: &Self) -> Ordering {
        other
            .programmable
            .cmp(&self.programmable)
            .then(other.official.cmp(&self.official))
            .then(other.popularity.cmp(&self.popularity))
            .then(other.video_id.cmp(&self.video_id))
    }

    /// How much a link is worth showing: programmable first, then popular.
    fn value(&self) -> i64 {
        self.popularity.clamp(0, 100)
            + if self.programmable {
                PROGRAMMABLE_BONUS
            } else {
                0
            }
    }
}

/// A catalog video that features other artists.
#[derive(Debug, Clone, Default)]
pub struct CollabVideo {
    pub video_id: i64,
    pub main: i64,
    pub featured: Vec<i64>,
    pub programmable: bool,
    pub official: bool,
    pub popularity: i64,
}

/// Undirected graph of artists who share a video: the main artist linked to
/// each featured artist, nodes by ascending TIDAL id, CSR rows.
#[derive(Debug, Default)]
pub struct CollabGraph {
    ids: Vec<i64>,
    offsets: Vec<usize>,
    targets: Vec<u32>,
    /// Per CSR entry, its pair's index into `links`.
    pairs: Vec<u32>,
    /// Per pair, its best link.
    links: Vec<Link>,
}

impl CollabGraph {
    pub fn from_videos(videos: &[CollabVideo]) -> Self {
        let mut by_pair: HashMap<(i64, i64), Vec<Link>> = HashMap::new();
        for video in videos {
            if video.main <= 0 || video.main == VARIOUS_ARTISTS_ID {
                continue;
            }
            let mut featured = video.featured.clone();
            featured.sort_unstable();
            featured.dedup();
            for other in featured {
                if other <= 0 || other == video.main || other == VARIOUS_ARTISTS_ID {
                    continue;
                }
                by_pair
                    .entry((video.main.min(other), video.main.max(other)))
                    .or_default()
                    .push(Link {
                        video_id: video.video_id,
                        main: video.main,
                        programmable: video.programmable,
                        official: video.official,
                        popularity: video.popularity,
                    });
            }
        }
        // Hash order is arbitrary; rows want a fixed one.
        let mut pairs: Vec<((i64, i64), Vec<Link>)> = by_pair.into_iter().collect();
        pairs.sort_unstable_by_key(|(pair, _)| *pair);
        let mut ids: Vec<i64> = pairs.iter().flat_map(|((a, b), _)| [*a, *b]).collect();
        ids.sort_unstable();
        ids.dedup();
        let index = |id: i64| ids.binary_search(&id).unwrap_or(0);
        let mut offsets = vec![0usize; ids.len() + 1];
        for ((a, b), _) in &pairs {
            offsets[index(*a) + 1] += 1;
            offsets[index(*b) + 1] += 1;
        }
        for i in 0..ids.len() {
            offsets[i + 1] += offsets[i];
        }
        let mut cursor = offsets.clone();
        let mut targets = vec![0u32; offsets[ids.len()]];
        let mut pair_of = vec![0u32; offsets[ids.len()]];
        let mut links = Vec::with_capacity(pairs.len());
        for (pair, ((a, b), mut list)) in pairs.into_iter().enumerate() {
            let (a, b) = (index(a), index(b));
            for (from, to) in [(a, b), (b, a)] {
                targets[cursor[from]] = to as u32;
                pair_of[cursor[from]] = pair as u32;
                cursor[from] += 1;
            }
            list.sort_by(Link::order);
            links.push(list[0]);
        }
        Self {
            ids,
            offsets,
            targets,
            pairs: pair_of,
            links,
        }
    }

    /// Every catalog video with featured artists; cut, programmable and
    /// popularity from the catalog stage when it has run, before that 90 s
    /// or more counts as programmable.
    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT c.tidal_video_id, c.artist_tidal_id,
                    json_extract(c.item_json, '$.featured_artist_ids'),
                    COALESCE(f.popularity, json_extract(c.item_json, '$.popularity')),
                    f.programmable, f.cut,
                    COALESCE(json_extract(c.item_json, '$.duration_s'),
                             json_extract(c.item_json, '$.duration_ms') / 1000)
               FROM video_catalog c
               LEFT JOIN video_facets f ON f.tidal_video_id = c.tidal_video_id
              WHERE instr(c.item_json, '\"featured_artist_ids\"') > 0",
        )?;
        let mut rows = stmt.query([])?;
        let mut videos = Vec::new();
        while let Some(row) = rows.next()? {
            let Some(main) = row.get::<_, Option<i64>>(1)? else {
                continue;
            };
            let featured: Option<String> = row.get(2)?;
            let featured: Vec<i64> = featured
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or_default();
            if featured.is_empty() {
                continue;
            }
            let duration: Option<f64> = row.get(6)?;
            let programmable = match row.get::<_, Option<i64>>(4)? {
                Some(flag) => flag != 0,
                None => duration.is_none_or(|d| d >= MIN_PROGRAMMABLE_S as f64),
            };
            videos.push(CollabVideo {
                video_id: row.get(0)?,
                main,
                featured,
                programmable,
                official: row.get::<_, Option<String>>(5)?.as_deref() == Some("official"),
                popularity: row.get::<_, Option<f64>>(3)?.unwrap_or(0.0) as i64,
            });
        }
        Ok(Self::from_videos(&videos))
    }

    fn index_of(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    /// Everyone `id` shares a video with, each with the pair's best link,
    /// best link first (programmable, then popular), ties by id.
    pub fn collaborators(&self, id: i64) -> Vec<(i64, Link)> {
        let Some(node) = self.index_of(id) else {
            return Vec::new();
        };
        let mut out: Vec<(i64, Link)> = (self.offsets[node]..self.offsets[node + 1])
            .map(|entry| {
                (
                    self.ids[self.targets[entry] as usize],
                    self.links[self.pairs[entry] as usize],
                )
            })
            .collect();
        out.sort_by(|a, b| b.1.value().cmp(&a.1.value()).then(a.0.cmp(&b.0)));
        out
    }
}

// --------------------------------------------------------------- map facts

/// What a journey needs to know of one artist on the map.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ArtistFact {
    pub scene: Option<i64>,
    pub charted: bool,
    /// Has a programmable video, so a journey stop can play it.
    pub playable: bool,
}

/// Every artist on the map (x and y set) and each scene's top artists.
#[derive(Debug, Default)]
pub struct MapFacts {
    pub artists: HashMap<i64, ArtistFact>,
    /// Per scene, its playable members by rank (the scene stage's exemplar
    /// order), at most `DESTINATION_TOP`.
    pub tops: HashMap<i64, Vec<i64>>,
}

impl MapFacts {
    /// From facet rows of (artist, scene, charted, programmable videos,
    /// catalog videos) and the ranking inputs. Before the link stage has
    /// counted programmable videos, any catalog video counts.
    pub fn from_rows(
        rows: &[(i64, Option<i64>, bool, i64, i64)],
        graph: &ArtistGraph,
        popularity: &HashMap<i64, i64>,
    ) -> Self {
        let counted = rows.iter().any(|row| row.3 > 0);
        let mut artists = HashMap::with_capacity(rows.len());
        let mut members: HashMap<i64, Vec<(f64, i64)>> = HashMap::new();
        for &(id, scene, charted, playable, videos) in rows {
            let playable = if counted { playable > 0 } else { videos > 0 };
            artists.insert(
                id,
                ArtistFact {
                    scene,
                    charted,
                    playable,
                },
            );
            if let Some(scene) = scene
                && playable
            {
                let score =
                    scenes::exemplar_score(videos, graph.degree(id), popularity.get(&id).copied());
                members.entry(scene).or_default().push((score, id));
            }
        }
        let tops = members
            .into_iter()
            .map(|(scene, mut list)| {
                list.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                list.truncate(DESTINATION_TOP);
                (scene, list.into_iter().map(|(_, id)| id).collect())
            })
            .collect();
        Self { artists, tops }
    }

    pub fn load(conn: &Connection, graph: &ArtistGraph) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, scene_id, charted, playable, videos FROM video_artist_facets
              WHERE x IS NOT NULL AND y IS NOT NULL",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)? != 0,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_rows(
            &rows,
            graph,
            &scenes::load_popularity(conn)?,
        ))
    }

    pub fn charted(&self, id: i64) -> bool {
        self.artists.get(&id).is_some_and(|a| a.charted)
    }

    pub fn scene(&self, id: i64) -> Option<i64> {
        self.artists.get(&id).and_then(|a| a.scene)
    }

    fn playable(&self, id: i64) -> bool {
        self.artists.get(&id).is_some_and(|a| a.playable)
    }
}

// --------------------------------------------------------------- somewhere new

/// Hops from `from` to every node within `max_hops` over nodes `allowed`
/// accepts, by graph index; u8::MAX where out of reach.
fn hops_from(
    graph: &ArtistGraph,
    from: usize,
    max_hops: usize,
    allowed: impl Fn(i64) -> bool,
) -> Vec<u8> {
    let mut hops = vec![u8::MAX; graph.ids.len()];
    hops[from] = 0;
    let mut frontier = vec![from];
    for depth in 1..=max_hops.min(u8::MAX as usize - 1) {
        let mut next = Vec::new();
        for node in frontier {
            for &target in graph.csr.row(node).0 {
                let target = target as usize;
                if hops[target] == u8::MAX && allowed(graph.ids[target]) {
                    hops[target] = depth as u8;
                    next.push(target);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    hops
}

/// Cheapest simple path from `from` to any of `goals`, between `min_hops`
/// and `max_hops` edges. A step costs -ln(weight) plus a toll plus
/// `stop_toll` of the artist it reaches; None refuses the artist. Goals are
/// only ever the last stop: a path arrives at the first one it reaches, so
/// no cheap pass through a goal can shadow the path that ends there.
/// Dijkstra over (node, hops) states, so the hop window never hides a
/// cheaper path that fits. Each stop carries the provider of the edge that
/// reached it.
pub fn route_to_any(
    graph: &ArtistGraph,
    from: i64,
    goals: &HashSet<i64>,
    min_hops: usize,
    max_hops: usize,
    stop_toll: impl Fn(i64) -> Option<f64>,
) -> Option<Vec<(i64, Option<&'static str>)>> {
    #[derive(PartialEq)]
    struct Open {
        cost: f64,
        hops: usize,
        node: u32,
    }
    impl Eq for Open {}
    impl Ord for Open {
        fn cmp(&self, other: &Self) -> Ordering {
            other
                .cost
                .total_cmp(&self.cost)
                .then(other.hops.cmp(&self.hops))
                .then(other.node.cmp(&self.node))
        }
    }
    impl PartialOrd for Open {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    let start = graph.index_of(from)?;
    if goals.is_empty() || max_hops < min_hops.max(1) {
        return None;
    }
    let layers = max_hops + 1;
    let n = graph.ids.len();
    let mut best = vec![f64::INFINITY; n * layers];
    // (previous node, CSR entry used to arrive)
    let mut came: Vec<(u32, usize)> = vec![(u32::MAX, usize::MAX); n * layers];
    let on_path = |came: &[(u32, usize)], mut at: usize, mut layer: usize, node: usize| loop {
        if at == node {
            return true;
        }
        if layer == 0 {
            return false;
        }
        at = came[at * layers + layer].0 as usize;
        layer -= 1;
    };
    let mut heap = BinaryHeap::new();
    best[start * layers] = 0.0;
    heap.push(Open {
        cost: 0.0,
        hops: 0,
        node: start as u32,
    });
    while let Some(Open { cost, hops, node }) = heap.pop() {
        let node = node as usize;
        if cost > best[node * layers + hops] {
            continue;
        }
        if hops >= min_hops.max(1) && goals.contains(&graph.ids[node]) {
            let mut path = Vec::with_capacity(hops + 1);
            let (mut at, mut layer) = (node, hops);
            while layer > 0 {
                let (previous, entry) = came[at * layers + layer];
                path.push((
                    graph.ids[at],
                    Some(EDGE_SOURCES[graph.sources[entry] as usize]),
                ));
                at = previous as usize;
                layer -= 1;
            }
            path.push((from, None));
            path.reverse();
            return Some(path);
        }
        if hops == max_hops {
            continue;
        }
        for entry in graph.csr.offsets[node]..graph.csr.offsets[node + 1] {
            let next = graph.csr.targets[entry] as usize;
            if next == node {
                continue;
            }
            let layer = hops + 1;
            if layer < min_hops && goals.contains(&graph.ids[next]) {
                continue;
            }
            let Some(toll) = stop_toll(graph.ids[next]) else {
                continue;
            };
            if on_path(&came, node, hops, next) {
                continue;
            }
            let step = -graph.csr.weights[entry].clamp(1e-3, 1.0).ln() + NEW_HOP_TOLL + toll;
            let total = cost + step;
            // Past the minimum, an arrival with fewer hops (but enough) that
            // is no dearer leaves this one nothing to add.
            let floor = if layer > min_hops { min_hops } else { layer };
            if (floor..=layer).any(|h| best[next * layers + h] <= total) {
                continue;
            }
            best[next * layers + layer] = total;
            came[next * layers + layer] = (node as u32, entry);
            heap.push(Open {
                cost: total,
                hops: layer,
                node: next as u32,
            });
        }
    }
    None
}

/// One scene as a destination sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSeen {
    pub id: i64,
    pub name: String,
    pub videos: i64,
    pub artists: i64,
    pub seen_videos: i64,
}

/// Every scene with the listener's distinct watched videos in it.
pub fn scenes_seen(conn: &Connection) -> Result<Vec<SceneSeen>> {
    let seen = super::census::seen_per_scene(conn)?;
    let mut stmt = conn.prepare(
        "SELECT scene_id, name, video_count, artist_count FROM video_scenes ORDER BY scene_id",
    )?;
    let rows = stmt.query_map([], |row| {
        let id: i64 = row.get(0)?;
        Ok(SceneSeen {
            id,
            name: row.get(1)?,
            videos: row.get(2)?,
            artists: row.get(3)?,
            seen_videos: seen.get(&id).copied().unwrap_or(0),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Destination order: least seen share (seen videos over videos) first,
/// then nearer, then bigger. `distance` is None for scenes out of reach,
/// which are left out, as are scenes under `DESTINATION_MIN_ARTISTS` and
/// the start's own scene.
pub fn destinations(
    scenes: &[SceneSeen],
    own: Option<i64>,
    distance: impl Fn(i64) -> Option<usize>,
) -> Vec<i64> {
    let mut candidates: Vec<(&SceneSeen, usize)> = scenes
        .iter()
        .filter(|scene| scene.artists >= DESTINATION_MIN_ARTISTS && Some(scene.id) != own)
        .filter_map(|scene| Some((scene, distance(scene.id)?)))
        .collect();
    let share = |scene: &SceneSeen| {
        (
            scene.seen_videos.max(0) as i128,
            scene.videos.max(1) as i128,
        )
    };
    candidates.sort_by(|(a, da), (b, db)| {
        let ((sa, va), (sb, vb)) = (share(a), share(b));
        (sa * vb)
            .cmp(&(sb * va))
            .then(da.cmp(db))
            .then(b.videos.cmp(&a.videos))
            .then(a.id.cmp(&b.id))
    });
    candidates.into_iter().map(|(scene, _)| scene.id).collect()
}

/// A planned trip somewhere new.
#[derive(Debug, Clone, PartialEq)]
pub struct NewTrip {
    pub scene: i64,
    /// Stops from the start, each with the provider of the edge into it.
    pub path: Vec<(i64, Option<&'static str>)>,
}

/// What a journey has already passed: its artists (never stopped at again)
/// and its scenes (never chosen again as somewhere new).
#[derive(Debug, Clone, Default)]
pub struct Visited {
    pub artists: HashSet<i64>,
    pub scenes: HashSet<i64>,
}

/// The cost of stopping at an artist: unknown ones are free, ones the
/// listener knows cost more; None refuses it (no programmable video, or the
/// journey has been there).
fn stop_toll(facts: &MapFacts, light: &LightMap, visited: &Visited, id: i64) -> Option<f64> {
    (facts.playable(id) && !visited.artists.contains(&id)).then(|| {
        match light.light(id, facts.charted(id)) {
            light::LIT => LIT_TOLL,
            light::WARM => WARM_TOLL,
            _ => 0.0,
        }
    })
}

/// One leg somewhere new from `from`: to `scene` when given, else to the
/// best destination with a path, never to a scene or through an artist the
/// journey has already visited. Every stop after the start is on the map
/// with a programmable video; stops the listener knows cost more. Err is the
/// reason shown when nothing is found.
fn plan_leg(
    graph: &ArtistGraph,
    facts: &MapFacts,
    light: &LightMap,
    scenes: &[SceneSeen],
    from: i64,
    scene: Option<i64>,
    visited: &Visited,
) -> Result<NewTrip, String> {
    let max_hops = NEW_MAX_STOPS - 1;
    let allowed = |id: i64| facts.playable(id);
    let stop_toll = |id: i64| stop_toll(facts, light, visited, id);
    let goals_of = |scene: i64| -> HashSet<i64> {
        facts
            .tops
            .get(&scene)
            .map(|tops| {
                tops.iter()
                    .copied()
                    .filter(|id| *id != from && !visited.artists.contains(id))
                    .collect()
            })
            .unwrap_or_default()
    };
    let fresh: Vec<SceneSeen> = scenes
        .iter()
        .filter(|s| !visited.scenes.contains(&s.id))
        .cloned()
        .collect();
    let order: Vec<i64> = match scene {
        Some(scene) => {
            if !scenes.iter().any(|s| s.id == scene) {
                return Err("unknown scene".into());
            }
            vec![scene]
        }
        None => {
            let Some(start) = graph.index_of(from) else {
                return Err("no unexplored scene within reach".into());
            };
            let hops = hops_from(graph, start, max_hops, allowed);
            let order = destinations(&fresh, facts.scene(from), |scene| {
                goals_of(scene)
                    .iter()
                    .filter_map(|id| graph.index_of(*id))
                    .map(|index| hops[index])
                    .filter(|h| *h != u8::MAX)
                    .min()
                    .map(usize::from)
            });
            if order.is_empty() {
                return Err("no unexplored scene within reach".into());
            }
            order
        }
    };
    for scene in order.iter().take(DESTINATION_TRIES) {
        let goals = goals_of(*scene);
        let path = route_to_any(graph, from, &goals, LEG_AIM_STOPS - 1, max_hops, stop_toll)
            .or_else(|| route_to_any(graph, from, &goals, NEW_MIN_STOPS - 1, max_hops, stop_toll));
        if let Some(path) = path {
            return Ok(NewTrip {
                scene: *scene,
                path,
            });
        }
    }
    Err(match scene {
        Some(scene) => format!(
            "no route to {} within {NEW_MAX_STOPS} stops",
            scenes
                .iter()
                .find(|s| s.id == scene)
                .map_or("that scene", |s| s.name.as_str())
        ),
        None => format!("no route to a new scene within {NEW_MAX_STOPS} stops"),
    })
}

// --------------------------------------------------------------- tours

/// How far a journey goes: through one new scene, two, or four. Each leg
/// aims for six to eight stops, so about 6, 12 or 24 in all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    Short,
    Long,
    Epic,
}

impl Length {
    /// `short`, `long` or `epic`; absent is short.
    pub fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None | Some("short") => Ok(Self::Short),
            Some("long") => Ok(Self::Long),
            Some("epic") => Ok(Self::Epic),
            Some(_) => Err("length must be short, long or epic"),
        }
    }

    pub fn legs(self) -> usize {
        match self {
            Self::Short => 1,
            Self::Long => 2,
            Self::Epic => 4,
        }
    }
}

/// A journey of one or more legs.
#[derive(Debug, Clone, PartialEq)]
pub struct Tour {
    /// The scene each leg arrived in; for a trip to an artist, the last leg
    /// arrives at that artist instead and has no entry here.
    pub scenes: Vec<i64>,
    /// Stops from the start, each with the provider of the edge into it.
    pub path: Vec<(i64, Option<&'static str>)>,
    /// Index in `path` where each waypoint leg arrived.
    pub arrivals: Vec<usize>,
}

impl Tour {
    fn start(from: i64, facts: &MapFacts) -> (Self, Visited) {
        let mut visited = Visited::default();
        visited.artists.insert(from);
        if let Some(own) = facts.scene(from) {
            visited.scenes.insert(own);
        }
        (
            Self {
                scenes: Vec::new(),
                path: vec![(from, None)],
                arrivals: Vec::new(),
            },
            visited,
        )
    }

    fn at(&self) -> i64 {
        self.path.last().map_or(0, |stop| stop.0)
    }

    /// Add a leg that starts where the tour stands.
    fn extend(&mut self, leg: &[(i64, Option<&'static str>)], visited: &mut Visited) {
        for stop in leg.iter().skip(1) {
            visited.artists.insert(stop.0);
            self.path.push(*stop);
        }
    }
}

/// Up to `waypoints` legs somewhere new from `from`, each to a scene the
/// journey has not been to (and never to `keep_clear`, the final scene).
/// Stops early when the map runs out of new scenes in reach.
fn waypoint_legs(
    graph: &ArtistGraph,
    facts: &MapFacts,
    light: &LightMap,
    scenes: &[SceneSeen],
    from: i64,
    waypoints: usize,
    keep_clear: Option<i64>,
) -> (Tour, Visited) {
    let (mut tour, mut visited) = Tour::start(from, facts);
    for _ in 0..waypoints {
        let mut around = visited.clone();
        if let Some(scene) = keep_clear {
            around.scenes.insert(scene);
        }
        let Ok(leg) = plan_leg(graph, facts, light, scenes, tour.at(), None, &around) else {
            break;
        };
        tour.extend(&leg.path, &mut visited);
        visited.scenes.insert(leg.scene);
        tour.scenes.push(leg.scene);
        tour.arrivals.push(tour.path.len() - 1);
    }
    (tour, visited)
}

/// Somewhere new in `legs` legs: each leg from where the last arrived to a
/// scene the journey has not been to; the last to `scene` when one is
/// chosen. With a chosen scene, waypoints are dropped one by one until the
/// last leg can reach it.
pub fn plan_tour(
    graph: &ArtistGraph,
    facts: &MapFacts,
    light: &LightMap,
    scenes: &[SceneSeen],
    from: i64,
    scene: Option<i64>,
    legs: usize,
) -> Result<Tour, String> {
    let legs = legs.max(1);
    if scene.is_none() {
        let (mut tour, visited) = waypoint_legs(graph, facts, light, scenes, from, legs, None);
        if tour.scenes.is_empty() {
            // Say why, as one leg would.
            return plan_leg(graph, facts, light, scenes, from, None, &visited).map(|_| tour);
        }
        tour.arrivals.pop();
        return Ok(tour);
    }
    let mut last_err = String::new();
    for waypoints in (0..legs).rev() {
        let (mut tour, visited) =
            waypoint_legs(graph, facts, light, scenes, from, waypoints, scene);
        if tour.scenes.len() < waypoints {
            continue;
        }
        match plan_leg(graph, facts, light, scenes, tour.at(), scene, &visited) {
            Ok(leg) => {
                let mut visited = visited;
                tour.extend(&leg.path, &mut visited);
                tour.scenes.push(leg.scene);
                return Ok(tour);
            }
            Err(reason) => last_err = reason,
        }
    }
    Err(last_err)
}

/// From `from` to the artist `to`: through `legs - 1` new scenes first when
/// the journey is longer, then the cheapest path to `to` within
/// `TO_MAX_STOPS`; waypoints are dropped one by one until it is reached.
pub fn plan_to(
    graph: &ArtistGraph,
    facts: &MapFacts,
    light: &LightMap,
    scenes: &[SceneSeen],
    from: i64,
    to: i64,
    legs: usize,
) -> Result<Tour, String> {
    if from == to {
        return Err("that is where it starts".into());
    }
    let goals = HashSet::from([to]);
    for waypoints in (0..legs.max(1)).rev() {
        let (mut tour, mut visited) = waypoint_legs(
            graph,
            facts,
            light,
            scenes,
            from,
            waypoints,
            facts.scene(to),
        );
        if tour.scenes.len() < waypoints {
            continue;
        }
        visited.artists.remove(&to);
        // The chosen artist is always a stop, video or not.
        let toll = |id: i64| {
            if id == to {
                Some(0.0)
            } else {
                stop_toll(facts, light, &visited, id)
            }
        };
        if let Some(leg) = route_to_any(graph, tour.at(), &goals, 1, TO_MAX_STOPS - 1, toll) {
            tour.extend(&leg, &mut visited);
            return Ok(tour);
        }
    }
    Err(format!("no route within {TO_MAX_STOPS} stops"))
}

// --------------------------------------------------------------- videos

/// Programmable videos of one artist, best first: the catalog stage's
/// quality order once it has run, before that the most popular of 90 s or
/// more (the Atlas cards' order).
pub fn programmable_videos(conn: &Connection, artist_id: i64) -> Result<Vec<i64>> {
    if artist_id <= 0 || artist_id == VARIOUS_ARTISTS_ID {
        return Ok(Vec::new());
    }
    if scenes::facets_ready(conn)? {
        let mut stmt = conn.prepare_cached(
            "SELECT tidal_video_id FROM video_facets
              WHERE artist_tidal_id = ?1 AND programmable = 1
              ORDER BY quality DESC, COALESCE(popularity, -1) DESC, tidal_video_id DESC",
        )?;
        let rows = stmt
            .query_map([artist_id], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        return Ok(rows);
    }
    let mut stmt = conn.prepare_cached(
        "SELECT tidal_video_id, json_extract(item_json, '$.duration_s') FROM video_catalog
          WHERE artist_tidal_id = ?1
          ORDER BY COALESCE(json_extract(item_json, '$.popularity'), -1) DESC, tidal_video_id DESC",
    )?;
    let rows = stmt
        .query_map([artist_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, duration)| duration.is_none_or(|d| d >= MIN_PROGRAMMABLE_S as f64))
        .map(|(id, _)| id)
        .collect())
}

/// The first of `ranked` outside `never`, preferring one outside `avoid`.
pub fn pick_video(ranked: &[i64], avoid: &HashSet<i64>, never: &HashSet<i64>) -> Option<i64> {
    let mut open = ranked.iter().copied().filter(|id| !never.contains(id));
    let first = open.clone().next();
    open.find(|id| !avoid.contains(id)).or(first)
}

/// Every video in the listener's history.
pub fn watched_videos(conn: &Connection) -> Result<HashSet<i64>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT tidal_video_id FROM video_history WHERE tidal_video_id > 0")?;
    let rows = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: i64, main: i64, featured: &[i64], popularity: i64) -> CollabVideo {
        CollabVideo {
            video_id: id,
            main,
            featured: featured.to_vec(),
            programmable: true,
            official: true,
            popularity,
        }
    }

    #[test]
    fn pairs_keep_their_best_links_programmable_and_official_first() {
        let mut clip = video(1, 10, &[20], 90);
        clip.programmable = false;
        let mut live = video(2, 20, &[10], 80);
        live.official = false;
        let graph = CollabGraph::from_videos(&[
            clip,
            live,
            video(3, 10, &[20, 20, 10, VARIOUS_ARTISTS_ID], 5),
        ]);
        assert_eq!(
            graph.ids,
            vec![10, 20],
            "various artists and self links dropped"
        );
        assert_eq!(
            graph.links[0].video_id, 3,
            "official beats popular, clips fall off"
        );
    }

    #[test]
    fn collaborators_come_best_link_first() {
        let mut clip = video(4, 1, &[5], 99);
        clip.programmable = false;
        let graph = CollabGraph::from_videos(&[
            video(1, 1, &[2], 10),
            video(2, 3, &[1], 60),
            video(3, 1, &[2, 4], 60),
            clip,
        ]);
        let ids: Vec<(i64, i64)> = graph
            .collaborators(1)
            .into_iter()
            .map(|(id, link)| (id, link.video_id))
            .collect();
        assert_eq!(ids, vec![(2, 3), (3, 2), (4, 3), (5, 4)]);
        assert!(graph.collaborators(99).is_empty());
    }

    #[test]
    fn routes_to_any_goal_respect_the_hop_window_and_tolls() {
        // 1 - 2 - 3 - 4 strong, 1 - 5 - 6 - 4 a little weaker.
        let graph = ArtistGraph::from_rows(&[
            (1, 2, 0, 0.9),
            (2, 3, 0, 0.9),
            (3, 4, 0, 0.9),
            (1, 5, 1, 0.8),
            (5, 6, 1, 0.8),
            (6, 4, 1, 0.8),
            (1, 4, 2, 0.9),
        ]);
        let goals = HashSet::from([4]);
        let free = route_to_any(&graph, 1, &goals, 3, 7, |_| Some(0.0)).unwrap();
        let stops: Vec<i64> = free.iter().map(|s| s.0).collect();
        assert_eq!(stops, vec![1, 2, 3, 4], "at least three hops");
        assert_eq!(free[1].1, Some("tidal"));
        let shy = route_to_any(&graph, 1, &goals, 3, 7, |id| {
            Some(if id == 2 { 1.0 } else { 0.0 })
        })
        .unwrap();
        let stops: Vec<i64> = shy.iter().map(|s| s.0).collect();
        assert_eq!(stops, vec![1, 5, 6, 4], "a known artist costs more");
        assert!(
            route_to_any(&graph, 1, &goals, 3, 7, |id| (id != 3 && id != 6)
                .then_some(0.0))
            .is_none()
        );
        assert!(
            route_to_any(&graph, 1, &goals, 4, 7, |_| Some(0.0)).is_none(),
            "no simple path that long"
        );
    }

    #[test]
    fn destinations_go_least_seen_then_nearer_then_bigger() {
        let scene = |id: i64, videos: i64, artists: i64, seen: i64| SceneSeen {
            id,
            name: format!("Scene {id}"),
            videos,
            artists,
            seen_videos: seen,
        };
        let scenes = vec![
            scene(1, 100, 10, 0),
            scene(2, 100, 10, 5),
            scene(3, 100, 10, 0),
            scene(4, 500, 10, 0),
            scene(5, 100, 4, 0),
            scene(6, 100, 10, 0),
            scene(7, 1000, 10, 10),
        ];
        let distance = |id: i64| match id {
            1 | 2 | 5 | 7 => Some(1),
            3 | 4 => Some(3),
            _ => None,
        };
        assert_eq!(destinations(&scenes, Some(1), distance), vec![4, 3, 7, 2]);
    }

    #[test]
    fn picks_skip_never_and_prefer_outside_avoid() {
        let ranked = [1, 2, 3];
        let none = HashSet::new();
        assert_eq!(pick_video(&ranked, &HashSet::from([1]), &none), Some(2));
        assert_eq!(
            pick_video(&ranked, &HashSet::from([1, 2, 3]), &none),
            Some(1)
        );
        assert_eq!(pick_video(&ranked, &none, &HashSet::from([1])), Some(2));
        assert_eq!(
            pick_video(&ranked, &HashSet::from([2, 3]), &HashSet::from([1])),
            Some(2)
        );
        assert_eq!(pick_video(&[], &none, &none), None);
    }
}
