//! Journeys across the Atlas (GET /api/videos/atlas/journey).
//!
//! - Collab chain: from one artist to another where every step is a real
//!   video both artists are on (a video by A that features B, or the
//!   reverse). Fewest steps, ties to the more popular link videos.
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

/// A collab chain takes at most this many steps.
pub const COLLAB_MAX_STEPS: usize = 8;
/// Somewhere new runs four to eight stops, the start included.
pub const NEW_MIN_STOPS: usize = 4;
pub const NEW_MAX_STOPS: usize = 8;
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
/// Link videos kept per pair: the best, and a second so a chain through a
/// video's main artist never plays that video twice.
const LINKS_PER_PAIR: usize = 2;
/// A programmable link outweighs any popularity when chains tie on steps.
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

    /// What a chain gains by taking this link; ties on steps go to the
    /// larger sum.
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

/// One stop of a chain: the artist and the link from the stop before
/// (None for the start).
pub type CollabStop = (i64, Option<Link>);

/// Undirected graph of artists who share a video: the main artist linked to
/// each featured artist, nodes by ascending TIDAL id, CSR rows.
#[derive(Debug, Default)]
pub struct CollabGraph {
    ids: Vec<i64>,
    offsets: Vec<usize>,
    targets: Vec<u32>,
    /// Per CSR entry, its pair's index into `links`.
    pairs: Vec<u32>,
    /// Per pair, its best links (at most `LINKS_PER_PAIR`), best first.
    links: Vec<Vec<Link>>,
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
            list.dedup_by_key(|link| link.video_id);
            list.truncate(LINKS_PER_PAIR);
            links.push(list);
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

    /// The chain from `from` to `to` with the fewest steps (at most
    /// `max_steps`), ties to the larger sum of link values, through artists
    /// `allowed` accepts (the endpoints always pass). No artist and no video
    /// appears twice: a video links its main artist to each featured one, so
    /// it could only repeat on the two steps through its main artist, and the
    /// search runs over (arriving pair, link) states to rule that out.
    ///
    /// Hops to the goal (a BFS that ignores the video rule) bound the search
    /// to the shortest corridor; a longer bound is tried only when the video
    /// rule blocks every chain that short.
    pub fn chain(
        &self,
        from: i64,
        to: i64,
        max_steps: usize,
        allowed: impl Fn(i64) -> bool,
    ) -> Option<Vec<CollabStop>> {
        let start = self.index_of(from)?;
        let goal = self.index_of(to)?;
        if start == goal {
            return None;
        }
        let max_steps = max_steps.min(u8::MAX as usize - 1);
        let mut to_goal = vec![u8::MAX; self.ids.len()];
        to_goal[goal] = 0;
        let mut frontier = vec![goal];
        for depth in 1..=max_steps {
            let mut next = Vec::new();
            for node in frontier {
                for &target in &self.targets[self.offsets[node]..self.offsets[node + 1]] {
                    let target = target as usize;
                    if to_goal[target] != u8::MAX {
                        continue;
                    }
                    // The start ends a chain; it is never passed through.
                    if target == start || allowed(self.ids[target]) {
                        to_goal[target] = depth as u8;
                        if target != start {
                            next.push(target);
                        }
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let shortest = to_goal[start] as usize;
        (shortest..=max_steps).find_map(|bound| self.search(start, goal, bound, &to_goal, &allowed))
    }

    /// Dijkstra over (arriving pair, link) states on (steps, -value), never
    /// leaving the corridor where steps so far plus hops to the goal fit in
    /// `bound`.
    fn search(
        &self,
        start: usize,
        goal: usize,
        bound: usize,
        to_goal: &[u8],
        allowed: &impl Fn(i64) -> bool,
    ) -> Option<Vec<CollabStop>> {
        #[derive(PartialEq, Eq)]
        struct Open {
            steps: u32,
            value: i64,
            state: usize,
        }
        impl Ord for Open {
            fn cmp(&self, other: &Self) -> Ordering {
                other
                    .steps
                    .cmp(&self.steps)
                    .then(self.value.cmp(&other.value))
                    .then(other.state.cmp(&self.state))
            }
        }
        impl PartialOrd for Open {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        const NONE: usize = usize::MAX;
        let better = |a: (u32, i64), b: (u32, i64)| a.0 < b.0 || (a.0 == b.0 && a.1 > b.1);

        // State = CSR entry * LINKS_PER_PAIR + link slot: arrived over that
        // entry's pair with that link.
        let states = self.targets.len() * LINKS_PER_PAIR;
        let mut best: Vec<(u32, i64)> = vec![(u32::MAX, i64::MIN); states];
        let mut came: Vec<usize> = vec![NONE; states];
        let mut heap = BinaryHeap::new();
        let node_of = |state: usize| self.targets[state / LINKS_PER_PAIR] as usize;
        let link_of = |state: usize| {
            self.links[self.pairs[state / LINKS_PER_PAIR] as usize][state % LINKS_PER_PAIR]
        };
        // Whether `node` is already a stop on the way to `state`.
        let on_path = |came: &[usize], mut state: usize, node: usize| {
            while state != NONE {
                if node_of(state) == node {
                    return true;
                }
                state = came[state];
            }
            node == start
        };
        let expand = |node: usize,
                      arrived: usize,
                      cost: (u32, i64),
                      best: &mut Vec<(u32, i64)>,
                      came: &mut Vec<usize>,
                      heap: &mut BinaryHeap<Open>| {
            let incoming = (arrived != NONE).then(|| link_of(arrived).video_id);
            for entry in self.offsets[node]..self.offsets[node + 1] {
                let next = self.targets[entry] as usize;
                let left = to_goal[next];
                if left == u8::MAX
                    || cost.0 as usize + 1 + left as usize > bound
                    || (next != goal && !allowed(self.ids[next]))
                    || (arrived == NONE && next == start)
                    || (arrived != NONE && on_path(came, arrived, next))
                {
                    continue;
                }
                let pair = &self.links[self.pairs[entry] as usize];
                for (slot, link) in pair.iter().enumerate() {
                    if incoming == Some(link.video_id) {
                        continue;
                    }
                    let state = entry * LINKS_PER_PAIR + slot;
                    let total = (cost.0 + 1, cost.1 + link.value());
                    if better(total, best[state]) {
                        best[state] = total;
                        came[state] = arrived;
                        heap.push(Open {
                            steps: total.0,
                            value: total.1,
                            state,
                        });
                    }
                }
            }
        };
        expand(start, NONE, (0, 0), &mut best, &mut came, &mut heap);
        while let Some(Open {
            steps,
            value,
            state,
        }) = heap.pop()
        {
            if (steps, value) != best[state] {
                continue;
            }
            let node = node_of(state);
            if node == goal {
                let mut path = Vec::with_capacity(steps as usize + 1);
                let mut at = state;
                while at != NONE {
                    path.push((self.ids[node_of(at)], Some(link_of(at))));
                    at = came[at];
                }
                path.push((self.ids[start], None));
                path.reverse();
                return Some(path);
            }
            if steps as usize >= bound {
                continue;
            }
            expand(node, state, (steps, value), &mut best, &mut came, &mut heap);
        }
        None
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

    pub fn on_map(&self, id: i64) -> bool {
        self.artists.contains_key(&id)
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

/// Plan somewhere new from `from`: to `scene` when given, else to the best
/// destination with a path. Every stop after the start is on the map with a
/// programmable video; stops the listener knows cost more. Err is the
/// reason shown when nothing is found.
pub fn plan_new(
    graph: &ArtistGraph,
    facts: &MapFacts,
    light: &LightMap,
    scenes: &[SceneSeen],
    from: i64,
    scene: Option<i64>,
) -> Result<NewTrip, String> {
    let max_hops = NEW_MAX_STOPS - 1;
    let allowed = |id: i64| facts.playable(id);
    let stop_toll = |id: i64| {
        allowed(id).then(|| match light.light(id, facts.charted(id)) {
            light::LIT => LIT_TOLL,
            light::WARM => WARM_TOLL,
            _ => 0.0,
        })
    };
    let goals_of = |scene: i64| -> HashSet<i64> {
        facts
            .tops
            .get(&scene)
            .map(|tops| tops.iter().copied().filter(|id| *id != from).collect())
            .unwrap_or_default()
    };
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
            let order = destinations(scenes, facts.scene(from), |scene| {
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
        if let Some(path) =
            route_to_any(graph, from, &goals, NEW_MIN_STOPS - 1, max_hops, stop_toll)
        {
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

    fn ids(path: &[CollabStop]) -> Vec<i64> {
        path.iter().map(|(id, _)| *id).collect()
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
        let links: Vec<i64> = graph.links[0].iter().map(|l| l.video_id).collect();
        assert_eq!(links, vec![3, 2], "official beats popular, clips fall off");
    }

    #[test]
    fn chains_take_the_fewest_steps_then_the_popular_links() {
        let graph = CollabGraph::from_videos(&[
            video(1, 1, &[2], 1),
            video(2, 3, &[2], 1),
            video(3, 1, &[4], 50),
            video(4, 4, &[3], 50),
            video(5, 1, &[5], 99),
            video(6, 5, &[6], 99),
            video(7, 6, &[3], 99),
        ]);
        let path = graph.chain(1, 3, 8, |_| true).unwrap();
        assert_eq!(ids(&path), vec![1, 4, 3]);
        assert_eq!(path[1].1.unwrap().video_id, 3);
        assert_eq!(path[2].1.unwrap().video_id, 4);
        let around = graph.chain(1, 3, 8, |id| id != 4 && id != 2).unwrap();
        assert_eq!(ids(&around), vec![1, 5, 6, 3]);
        assert!(graph.chain(1, 3, 1, |_| true).is_none());
        assert!(graph.chain(1, 99, 8, |_| true).is_none());
    }

    #[test]
    fn a_chain_never_plays_a_video_twice_through_its_main_artist() {
        let graph =
            CollabGraph::from_videos(&[video(1, 51, &[52, 53], 80), video(2, 51, &[53], 10)]);
        let path = graph.chain(52, 53, 8, |_| true).unwrap();
        assert_eq!(ids(&path), vec![52, 51, 53]);
        assert_eq!(path[1].1.unwrap().video_id, 1);
        assert_eq!(path[2].1.unwrap().video_id, 2);
        let only = CollabGraph::from_videos(&[video(1, 51, &[52, 53], 80)]);
        assert!(only.chain(52, 53, 8, |_| true).is_none());
    }

    #[test]
    fn chains_stop_at_the_step_cap() {
        let videos: Vec<CollabVideo> = (0..9).map(|i| video(i, 100 + i, &[101 + i], 1)).collect();
        let graph = CollabGraph::from_videos(&videos);
        assert_eq!(graph.chain(100, 108, 8, |_| true).unwrap().len(), 9);
        assert!(graph.chain(100, 109, 8, |_| true).is_none());
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
