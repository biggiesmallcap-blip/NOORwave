//! Scene finder: Louvain communities over the weighted artist graph,
//! giants split, small ones dissolved, stable ids across rebuilds, exemplar
//! names, a soft second scene. Writes video_scenes and the scene columns of
//! video_artist_facets (scene, second scene, confidence, charted, name,
//! videos).
//!
//! Also home of the merged undirected artist graph every Atlas stage and
//! endpoint reads (`ArtistGraph`, cached like the discovery graph).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use super::louvain::{self, Csr};
use crate::services::video_discovery::names::name_key;

/// TIDAL's "Various Artists": compilations, never a scene member.
pub const VARIOUS_ARTISTS_ID: i64 = 2935;
/// Videos shorter than this are clips, never programmed.
pub const MIN_PROGRAMMABLE_S: i64 = 90;
/// Graph edge providers, indexed by `ArtistGraph::sources`.
pub const EDGE_SOURCES: [&str; 4] = ["tidal", "lastfm", "featured", "colist"];

const GRAPH_TTL: Duration = Duration::from_secs(600);
const NEXT_ID_KEY: &str = "video_intel.scene_next_id";
const EXEMPLARS: usize = 4;
const STABLE_JACCARD: f64 = 0.5;
const SECOND_SCENE_SHARE: f64 = 0.3;
const ROUTE_HOP_TOLL: f64 = 0.05;

/// Tuning for `find_scenes`; the defaults are what the real catalog needs
/// (about 80 scenes from 15k artists).
#[derive(Debug, Clone, Copy)]
pub struct SceneParams {
    pub resolution: f64,
    /// Communities with more videos than this are split again.
    pub split_over: i64,
    pub split_resolution: f64,
    /// A piece of a split giant stands on its own from this many videos.
    pub split_piece_min: i64,
    /// Scenes under this many videos dissolve into their neighbours.
    pub dissolve_under: i64,
    pub dissolve_rounds: usize,
}

impl Default for SceneParams {
    fn default() -> Self {
        Self {
            resolution: 1.0,
            split_over: 5_000,
            split_resolution: 2.0,
            split_piece_min: 600,
            dissolve_under: 300,
            dissolve_rounds: 6,
        }
    }
}

/// Display names for the big scenes, keyed by a signature artist's TIDAL id.
/// Derived from the 2026-10-14 catalog snapshot (77 scenes): each artist is
/// a top-ranked member of the scene it names, checked against the scene's
/// 30 biggest artists. A scene holding several signature artists takes the
/// name of its highest-ranked one; a name already taken by a bigger scene,
/// and every scene without a signature artist, falls back to exemplars.
pub const CURATED_NAMES: &[(i64, &str)] = &[
    // Pop
    (4761957, "Top 40"),     // The Weeknd, Ariana Grande, Justin Bieber, Drake
    (1532, "2000s Pop"),     // Britney Spears, Shakira, Rihanna, Christina Aguilera
    (1510, "2000s Pop"),     // Christina Aguilera
    (2573, "Dance Pop"),     // Kylie Minogue, Madonna, Dua Lipa, Robyn
    (4931831, "Alt Pop"),    // Lorde, Olivia Rodrigo, Lana Del Rey, Billie Eilish
    (8036491, "K-pop"),      // BLACKPINK, NCT 127, i-dle, SEVENTEEN
    (362, "Crooners"),       // Frank Sinatra, Michael Buble, Bing Crosby, Norah Jones
    (12263, "Crooners"),     // Michael Buble
    (637, "Funk and Disco"), // Earth, Wind & Fire, Prince, ABBA, Donna Summer
    (15796, "Modern Soul"),  // Amy Winehouse, Alicia Keys, Adele, Olivia Dean
    (145, "R&B Classics"),   // Luther Vandross, Whitney Houston, Sade, Maxwell
    (460, "Songwriters"),    // John Mayer, Ed Sheeran, Brandi Carlile, Jack Johnson
    (4524636, "Indie Folk"), // The Lumineers, George Ezra, Bon Iver, Ben Howard
    (17364, "Indie"),        // Arcade Fire, Metronomy, Glass Animals, Tame Impala
    // Rock
    (8877, "New Wave and 80s"), // Depeche Mode, New Order, Eurythmics, a-ha
    (573, "Classic Rock"),      // The Rolling Stones, Springsteen, Dylan, Pink Floyd
    (3634161, "Pop Rock Legends"), // The Beatles, Billy Joel, Elton John, Fleetwood Mac
    (614, "Soft Rock"),         // James Taylor, Rod Stewart, Simon & Garfunkel
    (27426, "80s Rock"),        // The Police, Foreigner, The Cars, Huey Lewis
    (8405, "Hard Rock and Metal"), // Metallica, AC/DC, Iron Maiden, Megadeth
    (780, "Nu Metal"),          // Korn, Linkin Park, Deftones, Disturbed
    (19368, "Alt Rock and Grunge"), // Nirvana, Foo Fighters, Soundgarden, RHCP
    (13566, "90s Alternative"), // The Goo Goo Dolls, Counting Crows, Matchbox Twenty
    (10052, "Britpop"),         // Blur, Muse, Keane, The Verve, Kasabian
    (29850, "Pop Punk"),        // blink-182, Green Day, Fall Out Boy, Sum 41
    (3523389, "Modern Rock"),   // Cage The Elephant, The Strokes, Imagine Dragons
    (59944, "Australia"),       // Missy Higgins, Powderfinger, Cold Chisel, Paul Kelly
    (4130, "Comedy Rock"),      // Weird Al Yankovic, Tenacious D, They Might Be Giants
    // Hip hop and R&B
    (7804, "Golden-Era Rap and R&B"), // JAY-Z, Nas, Mary J. Blige, Biggie, Snoop Dogg
    (1003, "Golden-Era Rap and R&B"), // Nas
    (3618971, "Trap Now"),            // Future, Lil Baby, Travis Scott, 21 Savage
    (7272766, "Street Rap"),          // G Herbo, Lil Durk, Polo G, Tee Grizzley
    (27518, "Dirty South"),           // Lil Wayne, 2 Chainz, T.I., Rick Ross
    (3816041, "New School Rap"),      // Kendrick Lamar, A$AP Rocky, Tyler, J. Cole
    (1826, "Alternative Hip Hop"),    // A Tribe Called Quest, De La Soul, Common, Atmosphere
    (4377, "Bay Area Rap"),           // E-40, Larry June, SOB X RBE, Philthy Rich
    (7067135, "NY Underground"),      // Westside Gunn, Benny The Butcher, Conway
    (3285, "2000s Hip Hop and R&B"),  // T-Pain, Ne-Yo, Akon, Nelly, Ludacris
    (3566984, "UK Rap and Afrobeats"), // Skepta, Headie One, Mr Eazi, Craig David
    // Electronic
    (27343, "Dance Hits"), // David Guetta, Calvin Harris, Kygo, Disclosure
    (4715335, "Main Stage EDM"), // Martin Garrix, Avicii, Hardwell, Steve Aoki
    (1093, "Euro Dance and Rave"), // Faithless, Scooter, Ace of Base, The Chemical Brothers
    (9056, "Trip-Hop and Downtempo"), // Massive Attack, Gorillaz, Bonobo, Air
    (3541504, "Drum and Bass"), // Chase & Status, Wilkinson, Sub Focus, Pendulum
    // Country, gospel, roots
    (4153, "Nashville"), // Kenny Chesney, Keith Urban, Luke Combs, Blake Shelton
    (5331, "New Country"), // Miranda Lambert, Maren Morris, Little Big Town
    (156, "Country Roads"), // Johnny Cash, Dolly Parton, Loretta Lynn
    (24902, "Honky Tonk"), // George Strait, Alan Jackson, Dwight Yoakam
    (9431, "Worship"),   // Chris Tomlin, Tasha Cobbs Leonard, Matt Maher
    (3540852, "Gospel and Bluegrass"), // Bill & Gloria Gaither, Oak Ridge Boys, Bill Monroe
    (5927, "Reggae and Dancehall"), // Bob Marley & The Wailers, Sean Paul, Shaggy, UB40
    (15406, "Reggae and Dancehall"), // Sean Paul
    (55, "Jazz"),        // Miles Davis, Herbie Hancock, John Coltrane
    (159, "Classical"),  // Yo-Yo Ma, Lang Lang, Pavarotti, Berliner Philharmoniker
    (1669, "Scores and Neo-Classical"), // Ludovico Einaudi, Max Richter, Hans Zimmer
    // Latin
    (16994, "Latin Pop"), // Enrique Iglesias, Ricardo Arjona, Thalia, Laura Pausini
    (9502, "Reggaeton"),  // Daddy Yankee, Bad Bunny, Feid, Anuel AA
    (17110, "Latin Urbano"), // Don Omar, Farruko, Rauw Alejandro, Nicky Jam
    (5237820, "Medellin"), // KAROL G, Maluma, Blessd, Reykon
    (7125934, "Latin Trap"), // Eladio Carrion, Jon Z, Cazzu
    (3969346, "Salsa and Bachata"), // Romeo Santos, Marc Anthony, Celia Cruz
    (3964060, "Cumbia"),  // Los Angeles Azules, Grupo Canaveral, Raymix
    (3628717, "Colombia"), // Carlos Vives, Camilo, Piso 21, Andres Cepeda
    (4539244, "Spanish Pop"), // Pablo Alboran, David Bisbal, Aitana, Sabina
    (3618024, "Latin Alternative"), // Natalia Lafourcade, Jorge Drexler, Lila Downs
    (3579773, "Rock en Espanol"), // Panteon Rococo, Molotov, Aterciopelados
    (4588, "Cantautores"), // Joan Manuel Serrat, Mercedes Sosa, Fito Paez
    (24900, "90s Pop en Espanol"), // Paulina Rubio, Fey, OV7, Gloria Trevi
    (4893770, "Mexican Rap"), // C-Kan, Santa Fe Klan, MC Davo
    (13536, "MPB and Bossa Nova"), // Caetano Veloso, Gal Costa, Gilberto Gil, Jobim
    (3665921, "Pagode and Sertanejo"), // Sorriso Maroto, Ferrugem, Pericles, Jorge & Mateus
    (4906194, "Brazilian Pop"), // Anitta, Alexandre Pires, Luisa Sonza
    // Elsewhere
    (3121, "Francophone"), // Indochine, Charles Aznavour, Soprano, Patrick Bruel
    (2008, "Italia"),      // Vasco Rossi, Lucio Dalla, Biagio Antonacci
    (4106879, "Finland"),  // JVG, Kube, Jenni Vartiainen, Cheek
    (34445, "Bollywood"),  // A.R. Rahman, Rahat Fateh Ali Khan, Asha Bhosle
];

// --------------------------------------------------------------- graph

/// The artist graph as Louvain, light and routes need it: undirected, one
/// edge per pair with every provider's evidence merged by noisy-OR, nodes
/// indexed by ascending TIDAL id. Various Artists is left out.
#[derive(Debug, Default)]
pub struct ArtistGraph {
    pub ids: Vec<i64>,
    pub csr: Csr,
    /// Index into `EDGE_SOURCES` of each CSR entry's strongest provider.
    pub sources: Vec<u8>,
}

fn source_index(source: &str) -> Option<u8> {
    EDGE_SOURCES
        .iter()
        .position(|known| *known == source)
        .map(|index| index as u8)
}

impl ArtistGraph {
    /// Rows of (seed, related, source index, weight) in any direction.
    pub fn from_rows(rows: &[(i64, i64, u8, f64)]) -> Self {
        // (1 - noisy-OR so far, strongest source, its weight)
        let mut pairs: BTreeMap<(i64, i64), (f64, u8, f64)> = BTreeMap::new();
        for &(a, b, source, weight) in rows {
            if a <= 0
                || b <= 0
                || a == b
                || a == VARIOUS_ARTISTS_ID
                || b == VARIOUS_ARTISTS_ID
                || !weight.is_finite()
                || weight <= 0.0
            {
                continue;
            }
            let weight = weight.min(1.0);
            let entry = pairs
                .entry((a.min(b), a.max(b)))
                .or_insert((1.0, source, 0.0));
            entry.0 *= 1.0 - weight;
            if weight > entry.2 || (weight == entry.2 && source < entry.1) {
                entry.1 = source;
                entry.2 = weight;
            }
        }
        let mut ids: Vec<i64> = pairs.keys().flat_map(|(a, b)| [*a, *b]).collect();
        ids.sort_unstable();
        ids.dedup();
        let index = |id: i64| ids.binary_search(&id).map(|i| i as u32).unwrap_or(0);
        let edges: Vec<(u32, u32, f64)> = pairs
            .iter()
            .map(|((a, b), (rest, _, _))| (index(*a), index(*b), 1.0 - rest))
            .collect();
        let csr = Csr::from_edges(ids.len(), &edges);
        let mut sources = Vec::with_capacity(csr.targets.len());
        for node in 0..csr.len() {
            for &target in csr.row(node).0 {
                let (a, b) = (ids[node], ids[target as usize]);
                sources.push(pairs.get(&(a.min(b), a.max(b))).map_or(0, |p| p.1));
            }
        }
        Self { ids, csr, sources }
    }

    pub fn load(conn: &Connection) -> Result<Self> {
        let mut stmt = conn.prepare(
            "SELECT seed_tidal_id, related_tidal_id, source, weight FROM video_related_artists
              WHERE source IN ('tidal', 'lastfm', 'featured', 'colist')",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, f64>(3)?,
                ))
            })?
            .filter_map(|row| match row {
                Ok((a, b, source, weight)) => {
                    source_index(&source).map(|source| Ok((a, b, source, weight)))
                }
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::from_rows(&rows))
    }

    pub fn index_of(&self, id: i64) -> Option<usize> {
        self.ids.binary_search(&id).ok()
    }

    /// Distinct neighbours of a node.
    pub fn degree(&self, id: i64) -> usize {
        self.index_of(id).map_or(0, |i| self.csr.row(i).0.len())
    }

    /// (neighbour id, merged weight, strongest provider).
    pub fn neighbors(&self, id: i64) -> impl Iterator<Item = (i64, f64, &'static str)> + '_ {
        let range = self
            .index_of(id)
            .map_or(0..0, |i| self.csr.offsets[i]..self.csr.offsets[i + 1]);
        range.map(move |entry| {
            (
                self.ids[self.csr.targets[entry] as usize],
                self.csr.weights[entry],
                EDGE_SOURCES[self.sources[entry] as usize],
            )
        })
    }

    /// Cheapest path from `from` to `to` by -ln(weight) plus a small toll per
    /// hop, at most `max_hops` edges, through nodes `allowed` accepts (the
    /// endpoints always pass). Dijkstra over (node, hops) states, so the hop
    /// cap never hides a cheaper longer path that still fits. Each stop
    /// carries the provider of the edge that reached it.
    pub fn route(
        &self,
        from: i64,
        to: i64,
        max_hops: usize,
        allowed: impl Fn(i64) -> bool,
    ) -> Option<Vec<(i64, Option<&'static str>)>> {
        use std::cmp::Ordering;
        use std::collections::BinaryHeap;

        #[derive(PartialEq)]
        struct State {
            cost: f64,
            hops: usize,
            node: u32,
        }
        impl Eq for State {}
        impl Ord for State {
            fn cmp(&self, other: &Self) -> Ordering {
                other
                    .cost
                    .total_cmp(&self.cost)
                    .then(other.hops.cmp(&self.hops))
                    .then(other.node.cmp(&self.node))
            }
        }
        impl PartialOrd for State {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let start = self.index_of(from)?;
        let goal = self.index_of(to)?;
        if start == goal {
            return Some(vec![(from, None)]);
        }
        let layers = max_hops + 1;
        let n = self.ids.len();
        let mut best = vec![f64::INFINITY; n * layers];
        // (previous node, CSR entry used to arrive)
        let mut came: Vec<(u32, usize)> = vec![(u32::MAX, usize::MAX); n * layers];
        let mut heap = BinaryHeap::new();
        best[start * layers] = 0.0;
        heap.push(State {
            cost: 0.0,
            hops: 0,
            node: start as u32,
        });
        while let Some(State { cost, hops, node }) = heap.pop() {
            let node = node as usize;
            if cost > best[node * layers + hops] {
                continue;
            }
            if node == goal {
                let mut path = Vec::with_capacity(hops + 1);
                let (mut at, mut layer) = (node, hops);
                while layer > 0 {
                    let (previous, entry) = came[at * layers + layer];
                    path.push((
                        self.ids[at],
                        Some(EDGE_SOURCES[self.sources[entry] as usize]),
                    ));
                    at = previous as usize;
                    layer -= 1;
                }
                path.push((self.ids[at], None));
                path.reverse();
                return Some(path);
            }
            if hops == max_hops {
                continue;
            }
            for entry in self.csr.offsets[node]..self.csr.offsets[node + 1] {
                let next = self.csr.targets[entry] as usize;
                if next == node || (next != goal && !allowed(self.ids[next])) {
                    continue;
                }
                let step = -self.csr.weights[entry].clamp(1e-3, 1.0).ln() + ROUTE_HOP_TOLL;
                let total = cost + step;
                let layer = hops + 1;
                // Dominated when an equal-or-fewer-hop arrival is no dearer.
                if (0..=layer).any(|h| best[next * layers + h] <= total) {
                    continue;
                }
                best[next * layers + layer] = total;
                came[next * layers + layer] = (node as u32, entry);
                heap.push(State {
                    cost: total,
                    hops: layer,
                    node: next as u32,
                });
            }
        }
        None
    }
}

/// (database file, loaded at, graph)
type GraphCache = Option<(String, Instant, Arc<ArtistGraph>)>;
static GRAPH_CACHE: Mutex<GraphCache> = Mutex::new(None);

/// The database file a connection reads, None for in-memory databases.
fn cache_key(conn: &Connection) -> Option<String> {
    conn.path()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

/// The shared merged graph, reloaded every 10 minutes. The scene build
/// refreshes it, so the map and its cards read the graph the scenes came
/// from. In-memory databases (tests) are never cached.
pub fn cached_graph(conn: &Connection) -> Result<Arc<ArtistGraph>> {
    let key = cache_key(conn);
    if let Some(key) = &key
        && let Ok(cache) = GRAPH_CACHE.lock()
        && let Some((cached_key, built, graph)) = cache.as_ref()
        && cached_key == key
        && built.elapsed() < GRAPH_TTL
    {
        return Ok(graph.clone());
    }
    let graph = Arc::new(ArtistGraph::load(conn)?);
    store_graph(key, graph.clone());
    Ok(graph)
}

fn store_graph(key: Option<String>, graph: Arc<ArtistGraph>) {
    if let Some(key) = key
        && let Ok(mut cache) = GRAPH_CACHE.lock()
    {
        *cache = Some((key, Instant::now(), graph));
    }
}

// --------------------------------------------------------------- artists

/// One artist's share of the catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtistVideos {
    pub name: String,
    pub videos: i64,
    pub programmable: i64,
    pub programmable_s: i64,
}

/// Whether the catalog stage has classified videos yet. Until it has, every
/// video of 90 s or more is treated as programmable.
pub fn facets_ready(conn: &Connection) -> Result<bool> {
    Ok(
        conn.query_row("SELECT EXISTS (SELECT 1 FROM video_facets)", [], |row| {
            row.get(0)
        })?,
    )
}

/// Whether a name reads as a compilation credit rather than an artist.
pub fn is_various_artists(id: i64, name: &str) -> bool {
    id == VARIOUS_ARTISTS_ID || name_key(name) == "various artists"
}

/// Every artist with catalog videos, keyed by TIDAL id. Various Artists is
/// left out.
pub fn load_artist_videos(conn: &Connection) -> Result<BTreeMap<i64, ArtistVideos>> {
    let mut out = BTreeMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, MAX(artist_name), COUNT(*),
                    SUM(CASE WHEN d IS NULL OR d >= ?1 THEN 1 ELSE 0 END),
                    SUM(CASE WHEN d IS NULL OR d >= ?1 THEN COALESCE(d, 0) ELSE 0 END)
               FROM (SELECT artist_tidal_id, artist_name,
                            COALESCE(json_extract(item_json, '$.duration_s'),
                                     json_extract(item_json, '$.duration_ms') / 1000) AS d
                       FROM video_catalog WHERE artist_tidal_id > 0)
              GROUP BY artist_tidal_id",
        )?;
        let rows = stmt.query_map([MIN_PROGRAMMABLE_S], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                ArtistVideos {
                    name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    videos: row.get(2)?,
                    programmable: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    programmable_s: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0) as i64,
                },
            ))
        })?;
        for row in rows {
            let (id, artist) = row?;
            if !is_various_artists(id, &artist.name) {
                out.insert(id, artist);
            }
        }
    }
    if facets_ready(conn)? {
        for artist in out.values_mut() {
            artist.programmable = 0;
            artist.programmable_s = 0;
        }
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, SUM(programmable),
                    SUM(CASE WHEN programmable = 1 THEN COALESCE(duration_s, 0) ELSE 0 END)
               FROM video_facets WHERE artist_tidal_id > 0 GROUP BY artist_tidal_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })?;
        for row in rows {
            let (id, programmable, seconds) = row?;
            if let Some(artist) = out.get_mut(&id) {
                artist.programmable = programmable;
                artist.programmable_s = seconds;
            }
        }
    }
    let unnamed: Vec<i64> = out
        .iter()
        .filter(|(_, artist)| artist.name.trim().is_empty())
        .map(|(id, _)| *id)
        .collect();
    for id in unnamed {
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM video_artist_state WHERE artist_tidal_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if let (Some(name), Some(artist)) = (name, out.get_mut(&id)) {
            artist.name = name;
        }
    }
    Ok(out)
}

fn load_popularity(conn: &Connection) -> Result<HashMap<i64, i64>> {
    let mut stmt = conn.prepare(
        "SELECT artist_tidal_id, popularity FROM video_artist_state WHERE popularity IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<HashMap<i64, i64>, _>>()?;
    Ok(rows)
}

/// How strongly an artist stands for its scene: prolific, well connected
/// and well known. Hubs sit at a disk's centre and name the scene.
pub fn exemplar_score(videos: i64, degree: usize, popularity: Option<i64>) -> f64 {
    let videos = videos.max(0) as f64;
    let popularity = popularity.unwrap_or(0).clamp(0, 100) as f64;
    videos.sqrt() * (1.0 + degree as f64).ln() * (0.25 + popularity / 100.0)
}

// --------------------------------------------------------------- scenes

/// Scene label per graph node, None for nodes that belong to no scene.
/// `videos[i]` is node i's catalog videos. Louvain at the base resolution;
/// communities over `split_over` videos are re-run alone at the split
/// resolution and their pieces under `split_piece_min` let go; then every
/// node in a scene under `dissolve_under` videos (or in none) joins the big
/// scene its edges point to most, round after round. Whatever cannot join
/// anything stays out.
pub fn find_scenes(graph: &Csr, videos: &[i64], params: &SceneParams) -> Vec<Option<u32>> {
    let n = graph.len();
    let base = louvain::louvain(graph, params.resolution);
    let communities = base.iter().map(|c| *c as usize + 1).max().unwrap_or(0);
    let mut members: Vec<Vec<u32>> = vec![Vec::new(); communities];
    let mut community_videos = vec![0i64; communities];
    for (node, community) in base.iter().enumerate() {
        members[*community as usize].push(node as u32);
        community_videos[*community as usize] += videos[node];
    }
    let mut scene: Vec<Option<u32>> = vec![None; n];
    let mut next = 0u32;
    for (community, nodes) in members.iter().enumerate() {
        if community_videos[community] > params.split_over {
            let parts = louvain::louvain(&graph.induced(nodes), params.split_resolution);
            let count = parts.iter().map(|p| *p as usize + 1).max().unwrap_or(0);
            let mut part_videos = vec![0i64; count];
            for (local, part) in parts.iter().enumerate() {
                part_videos[*part as usize] += videos[nodes[local] as usize];
            }
            let mut labels = vec![None; count];
            for (part, label) in labels.iter_mut().enumerate() {
                if part_videos[part] >= params.split_piece_min {
                    *label = Some(next);
                    next += 1;
                }
            }
            for (local, part) in parts.iter().enumerate() {
                scene[nodes[local] as usize] = labels[*part as usize];
            }
        } else {
            for &node in nodes {
                scene[node as usize] = Some(next);
            }
            next += 1;
        }
    }
    let labels = next as usize;
    let scene_videos = |scene: &[Option<u32>]| {
        let mut totals = vec![0i64; labels];
        for (node, label) in scene.iter().enumerate() {
            if let Some(label) = label {
                totals[*label as usize] += videos[node];
            }
        }
        totals
    };
    let mut tally: BTreeMap<u32, f64> = BTreeMap::new();
    for _ in 0..params.dissolve_rounds {
        let totals = scene_videos(&scene);
        let big =
            |label: Option<u32>| label.is_some_and(|l| totals[l as usize] >= params.dissolve_under);
        let mut moves = Vec::new();
        for node in 0..n {
            if big(scene[node]) {
                continue;
            }
            tally.clear();
            let (targets, weights) = graph.row(node);
            for (&target, &weight) in targets.iter().zip(weights) {
                let label = scene[target as usize];
                if target as usize != node && big(label) {
                    *tally.entry(label.unwrap_or_default()).or_default() += weight;
                }
            }
            // Strongest pull wins; BTreeMap order breaks ties to the lowest label.
            let mut choice: Option<(u32, f64)> = None;
            for (&label, &weight) in &tally {
                if choice.is_none_or(|(_, best)| weight > best) {
                    choice = Some((label, weight));
                }
            }
            if let Some((label, _)) = choice {
                moves.push((node, label));
            }
        }
        if moves.is_empty() {
            break;
        }
        for (node, label) in moves {
            scene[node] = Some(label);
        }
    }
    let totals = scene_videos(&scene);
    for label in scene.iter_mut() {
        if label.is_some_and(|l| totals[l as usize] < params.dissolve_under) {
            *label = None;
        }
    }
    scene
}

/// A scene as the previous build left it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviousScene {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
    pub hue: f64,
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub channel_no: Option<i64>,
    /// Member artists with videos, ascending.
    pub members: Vec<i64>,
}

/// For each new scene (members ascending), the index of the previous scene
/// it continues: Jaccard over members of at least 0.5, one to one, the
/// strongest overlaps claimed first.
pub fn match_previous(new: &[Vec<i64>], previous: &[PreviousScene]) -> Vec<Option<usize>> {
    let mut owner: HashMap<i64, usize> = HashMap::new();
    for (index, scene) in previous.iter().enumerate() {
        for id in &scene.members {
            owner.insert(*id, index);
        }
    }
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (new_index, members) in new.iter().enumerate() {
        let mut overlap: BTreeMap<usize, usize> = BTreeMap::new();
        for id in members {
            if let Some(old) = owner.get(id) {
                *overlap.entry(*old).or_default() += 1;
            }
        }
        for (old, shared) in overlap {
            let union = members.len() + previous[old].members.len() - shared;
            let jaccard = shared as f64 / union.max(1) as f64;
            if jaccard >= STABLE_JACCARD {
                candidates.push((jaccard, new_index, old));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut matched = vec![None; new.len()];
    let mut taken = HashSet::new();
    for (_, new_index, old) in candidates {
        if matched[new_index].is_none() && taken.insert(old) {
            matched[new_index] = Some(old);
        }
    }
    matched
}

/// "Coldplay & Arcade Fire"; a comma when either name already has an "&".
pub fn exemplar_name(names: &[String]) -> String {
    match names {
        [] => "Uncharted".to_string(),
        [one] => one.clone(),
        [a, b, ..] if a.contains('&') || b.contains('&') => format!("{a}, {b}"),
        [a, b, ..] => format!("{a} & {b}"),
    }
}

/// The curated name of the highest-ranked signature artist among `ranked`.
pub fn curated_name(ranked: &[i64]) -> Option<&'static str> {
    ranked.iter().find_map(|id| {
        CURATED_NAMES
            .iter()
            .find(|(signature, _)| signature == id)
            .map(|(_, name)| *name)
    })
}

/// One scene of a build, ready to write.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneRow {
    pub id: i64,
    pub name: String,
    pub pinned: bool,
    pub exemplars: Vec<String>,
    pub hue: f64,
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub video_count: i64,
    pub artist_count: i64,
    pub hours: f64,
    pub channel_no: Option<i64>,
    /// Member artists with videos, ranked (hubs first).
    pub ranked: Vec<i64>,
}

/// One artist's scene facts.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtistSceneRow {
    pub id: i64,
    pub name: String,
    pub videos: i64,
    pub scene: Option<i64>,
    pub scene2: Option<i64>,
    pub conf: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ScenePlan {
    pub scenes: Vec<SceneRow>,
    pub artists: Vec<ArtistSceneRow>,
    pub next_id: i64,
}

/// The whole stage as a pure function over pre-fetched data.
pub fn plan(
    graph: &ArtistGraph,
    artists: &BTreeMap<i64, ArtistVideos>,
    popularity: &HashMap<i64, i64>,
    previous: &[PreviousScene],
    next_id: i64,
    params: &SceneParams,
) -> ScenePlan {
    let videos: Vec<i64> = graph
        .ids
        .iter()
        .map(|id| artists.get(id).map_or(0, |a| a.videos))
        .collect();
    let labels = find_scenes(&graph.csr, &videos, params);

    let mut groups: BTreeMap<u32, Vec<i64>> = BTreeMap::new();
    for (node, label) in labels.iter().enumerate() {
        if let Some(label) = label
            && videos[node] > 0
        {
            groups.entry(*label).or_default().push(graph.ids[node]);
        }
    }
    let score = |id: i64| {
        exemplar_score(
            artists.get(&id).map_or(0, |a| a.videos),
            graph.degree(id),
            popularity.get(&id).copied(),
        )
    };
    struct Draft {
        label: u32,
        members: Vec<i64>,
        ranked: Vec<i64>,
        videos: i64,
        seconds: i64,
    }
    let mut drafts: Vec<Draft> = groups
        .into_iter()
        .map(|(label, members)| {
            let mut ranked = members.clone();
            ranked.sort_by(|a, b| score(*b).total_cmp(&score(*a)).then(a.cmp(b)));
            let videos = members.iter().map(|id| artists[id].videos).sum();
            let seconds = members.iter().map(|id| artists[id].programmable_s).sum();
            Draft {
                label,
                members,
                ranked,
                videos,
                seconds,
            }
        })
        .collect();
    drafts.sort_by(|a, b| {
        b.videos
            .cmp(&a.videos)
            .then(a.members[0].cmp(&b.members[0]))
    });

    let member_lists: Vec<Vec<i64>> = drafts.iter().map(|d| d.members.clone()).collect();
    let matched = match_previous(&member_lists, previous);
    let curated: HashSet<&str> = CURATED_NAMES.iter().map(|(_, name)| *name).collect();
    let mut next_id = next_id.max(previous.iter().map(|p| p.id + 1).max().unwrap_or(1));
    let mut label_to_id: HashMap<u32, i64> = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    let mut scenes = Vec::with_capacity(drafts.len());
    for (draft, matched) in drafts.iter().zip(&matched) {
        let old = matched.map(|index| &previous[index]);
        let id = match old {
            Some(old) => old.id,
            None => {
                next_id += 1;
                next_id - 1
            }
        };
        label_to_id.insert(draft.label, id);
        let exemplars: Vec<String> = draft
            .ranked
            .iter()
            .take(EXEMPLARS)
            .map(|id| artists[id].name.clone())
            .collect();
        // A hand-pinned name survives; a curated one follows the table; a
        // name a bigger scene already took falls back to exemplars.
        let (mut name, mut pinned) = match (old, curated_name(&draft.ranked)) {
            (Some(old), _) if old.pinned && !curated.contains(old.name.as_str()) => {
                (old.name.clone(), true)
            }
            (_, Some(name)) => (name.to_string(), true),
            (Some(old), None) if !old.pinned => (old.name.clone(), false),
            _ => (exemplar_name(&exemplars), false),
        };
        if !used_names.insert(name.clone()) {
            name = exemplar_name(&exemplars);
            pinned = false;
            used_names.insert(name.clone());
        }
        scenes.push(SceneRow {
            id,
            name,
            pinned,
            exemplars,
            hue: old.map_or(0.0, |o| o.hue),
            x: old.map_or(0.0, |o| o.x),
            y: old.map_or(0.0, |o| o.y),
            r: old.map_or(0.0, |o| o.r),
            video_count: draft.videos,
            artist_count: draft.members.len() as i64,
            hours: draft.seconds as f64 / 3600.0,
            channel_no: old.and_then(|o| o.channel_no),
            ranked: draft.ranked.clone(),
        });
    }

    let mut rows = Vec::with_capacity(artists.len());
    let mut tally: BTreeMap<u32, f64> = BTreeMap::new();
    for (id, artist) in artists {
        let node = graph.index_of(*id);
        let label = node.and_then(|node| labels[node]);
        let scene = label.and_then(|label| label_to_id.get(&label).copied());
        let (mut scene2, mut conf) = (None, None);
        if let (Some(node), Some(label), Some(_)) = (node, label, scene) {
            tally.clear();
            let (targets, weights) = graph.csr.row(node);
            for (&target, &weight) in targets.iter().zip(weights) {
                if let Some(other) = labels[target as usize]
                    && target as usize != node
                {
                    *tally.entry(other).or_default() += weight;
                }
            }
            let total: f64 = tally.values().sum();
            if total > 0.0 {
                conf = Some(tally.get(&label).copied().unwrap_or(0.0) / total);
                let mut runner_up: Option<(u32, f64)> = None;
                for (&other, &weight) in &tally {
                    if other != label && runner_up.is_none_or(|(_, best)| weight > best) {
                        runner_up = Some((other, weight));
                    }
                }
                if let Some((other, weight)) = runner_up
                    && weight / total >= SECOND_SCENE_SHARE
                {
                    scene2 = label_to_id.get(&other).copied();
                }
            }
        }
        rows.push(ArtistSceneRow {
            id: *id,
            name: artist.name.clone(),
            videos: artist.videos,
            scene,
            scene2,
            conf,
        });
    }
    ScenePlan {
        scenes,
        artists: rows,
        next_id,
    }
}

// --------------------------------------------------------------- storage

pub fn load_previous(conn: &Connection) -> Result<Vec<PreviousScene>> {
    let mut stmt = conn.prepare(
        "SELECT scene_id, name, pinned, hue, x, y, r, channel_no, members_json
           FROM video_scenes ORDER BY scene_id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let members: String = row.get(8)?;
            let mut members: Vec<i64> = serde_json::from_str(&members).unwrap_or_default();
            members.sort_unstable();
            members.dedup();
            Ok(PreviousScene {
                id: row.get(0)?,
                name: row.get(1)?,
                pinned: row.get::<_, i64>(2)? != 0,
                hue: row.get(3)?,
                x: row.get(4)?,
                y: row.get(5)?,
                r: row.get(6)?,
                channel_no: row.get(7)?,
                members,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn stored_next_id(conn: &Connection) -> Result<i64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            [NEXT_ID_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(1).max(1))
}

fn write(conn: &Connection, plan: &ScenePlan) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM video_scenes", [])?;
    {
        let mut insert = tx.prepare(
            "INSERT INTO video_scenes (scene_id, name, pinned, exemplars_json, tags_json, hue,
                    x, y, r, video_count, artist_count, hours, channel_no, members_json, built_at)
             VALUES (?1, ?2, ?3, ?4, '[]', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, datetime('now'))",
        )?;
        for scene in &plan.scenes {
            let mut members = scene.ranked.clone();
            members.sort_unstable();
            insert.execute(params![
                scene.id,
                scene.name,
                scene.pinned as i64,
                serde_json::to_string(&scene.exemplars)?,
                scene.hue,
                scene.x,
                scene.y,
                scene.r,
                scene.video_count,
                scene.artist_count,
                scene.hours,
                scene.channel_no,
                serde_json::to_string(&members)?,
            ])?;
        }
    }
    let existing: Vec<i64> = tx
        .prepare("SELECT artist_tidal_id FROM video_artist_facets")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let current: HashSet<i64> = plan.artists.iter().map(|a| a.id).collect();
    {
        let mut delete =
            tx.prepare("DELETE FROM video_artist_facets WHERE artist_tidal_id = ?1")?;
        for id in existing.iter().filter(|id| !current.contains(id)) {
            delete.execute([id])?;
        }
        let mut upsert = tx.prepare(
            "INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, scene2_id,
                    scene_conf, videos, charted, built_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(artist_tidal_id) DO UPDATE SET
                name = excluded.name, scene_id = excluded.scene_id,
                scene2_id = excluded.scene2_id, scene_conf = excluded.scene_conf,
                videos = excluded.videos, charted = excluded.charted,
                built_at = excluded.built_at",
        )?;
        for artist in &plan.artists {
            upsert.execute(params![
                artist.id,
                artist.name,
                artist.scene,
                artist.scene2,
                artist.conf,
                artist.videos,
                artist.scene.is_some() as i64,
            ])?;
        }
    }
    tx.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![NEXT_ID_KEY, plan.next_id.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

/// Rebuild this stage's rows. Returns the number of scenes written.
pub fn build(conn: &Connection) -> Result<usize> {
    let artists = load_artist_videos(conn)?;
    let graph = ArtistGraph::load(conn)?;
    let popularity = load_popularity(conn)?;
    let previous = load_previous(conn)?;
    let next_id = stored_next_id(conn)?;
    let plan = plan(
        &graph,
        &artists,
        &popularity,
        &previous,
        next_id,
        &SceneParams::default(),
    );
    write(conn, &plan)?;
    store_graph(cache_key(conn), Arc::new(graph));
    Ok(plan.scenes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    /// Three cliques of artists; each clique's artists have `videos` each.
    fn three_cliques(conn: &Connection, sizes: [i64; 3], videos: i64) {
        let mut id = 100;
        for (group, size) in sizes.iter().enumerate() {
            let start = id;
            for _ in 0..*size {
                for v in 0..videos {
                    conn.execute(
                        "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![
                            id * 1000 + v,
                            id,
                            format!("Artist {id}"),
                            format!(r#"{{"tidal_id":{},"title":"Song {v}","duration_s":200}}"#, id * 1000 + v)
                        ],
                    )
                    .unwrap();
                }
                id += 1;
            }
            for a in start..id {
                for b in a + 1..id {
                    conn.execute(
                        "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                         VALUES (?1, ?2, 'x', 'lastfm', 0.9)",
                        params![a, b],
                    )
                    .unwrap();
                }
            }
            if group > 0 {
                conn.execute(
                    "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                     VALUES (?1, ?2, 'x', 'tidal', 0.3)",
                    params![start - 1, start],
                )
                .unwrap();
            }
        }
    }

    fn small_params() -> SceneParams {
        SceneParams {
            split_over: 1_000_000,
            dissolve_under: 10,
            ..SceneParams::default()
        }
    }

    #[test]
    fn graph_merges_providers_and_drops_various_artists() {
        let graph = ArtistGraph::from_rows(&[
            (1, 2, 0, 0.5),
            (2, 1, 1, 0.5),
            (1, 3, 2, 0.85),
            (1, VARIOUS_ARTISTS_ID, 2, 0.85),
            (4, 4, 0, 1.0),
        ]);
        assert_eq!(graph.ids, vec![1, 2, 3]);
        let neighbors: Vec<_> = graph.neighbors(1).collect();
        assert_eq!(neighbors.len(), 2);
        assert_eq!(neighbors[0].0, 2);
        assert!((neighbors[0].1 - 0.75).abs() < 1e-12);
        assert_eq!(neighbors[0].2, "tidal", "ties go to the first provider");
        assert_eq!((neighbors[1].0, neighbors[1].2), (3, "featured"));
        assert!((neighbors[1].1 - 0.85).abs() < 1e-12);
        assert_eq!(graph.degree(1), 2);
        assert_eq!(graph.degree(99), 0);
    }

    #[test]
    fn small_scenes_dissolve_into_their_best_neighbour() {
        // Two big cliques and one pair hanging off the second clique.
        let mut edges = Vec::new();
        for (start, size) in [(0u32, 6u32), (6, 6)] {
            for a in start..start + size {
                for b in a + 1..start + size {
                    edges.push((a, b, 1.0));
                }
            }
        }
        edges.extend([(12, 13, 1.0), (12, 7, 0.4), (13, 8, 0.2), (0, 6, 0.1)]);
        let csr = Csr::from_edges(14, &edges);
        let mut videos = vec![10i64; 14];
        videos[12] = 1;
        videos[13] = 1;
        let params = SceneParams {
            dissolve_under: 20,
            ..small_params()
        };
        let scenes = find_scenes(&csr, &videos, &params);
        assert!(scenes.iter().all(Option::is_some));
        assert_eq!(scenes[12], scenes[7]);
        assert_eq!(scenes[13], scenes[7]);
        assert_ne!(scenes[0], scenes[7]);
    }

    #[test]
    fn isolated_crumbs_stay_out_and_giants_split() {
        let mut edges = Vec::new();
        for (start, size) in [(0u32, 5u32), (5, 5)] {
            for a in start..start + size {
                for b in a + 1..start + size {
                    edges.push((a, b, 1.0));
                }
            }
        }
        edges.push((4, 5, 0.6));
        edges.push((10, 11, 1.0));
        let csr = Csr::from_edges(12, &edges);
        let videos = vec![100i64; 12];
        let whole = SceneParams {
            resolution: 0.01,
            split_over: 1_000_000,
            dissolve_under: 300,
            ..SceneParams::default()
        };
        let merged = find_scenes(&csr, &videos, &whole);
        assert_eq!(merged[0], merged[9], "one community at a tiny resolution");
        assert_eq!(merged[10], None, "a crumb of 200 videos with nowhere to go");
        let split = SceneParams {
            split_over: 500,
            split_resolution: 1.0,
            split_piece_min: 300,
            ..whole
        };
        let scenes = find_scenes(&csr, &videos, &split);
        assert_ne!(
            scenes[0], scenes[9],
            "the giant splits back into its cliques"
        );
        assert_eq!(scenes[0], scenes[4]);
    }

    #[test]
    fn stable_ids_follow_the_best_overlap() {
        let previous = vec![
            PreviousScene {
                id: 7,
                members: vec![1, 2, 3, 4],
                ..Default::default()
            },
            PreviousScene {
                id: 9,
                members: vec![10, 11, 12],
                ..Default::default()
            },
        ];
        let new = vec![
            vec![10, 11, 12, 13],    // 3/4 with scene 9
            vec![1, 2, 3, 5, 6],     // 3/6 with scene 7: exactly 0.5
            vec![20, 21],            // new
            vec![1, 2, 3, 4, 5, 30], // 4/6 with scene 7: wins it
        ];
        assert_eq!(
            match_previous(&new, &previous),
            vec![Some(1), None, None, Some(0)]
        );
        assert_eq!(match_previous(&new[2..3], &[]), vec![None]);
    }

    #[test]
    fn names_join_exemplars_and_prefer_the_curated_table() {
        assert_eq!(
            exemplar_name(&["Coldplay".into(), "Arcade Fire".into(), "Blur".into()]),
            "Coldplay & Arcade Fire"
        );
        assert_eq!(
            exemplar_name(&["Simon & Garfunkel".into(), "Paul Simon".into()]),
            "Simon & Garfunkel, Paul Simon"
        );
        assert_eq!(exemplar_name(&["Solo".into()]), "Solo");
        assert_eq!(curated_name(&[-1, 3121, 5927]), Some("Francophone"));
        assert_eq!(curated_name(&[-1, -2]), None);
        let ids: HashSet<i64> = CURATED_NAMES.iter().map(|(id, _)| *id).collect();
        assert_eq!(
            ids.len(),
            CURATED_NAMES.len(),
            "one name per signature artist"
        );
        assert!(CURATED_NAMES.iter().all(|(_, name)| name.is_ascii()));
    }

    #[test]
    fn a_curated_name_goes_to_the_biggest_scene_that_claims_it() {
        // Two cliques, each holding a "Reggae and Dancehall" signature artist.
        let mut rows = Vec::new();
        let groups = [[5927i64, 901, 902, 903], [15406, 911, 912, 913]];
        for group in &groups {
            for (i, a) in group.iter().enumerate() {
                for b in &group[i + 1..] {
                    rows.push((*a, *b, 1u8, 0.9));
                }
            }
        }
        rows.push((903, 913, 0, 0.1));
        let graph = ArtistGraph::from_rows(&rows);
        let mut artists = BTreeMap::new();
        for (index, group) in groups.iter().enumerate() {
            for id in group {
                artists.insert(
                    *id,
                    ArtistVideos {
                        name: format!("A{id}"),
                        videos: if index == 0 { 50 } else { 20 },
                        ..Default::default()
                    },
                );
            }
        }
        let plan = plan(&graph, &artists, &HashMap::new(), &[], 1, &small_params());
        assert_eq!(plan.scenes.len(), 2);
        assert_eq!(plan.scenes[0].name, "Reggae and Dancehall");
        assert!(plan.scenes[0].pinned);
        assert_ne!(plan.scenes[1].name, "Reggae and Dancehall");
        assert!(!plan.scenes[1].pinned);
    }

    #[test]
    fn build_writes_scenes_and_keeps_ids_and_channels_across_rebuilds() {
        let conn = conn();
        three_cliques(&conn, [5, 5, 5], 4);
        // Below the default dissolve threshold every scene would vanish, so
        // drive the pure planner with small-catalog parameters.
        let run = |conn: &Connection| {
            let artists = load_artist_videos(conn).unwrap();
            let graph = ArtistGraph::load(conn).unwrap();
            let previous = load_previous(conn).unwrap();
            let plan = plan(
                &graph,
                &artists,
                &HashMap::new(),
                &previous,
                stored_next_id(conn).unwrap(),
                &small_params(),
            );
            write(conn, &plan).unwrap();
            plan
        };
        let first = run(&conn);
        assert_eq!(first.scenes.len(), 3);
        assert!(first.artists.iter().all(|a| a.scene.is_some()));
        let ids: Vec<i64> = first.scenes.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        conn.execute(
            "UPDATE video_scenes SET channel_no = 12, x = 40.0, y = 50.0, r = 9.0 WHERE scene_id = 2",
            [],
        )
        .unwrap();
        let second = run(&conn);
        let again: Vec<i64> = second.scenes.iter().map(|s| s.id).collect();
        assert_eq!(again, ids);
        let kept = second.scenes.iter().find(|s| s.id == 2).unwrap();
        assert_eq!(
            (kept.channel_no, kept.x, kept.y, kept.r),
            (Some(12), 40.0, 50.0, 9.0)
        );
        let (charted, rows): (i64, i64) = conn
            .query_row(
                "SELECT SUM(charted), COUNT(*) FROM video_artist_facets",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((charted, rows), (15, 15));
        // Ids are never reused: a scene with no predecessor counts up from
        // the stored next id even when lower ids are free.
        conn.execute(
            "DELETE FROM video_related_artists WHERE seed_tidal_id >= 110",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM video_scenes WHERE scene_id = 1", [])
            .unwrap();
        let third = run(&conn);
        let scene_of = |artist: i64| {
            third
                .artists
                .iter()
                .find(|a| a.id == artist)
                .and_then(|a| a.scene)
        };
        assert_eq!(scene_of(100), Some(4));
        assert_eq!(scene_of(105), Some(2));
        assert_eq!(scene_of(112), None, "lost every edge: uncharted");
        assert!(third.scenes.iter().all(|s| s.id != 1 && s.id != 3));
        assert_eq!(third.next_id, 5);
    }

    #[test]
    fn the_default_build_runs_on_an_empty_catalog() {
        let conn = conn();
        assert_eq!(build(&conn).unwrap(), 0);
        three_cliques(&conn, [3, 3, 3], 2);
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (1, ?1, 'Various Artists', '{\"tidal_id\":1,\"title\":\"x\"}')",
            [VARIOUS_ARTISTS_ID],
        )
        .unwrap();
        // Far under 300 videos: no scene, every artist uncharted, no crash.
        assert_eq!(build(&conn).unwrap(), 0);
        let (rows, charted): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(charted), 0) FROM video_artist_facets",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((rows, charted), (9, 0));
    }

    #[test]
    fn programmable_counts_fall_back_to_duration_until_facets_exist() {
        let conn = conn();
        conn.execute_batch(
            r#"INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json) VALUES
                 (1, 5, 'Five', '{"tidal_id":1,"title":"a","duration_s":200}'),
                 (2, 5, 'Five', '{"tidal_id":2,"title":"b","duration_s":30}'),
                 (3, 5, 'Five', '{"tidal_id":3,"title":"c"}');"#,
        )
        .unwrap();
        let artists = load_artist_videos(&conn).unwrap();
        assert_eq!(
            artists[&5],
            ArtistVideos {
                name: "Five".into(),
                videos: 3,
                programmable: 2,
                programmable_s: 200,
            }
        );
        conn.execute(
            "INSERT INTO video_facets (tidal_video_id, artist_tidal_id, cut, programmable, song_key, duration_s)
             VALUES (1, 5, 'official', 1, 'k', 200), (3, 5, 'interview', 0, 'k2', 600)",
            [],
        )
        .unwrap();
        let artists = load_artist_videos(&conn).unwrap();
        assert_eq!(
            (artists[&5].programmable, artists[&5].programmable_s),
            (1, 200)
        );
    }

    /// Builds scenes, spread and layout twice over a copy of a real database
    /// and prints what the Atlas would show. Never point it at the live DB:
    /// it runs migrations and writes the derived tables.
    ///   NOOR_INTEL_DB=<copy> cargo test -p noor-server real_catalog_atlas_build -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_catalog_atlas_build() {
        use super::super::{layout, spread};
        let Ok(path) = std::env::var("NOOR_INTEL_DB") else {
            eprintln!("NOOR_INTEL_DB not set");
            return;
        };
        let conn = Connection::open(&path).unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let positions = |conn: &Connection| -> BTreeMap<i64, (String, f64, f64, f64)> {
            conn.prepare("SELECT scene_id, name, x, y, r FROM video_scenes")
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?),
                    ))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let artist_positions = |conn: &Connection| -> BTreeMap<i64, (Option<i64>, f64, f64)> {
            conn.prepare("SELECT artist_tidal_id, scene_id, x, y FROM video_artist_facets")
                .unwrap()
                .query_map([], |row| {
                    Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?)))
                })
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        let mut runs = Vec::new();
        for round in 0..2 {
            let t = Instant::now();
            let scenes = build(&conn).unwrap();
            let scenes_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let genres = spread::build(&conn).unwrap();
            let spread_ms = t.elapsed().as_millis();
            let t = Instant::now();
            let placed = layout::build(&conn).unwrap();
            let layout_ms = t.elapsed().as_millis();
            eprintln!(
                "round {round}: scenes {scenes} ({scenes_ms} ms), genres {genres} ({spread_ms} ms), placed {placed} ({layout_ms} ms), coverage {:.3}",
                spread::coverage(&conn).unwrap()
            );
            runs.push((positions(&conn), artist_positions(&conn)));
        }
        let (first, second) = (&runs[0], &runs[1]);
        let same_ids = first.0.keys().eq(second.0.keys());
        let scene_drift = first
            .0
            .iter()
            .filter_map(|(id, a)| {
                second
                    .0
                    .get(id)
                    .map(|b| ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt())
            })
            .fold(0.0, f64::max);
        let artist_drift = first
            .1
            .iter()
            .filter_map(|(id, a)| {
                second
                    .1
                    .get(id)
                    .map(|b| ((a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt())
            })
            .fold(0.0, f64::max);
        eprintln!(
            "stable ids: {same_ids}; max scene drift {scene_drift:.6}; max artist drift {artist_drift:.6}"
        );
        let (charted, uncharted, covered_videos): (i64, i64, i64) = conn
            .query_row(
                "SELECT SUM(charted), SUM(1 - charted), SUM(CASE WHEN charted = 1 THEN videos ELSE 0 END)
                   FROM video_artist_facets",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        eprintln!("charted {charted}, uncharted {uncharted}, videos in scenes {covered_videos}");
        let mut stmt = conn
            .prepare(
                "SELECT scene_id, name, pinned, video_count, artist_count, round(hours), exemplars_json, tags_json,
                        round(x), round(y), round(r), round(hue)
                   FROM video_scenes ORDER BY video_count DESC",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(format!(
                    "{:>3} | {:<34} | p{} | {:>5} v | {:>4} a | {:>4} h | {} | {} | ({}, {}) r{} hue {}",
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, f64>(8)?,
                    row.get::<_, f64>(9)?,
                    row.get::<_, f64>(10)?,
                    row.get::<_, f64>(11)?,
                ))
            })
            .unwrap();
        for row in rows {
            eprintln!("{}", row.unwrap());
        }
        if std::env::var("NOOR_INTEL_MEMBERS").is_ok() {
            // Top ranked members with ids, for curating names.
            let plan_scenes = load_previous(&conn).unwrap();
            let names: HashMap<i64, String> = conn
                .prepare("SELECT artist_tidal_id, name FROM video_artist_facets")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let graph = ArtistGraph::load(&conn).unwrap();
            let popularity = load_popularity(&conn).unwrap();
            let videos: HashMap<i64, i64> = conn
                .prepare("SELECT artist_tidal_id, videos FROM video_artist_facets")
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            for scene in plan_scenes {
                let mut ranked = scene.members.clone();
                let score = |id: &i64| {
                    exemplar_score(videos[id], graph.degree(*id), popularity.get(id).copied())
                };
                ranked.sort_by(|a, b| score(b).total_cmp(&score(a)).then(a.cmp(b)));
                let top: Vec<String> = ranked
                    .iter()
                    .take(12)
                    .map(|id| format!("{}={}", names[id], id))
                    .collect();
                eprintln!("{} {}: {}", scene.id, scene.name, top.join(", "));
            }
        }
    }

    #[test]
    fn routes_take_the_strongest_path_within_the_hop_cap() {
        let graph = ArtistGraph::from_rows(&[
            (1, 2, 0, 0.9),
            (2, 3, 1, 0.9),
            (3, 4, 2, 0.9),
            (1, 4, 3, 0.2),
            (4, 5, 0, 0.9),
        ]);
        let path = graph.route(1, 4, 6, |_| true).unwrap();
        let ids: Vec<i64> = path.iter().map(|p| p.0).collect();
        assert_eq!(
            ids,
            vec![1, 2, 3, 4],
            "three strong hops beat one weak edge"
        );
        assert_eq!(path[0].1, None);
        assert_eq!(path[3].1, Some("featured"));
        let capped = graph.route(1, 4, 2, |_| true).unwrap();
        assert_eq!(capped.len(), 2, "the cap forces the direct edge");
        let blocked = graph.route(1, 4, 6, |id| id != 3).unwrap();
        assert_eq!(blocked.len(), 2);
        assert!(graph.route(1, 99, 6, |_| true).is_none());
        assert_eq!(graph.route(5, 5, 6, |_| true).unwrap(), vec![(5, None)]);
        assert!(graph.route(1, 5, 1, |_| true).is_none());
    }
}
