//! The Archive lens query: one tile per song with live facet counts under
//! the other active lenses.
//!
//! 130k songs and five lens dimensions make SQL GROUP BYs too slow for a
//! keystroke-paced UI (one scan per facet), so the lens columns live in a
//! compact in-memory index rebuilt once per catalog build (keyed by the
//! build-run signature). One pass over the index answers the filter, every
//! facet count and the sort; SQLite is only asked for the page's rows.
//!
//! Light is computed per request (watch history and likes change all the
//! time). This is the catalog worker's local version: 3 watched or saved,
//! 2 library artist, 1 charted, 0 uncharted. The atlas's light.rs is the
//! canonical one.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;

use super::cuts::{self, Cut};
use crate::services::video_sets::{VideoCandidate, VideoSetItem};
use crate::services::video_stations::to_item;

pub const DEFAULT_LIMIT: usize = 60;
pub const MAX_LIMIT: usize = 200;
/// Song popularity (best cut's TIDAL popularity) at or above which a song is
/// "big"; below MID_FROM it is "deep". On the snapshot that is the top ~16%
/// and the bottom ~60% (most videos score under 5).
pub const BIG_FROM: i32 = 15;
pub const MID_FROM: i32 = 4;
/// Length classes from the best cut's runtime.
pub const LONG_FROM_S: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Depth {
    Big,
    Mid,
    Deep,
}

impl Depth {
    pub const ALL: [Depth; 3] = [Depth::Big, Depth::Mid, Depth::Deep];

    pub fn of(popularity: i32) -> Depth {
        if popularity >= BIG_FROM {
            Depth::Big
        } else if popularity >= MID_FROM {
            Depth::Mid
        } else {
            Depth::Deep
        }
    }

    fn parse(raw: &str) -> Option<Depth> {
        match raw {
            "big" => Some(Depth::Big),
            "mid" => Some(Depth::Mid),
            "deep" => Some(Depth::Deep),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Length {
    Clip,
    Song,
    Long,
}

impl Length {
    pub const ALL: [Length; 3] = [Length::Clip, Length::Song, Length::Long];

    pub fn of(duration_s: i64) -> Length {
        if duration_s > 0 && duration_s < cuts::CLIP_UNDER_S {
            Length::Clip
        } else if duration_s >= LONG_FROM_S {
            Length::Long
        } else {
            Length::Song
        }
    }

    fn parse(raw: &str) -> Option<Length> {
        match raw {
            "clip" => Some(Length::Clip),
            "song" => Some(Length::Song),
            "long" => Some(Length::Long),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Catalog,
    Liked,
    Saved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Relevance,
    Newest,
    Year,
    Popular,
    Versions,
}

/// `ArchiveQuery` from the client. Unknown values are ignored, never errors.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub scene: Option<i64>,
    /// Only this artist's songs (TIDAL id). Not a facet: it scopes every count.
    pub artist: Option<i64>,
    pub cut: Option<Cut>,
    pub era_from: Option<i64>,
    pub era_to: Option<i64>,
    pub depth: Option<Depth>,
    pub length: Option<Length>,
    pub source: Source,
    pub sort: Sort,
    pub q: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

impl Query {
    pub fn from_params(params: &HashMap<String, String>) -> Query {
        let get = |key: &str| params.get(key).map(|v| v.trim()).filter(|v| !v.is_empty());
        let int = |key: &str| get(key).and_then(|v| v.parse::<i64>().ok());
        Query {
            scene: int("scene"),
            artist: int("artist").filter(|id| *id > 0),
            cut: get("cut").and_then(Cut::parse),
            era_from: int("era_from"),
            era_to: int("era_to"),
            depth: get("depth").and_then(Depth::parse),
            length: get("length").and_then(Length::parse),
            source: match get("source") {
                Some("liked") => Source::Liked,
                Some("saved") => Source::Saved,
                _ => Source::Catalog,
            },
            sort: match get("sort") {
                Some("newest") => Sort::Newest,
                Some("year") => Sort::Year,
                Some("popular") => Sort::Popular,
                Some("versions") => Sort::Versions,
                _ => Sort::Relevance,
            },
            q: get("q").map(|q| q.chars().take(100).collect()),
            offset: int("offset").map_or(0, |o| o.max(0) as usize),
            limit: int("limit").map_or(DEFAULT_LIMIT, |l| (l.max(1) as usize).min(MAX_LIMIT)),
        }
    }
}

// ---------------------------------------------------------------- light

/// The local light rule (see the module docs).
#[derive(Debug, Default, Clone)]
pub struct Lights {
    pub lit: HashSet<i64>,
    pub library: HashSet<i64>,
}

impl Lights {
    pub fn load(conn: &Connection) -> Result<Lights> {
        let ids = |sql: &str| -> Result<HashSet<i64>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, Option<i64>>(0))?;
            let mut out = HashSet::new();
            for row in rows {
                if let Some(id) = row?.filter(|id| *id > 0) {
                    out.insert(id);
                }
            }
            Ok(out)
        };
        Ok(Lights {
            lit: ids("SELECT DISTINCT artist_tidal_id FROM video_history
                 UNION SELECT json_extract(item_json, '$.artist_id') FROM saved_videos")?,
            library: ids("SELECT tidal_id FROM artists WHERE tidal_id > 0")?,
        })
    }

    pub fn light(&self, artist: Option<i64>, charted: bool) -> u8 {
        match artist.filter(|id| *id > 0) {
            Some(id) if self.lit.contains(&id) => 3,
            Some(id) if self.library.contains(&id) => 2,
            _ if charted => 1,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------- index

#[derive(Debug, Clone)]
struct Row {
    rowid: i64,
    artist: i64,
    scene: i64,
    mask: i64,
    programmable: bool,
    year: Option<i64>,
    popularity: i32,
    length: Length,
    cut_count: i64,
    newest: i64,
    charted: bool,
    /// Folded "title artist", for the text lens.
    text: Box<str>,
    /// Byte length of the folded title at the start of `text`.
    title_len: usize,
}

/// The lens columns of every song.
pub struct Index {
    signature: String,
    rows: Vec<Row>,
}

/// Changes whenever a build stage finishes or the songs table is rebuilt.
pub fn signature(conn: &Connection) -> Result<String> {
    Ok(conn.query_row(
        "SELECT (SELECT COUNT(finished_at) || ':' || COALESCE(MAX(id), 0) || ':'
                        || COALESCE(MAX(finished_at), '')
                   FROM video_intel_runs)
                || ':' || COALESCE((SELECT MAX(rowid) FROM video_songs), 0)",
        [],
        |row| row.get(0),
    )?)
}

fn squash(text: &str) -> String {
    cuts::fold(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Index {
    pub fn load(conn: &Connection) -> Result<Index> {
        let signature = signature(conn)?;
        let mut stmt = conn.prepare(
            "SELECT s.rowid, COALESCE(s.artist_tidal_id, 0), COALESCE(s.scene_id, -1),
                    s.cuts_mask, s.programmable, s.song_year, COALESCE(s.popularity, -1),
                    COALESCE(f.duration_s, 0), s.cut_count, s.title,
                    COALESCE(s.artist_name, ''), COALESCE(a.charted, 0),
                    (SELECT MAX(x.tidal_video_id) FROM video_facets x WHERE x.song_key = s.song_key)
               FROM video_songs s
               LEFT JOIN video_facets f ON f.tidal_video_id = s.best_video_id
               LEFT JOIN video_artist_facets a ON a.artist_tidal_id = s.artist_tidal_id",
        )?;
        let mut rows = Vec::new();
        let mut query = stmt.query([])?;
        while let Some(row) = query.next()? {
            let title = squash(&row.get::<_, String>(9)?);
            let artist = squash(&row.get::<_, String>(10)?);
            let title_len = title.len();
            rows.push(Row {
                rowid: row.get(0)?,
                artist: row.get(1)?,
                scene: row.get(2)?,
                mask: row.get(3)?,
                programmable: row.get(4)?,
                year: row.get(5)?,
                popularity: row.get::<_, i64>(6)?.clamp(-1, 1000) as i32,
                length: Length::of(row.get(7)?),
                cut_count: row.get(8)?,
                charted: row.get::<_, i64>(11)? != 0,
                newest: row.get::<_, Option<i64>>(12)?.unwrap_or(0),
                text: format!("{title} {artist}").into_boxed_str(),
                title_len,
            });
        }
        Ok(Index { signature, rows })
    }

    pub fn song_count(&self) -> usize {
        self.rows.len()
    }
}

static CACHE: LazyLock<Mutex<Option<Arc<Index>>>> = LazyLock::new(|| Mutex::new(None));

fn from_cache(signature: &str) -> Option<Arc<Index>> {
    CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .filter(|index| index.signature == signature)
}

fn store(index: Index) -> Arc<Index> {
    let index = Arc::new(index);
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(index.clone());
    }
    index
}

/// The index for the current build, loading it when the build changed.
pub fn cached(conn: &Connection) -> Result<Arc<Index>> {
    let current = signature(conn)?;
    if let Some(index) = from_cache(&current) {
        return Ok(index);
    }
    Ok(store(Index::load(conn)?))
}

/// Like `cached`, but a reload (about a second on the real catalog) runs on
/// its own connection so it never holds the shared one.
pub fn cached_for(db: &crate::db::Database) -> Result<Arc<Index>> {
    let current = db.with_conn(signature)?;
    if let Some(index) = from_cache(&current) {
        return Ok(index);
    }
    match db.open_isolated() {
        Ok(conn) => Ok(store(Index::load(&conn)?)),
        Err(_) => db.with_conn(cached),
    }
}

// ---------------------------------------------------------------- response

/// `ArchiveSong` on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveSong {
    pub song_key: String,
    pub title: String,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub best: VideoSetItem,
    pub cut_count: i64,
    pub cuts: Vec<Cut>,
    pub song_year: Option<i64>,
    pub scene_id: Option<i64>,
    pub light: u8,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SceneFacet {
    pub id: i64,
    pub name: String,
    pub hue: f64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CutFacet {
    pub cut: Cut,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecadeFacet {
    pub decade: i64,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DepthFacet {
    pub depth: Depth,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LengthFacet {
    pub length: Length,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Facets {
    pub scene: Vec<SceneFacet>,
    pub cut: Vec<CutFacet>,
    pub decade: Vec<DecadeFacet>,
    pub depth: Vec<DepthFacet>,
    pub length: Vec<LengthFacet>,
}

/// `ArchiveResponse` on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    /// True until the first catalog build has finished.
    pub building: bool,
    pub total: usize,
    pub items: Vec<ArchiveSong>,
    pub facets: Facets,
}

// ---------------------------------------------------------------- query

const SCENE: u8 = 1;
const CUT: u8 = 2;
const ERA: u8 = 4;
const DEPTH: u8 = 8;
const LENGTH: u8 = 16;
const ALL: u8 = SCENE | CUT | ERA | DEPTH | LENGTH;

fn passes(row: &Row, query: &Query) -> u8 {
    let mut pass = 0;
    if query.scene.is_none_or(|scene| row.scene == scene) {
        pass |= SCENE;
    }
    let cut_ok = match query.cut {
        Some(cut) => row.mask & cut.bit() != 0,
        // Without a cut lens the Archive shows what can be programmed.
        None => row.programmable,
    };
    if cut_ok {
        pass |= CUT;
    }
    let era_ok = match (query.era_from, query.era_to) {
        (None, None) => true,
        (from, to) => row
            .year
            .is_some_and(|y| from.is_none_or(|f| y >= f) && to.is_none_or(|t| y <= t)),
    };
    if era_ok {
        pass |= ERA;
    }
    if query
        .depth
        .is_none_or(|depth| Depth::of(row.popularity) == depth)
    {
        pass |= DEPTH;
    }
    if query.length.is_none_or(|length| row.length == length) {
        pass |= LENGTH;
    }
    pass
}

/// Songs a liked or saved source lens allows, as rowids.
fn source_rowids(conn: &Connection, source: Source) -> Result<Option<HashSet<i64>>> {
    let sql = match source {
        Source::Catalog => return Ok(None),
        Source::Liked => {
            "SELECT s.rowid FROM video_songs s WHERE s.song_key IN (
                 SELECT f.song_key FROM library_videos lv
                   JOIN video_facets f ON f.tidal_video_id = lv.tidal_video_id
                  WHERE lv.suppressed = 0)"
        }
        Source::Saved => {
            "SELECT s.rowid FROM video_songs s WHERE s.song_key IN (
                 SELECT f.song_key FROM saved_videos sv
                   JOIN video_facets f ON f.tidal_video_id = sv.tidal_video_id)"
        }
    };
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, i64>(0))?;
    Ok(Some(rows.collect::<Result<_, _>>()?))
}

/// The filter, facets and ordered page of rowids. Pure over the index.
pub fn select(
    index: &Index,
    query: &Query,
    allowed: Option<&HashSet<i64>>,
    lights: &Lights,
) -> (usize, Vec<i64>, Facets, HashMap<i64, u8>) {
    let needle = query.q.as_deref().map(squash).filter(|q| !q.is_empty());
    let mut matched: Vec<&Row> = Vec::new();
    let mut scene_counts: HashMap<i64, usize> = HashMap::new();
    let mut cut_counts = [0usize; 11];
    let mut decade_counts: HashMap<i64, usize> = HashMap::new();
    let mut depth_counts: HashMap<Depth, usize> = HashMap::new();
    let mut length_counts: HashMap<Length, usize> = HashMap::new();
    for row in &index.rows {
        if allowed.is_some_and(|ids| !ids.contains(&row.rowid)) {
            continue;
        }
        if query.artist.is_some_and(|artist| row.artist != artist) {
            continue;
        }
        if let Some(needle) = &needle
            && !row.text.contains(needle.as_str())
        {
            continue;
        }
        let pass = passes(row, query);
        let others = |dimension: u8| pass | dimension == ALL;
        if pass == ALL {
            matched.push(row);
        }
        if others(SCENE) && row.scene >= 0 {
            *scene_counts.entry(row.scene).or_default() += 1;
        }
        if others(CUT) {
            for cut in Cut::ALL {
                if row.mask & cut.bit() != 0 {
                    cut_counts[cut.index()] += 1;
                }
            }
        }
        if others(ERA)
            && let Some(year) = row.year
        {
            *decade_counts.entry(year.div_euclid(10) * 10).or_default() += 1;
        }
        if others(DEPTH) {
            *depth_counts.entry(Depth::of(row.popularity)).or_default() += 1;
        }
        if others(LENGTH) {
            *length_counts.entry(row.length).or_default() += 1;
        }
    }
    let light_of = |row: &Row| lights.light(Some(row.artist), row.charted);
    let q_rank = |row: &Row| -> u8 {
        let Some(needle) = &needle else {
            return 0;
        };
        let title = &row.text[..row.title_len];
        if title == needle.as_str() {
            3
        } else if title.starts_with(needle.as_str()) {
            2
        } else if row.text[row.title_len..]
            .trim_start()
            .starts_with(needle.as_str())
        {
            1
        } else {
            0
        }
    };
    // One key per matched row, computed once (light is a set lookup), then
    // only the rows up to the page end are ordered.
    let key = |row: &Row| -> [i64; 5] {
        let popularity = i64::from(row.popularity);
        match query.sort {
            Sort::Relevance => [
                -i64::from(q_rank(row)),
                -i64::from(light_of(row)),
                -popularity,
                -row.cut_count,
                row.rowid,
            ],
            Sort::Newest => [-row.newest, 0, 0, 0, row.rowid],
            Sort::Year => [
                i64::from(row.year.is_none()),
                row.year.unwrap_or(0),
                -popularity,
                0,
                row.rowid,
            ],
            Sort::Popular => [-popularity, -row.cut_count, 0, 0, row.rowid],
            Sort::Versions => [-row.cut_count, -popularity, 0, 0, row.rowid],
        }
    };
    let mut keyed: Vec<([i64; 5], &Row)> = matched.iter().map(|row| (key(row), *row)).collect();
    let total = keyed.len();
    let start = query.offset.min(total);
    let end = (start + query.limit).min(total);
    // Best-first and popular lists space artists out (one artist's catalog
    // would otherwise fill the first screen); an artist scope needs none.
    let spaced = matches!(query.sort, Sort::Relevance | Sort::Popular) && query.artist.is_none();
    let page_rows: Vec<&Row> = if spaced {
        keyed.sort_unstable_by_key(|(key, _)| *key);
        let ranked: Vec<&Row> = keyed.iter().map(|(_, row)| *row).collect();
        space_artists(&ranked, end).split_off(start)
    } else {
        if end > 0 && end < total {
            keyed.select_nth_unstable_by_key(end - 1, |(key, _)| *key);
        }
        keyed[..end].sort_unstable_by_key(|(key, _)| *key);
        keyed[start..end].iter().map(|(_, row)| *row).collect()
    };
    let page: Vec<i64> = page_rows.iter().map(|row| row.rowid).collect();
    let lights_by_row: HashMap<i64, u8> = page_rows
        .iter()
        .map(|row| (row.rowid, light_of(row)))
        .collect();

    let mut facets = Facets {
        scene: scene_counts
            .into_iter()
            .map(|(id, count)| SceneFacet {
                id,
                name: String::new(),
                hue: 0.0,
                count,
            })
            .collect(),
        cut: Cut::ALL
            .into_iter()
            .filter(|cut| cut_counts[cut.index()] > 0 || query.cut == Some(*cut))
            .map(|cut| CutFacet {
                cut,
                count: cut_counts[cut.index()],
            })
            .collect(),
        decade: {
            let mut decades: Vec<DecadeFacet> = decade_counts
                .into_iter()
                .map(|(decade, count)| DecadeFacet { decade, count })
                .collect();
            decades.sort_by_key(|d| d.decade);
            decades
        },
        depth: Depth::ALL
            .into_iter()
            .map(|depth| DepthFacet {
                depth,
                count: depth_counts.get(&depth).copied().unwrap_or(0),
            })
            .collect(),
        length: Length::ALL
            .into_iter()
            .map(|length| LengthFacet {
                length,
                count: length_counts.get(&length).copied().unwrap_or(0),
            })
            .collect(),
    };
    facets
        .scene
        .sort_by(|a, b| b.count.cmp(&a.count).then(a.id.cmp(&b.id)));
    (total, page, facets, lights_by_row)
}

/// No artist twice within this many consecutive results.
pub const SPACING_WINDOW: usize = 6;

/// The first `end` rows of a ranked list with artists spaced out: each slot
/// takes the best-ranked remaining row whose artist is not among the last
/// `SPACING_WINDOW - 1` taken. Later rows by a repeated artist move down,
/// never out; when only blocked artists remain the best-ranked row goes
/// anyway. Deterministic and prefix-stable, so every page is a slice of one
/// spaced order.
fn space_artists<'a>(ranked: &[&'a Row], end: usize) -> Vec<&'a Row> {
    // Songs without an artist count as their own artist.
    let artist = |row: &Row| {
        if row.artist > 0 {
            row.artist
        } else {
            -row.rowid
        }
    };
    let mut out: Vec<&Row> = Vec::with_capacity(end);
    let mut recent: VecDeque<i64> = VecDeque::with_capacity(SPACING_WINDOW);
    // Deferred rows per artist (in rank order) and each artist's first
    // deferred rank, so the best allowed deferred row is found in a few steps.
    let mut waiting: HashMap<i64, VecDeque<usize>> = HashMap::new();
    let mut heads: BTreeSet<(usize, i64)> = BTreeSet::new();
    let mut next = 0usize;
    while out.len() < end {
        let blocked = |id: i64| recent.contains(&id);
        let mut pick = heads
            .iter()
            .find(|(_, id)| !blocked(*id))
            .map(|&(rank, id)| (rank, id));
        if pick.is_none() {
            while next < ranked.len() {
                let rank = next;
                next += 1;
                let id = artist(ranked[rank]);
                if !blocked(id) {
                    pick = Some((rank, id));
                    break;
                }
                let queue = waiting.entry(id).or_default();
                if queue.is_empty() {
                    heads.insert((rank, id));
                }
                queue.push_back(rank);
            }
        }
        // Only blocked artists left: the best-ranked one goes anyway.
        if pick.is_none() {
            pick = heads.iter().next().copied();
        }
        let Some((rank, id)) = pick else {
            break;
        };
        if heads.remove(&(rank, id)) {
            let queue = waiting.entry(id).or_default();
            queue.pop_front();
            if let Some(&head) = queue.front() {
                heads.insert((head, id));
            }
        }
        out.push(ranked[rank]);
        if recent.len() == SPACING_WINDOW - 1 {
            recent.pop_front();
        }
        recent.push_back(id);
    }
    out
}

/// The best cut of one kind for a song, when a cut lens is on.
fn best_of_cut(conn: &Connection, song_key: &str, cut: Cut) -> Result<Option<i64>> {
    use rusqlite::OptionalExtension;
    Ok(conn
        .query_row(
            "SELECT tidal_video_id FROM video_facets WHERE song_key = ?1 AND cut = ?2
              ORDER BY programmable DESC, COALESCE(duration_s, 0) DESC,
                       COALESCE(popularity, -1) DESC, tidal_video_id ASC
              LIMIT 1",
            params![song_key, cut.as_str()],
            |row| row.get(0),
        )
        .optional()?)
}

/// Answer one Archive request.
pub fn query(conn: &Connection, index: &Index, query: &Query, lights: &Lights) -> Result<Response> {
    let allowed = source_rowids(conn, query.source)?;
    let (total, page, mut facets, lights_by_row) = select(index, query, allowed.as_ref(), lights);
    // Scene names and hues for the scene chips.
    {
        let mut stmt = conn.prepare("SELECT scene_id, name, hue FROM video_scenes")?;
        let scenes: HashMap<i64, (String, f64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))?
            .collect::<Result<_, _>>()?;
        for facet in &mut facets.scene {
            if let Some((name, hue)) = scenes.get(&facet.id) {
                facet.name = name.clone();
                facet.hue = *hue;
            }
        }
    }
    struct SongRow {
        key: String,
        title: String,
        artist_id: Option<i64>,
        artist_name: Option<String>,
        best: i64,
        cut_count: i64,
        mask: i64,
        year: Option<i64>,
        scene: Option<i64>,
    }
    let ids_json = serde_json::to_string(&page)?;
    let mut songs: HashMap<i64, SongRow> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT rowid, song_key, title, artist_tidal_id, artist_name, best_video_id,
                    cut_count, cuts_mask, song_year, scene_id
               FROM video_songs WHERE rowid IN (SELECT value FROM json_each(?1))",
        )?;
        let mut rows = stmt.query([&ids_json])?;
        while let Some(row) = rows.next()? {
            songs.insert(
                row.get(0)?,
                SongRow {
                    key: row.get(1)?,
                    title: row.get(2)?,
                    artist_id: row.get(3)?,
                    artist_name: row.get(4)?,
                    best: row.get(5)?,
                    cut_count: row.get(6)?,
                    mask: row.get(7)?,
                    year: row.get(8)?,
                    scene: row.get(9)?,
                },
            );
        }
    }
    if let Some(cut) = query.cut {
        for song in songs.values_mut() {
            if let Some(best) = best_of_cut(conn, &song.key, cut)? {
                song.best = best;
            }
        }
    }
    let best_ids: Vec<i64> = songs.values().map(|s| s.best).collect();
    let mut videos: HashMap<i64, VideoCandidate> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, item_json FROM video_catalog
              WHERE tidal_video_id IN (SELECT value FROM json_each(?1))",
        )?;
        let mut rows = stmt.query([serde_json::to_string(&best_ids)?])?;
        while let Some(row) = rows.next()? {
            if let Ok(video) = serde_json::from_str::<VideoCandidate>(&row.get::<_, String>(1)?) {
                videos.insert(row.get(0)?, video);
            }
        }
    }
    let items = page
        .iter()
        .filter_map(|rowid| {
            let song = songs.remove(rowid)?;
            let video = videos.get(&song.best)?;
            Some(ArchiveSong {
                song_key: song.key,
                title: song.title,
                artist_id: song.artist_id,
                artist_name: song.artist_name,
                best: to_item(video),
                cut_count: song.cut_count,
                cuts: Cut::from_mask(song.mask),
                song_year: song.year,
                scene_id: song.scene,
                light: lights_by_row.get(rowid).copied().unwrap_or(0),
            })
        })
        .collect();
    Ok(Response {
        building: super::census::built_at(conn)?.is_none(),
        total,
        items,
        facets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::video_intel::songs::tests::{add, conn};
    use crate::services::video_intel::{concerts, cuts, era, link, songs};

    fn params(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn seeded() -> Connection {
        let conn = conn();
        add(&conn, 1, 7, "Alpha", 240, Some(40), Some(1999));
        add(
            &conn,
            2,
            7,
            "Alpha (Live at Wembley)",
            260,
            Some(10),
            Some(2005),
        );
        add(
            &conn,
            3,
            7,
            "Beta (Official Lyric Video)",
            200,
            Some(2),
            Some(2012),
        );
        add(&conn, 4, 8, "Gamma", 700, None, Some(2021));
        add(
            &conn,
            5,
            8,
            "Delta (Official Trailer)",
            120,
            Some(50),
            Some(2021),
        );
        add(
            &conn,
            6,
            9,
            "Acustico Total (Ac\u{fa}stico)",
            230,
            Some(8),
            Some(2018),
        );
        conn.execute_batch(
            "INSERT INTO video_scenes (scene_id, name, hue) VALUES (1, 'Seven & Eight', 120);
             INSERT INTO video_artist_facets (artist_tidal_id, scene_id, charted) VALUES (7, 1, 1), (8, 1, 1);
             INSERT INTO artists (id, tidal_id, name) VALUES (1, 8, 'Artist 8');
             INSERT INTO video_history (tidal_video_id, artist_tidal_id) VALUES (1, 7);",
        )
        .unwrap();
        cuts::build(&conn).unwrap();
        songs::build(&conn).unwrap();
        concerts::build(&conn).unwrap();
        era::build(&conn).unwrap();
        link::build(&conn).unwrap();
        conn
    }

    fn run(conn: &Connection, pairs: &[(&str, &str)]) -> Response {
        let index = Index::load(conn).unwrap();
        let lights = Lights::load(conn).unwrap();
        query(conn, &index, &Query::from_params(&params(pairs)), &lights).unwrap()
    }

    fn titles(response: &Response) -> Vec<String> {
        response.items.iter().map(|s| s.title.clone()).collect()
    }

    #[test]
    fn default_view_is_one_tile_per_programmable_song() {
        let conn = seeded();
        let all = run(&conn, &[]);
        // Delta is only a trailer, so it is not programmable and stays out.
        assert_eq!(all.total, 4);
        let alpha = all.items.iter().find(|s| s.title == "Alpha").unwrap();
        assert_eq!(alpha.cut_count, 2);
        assert_eq!(alpha.cuts, vec![Cut::Official, Cut::Live]);
        assert_eq!(alpha.best.tidal_id, 1);
        assert_eq!(alpha.light, 3, "watched artist");
        assert_eq!(alpha.scene_id, Some(1));
        let gamma = all.items.iter().find(|s| s.title == "Gamma").unwrap();
        assert_eq!(gamma.light, 2, "library artist");
        // Relevance puts lit artists first.
        assert_eq!(all.items[0].artist_id, Some(7));
    }

    #[test]
    fn facets_count_under_the_other_lenses() {
        let conn = seeded();
        let live = run(&conn, &[("cut", "live")]);
        assert_eq!(titles(&live), vec!["Alpha"]);
        assert_eq!(
            live.items[0].best.tidal_id, 2,
            "the live cut fronts the tile"
        );
        // Cut counts ignore the cut lens itself but keep the others.
        let cut = |response: &Response, cut: Cut| {
            response
                .facets
                .cut
                .iter()
                .find(|f| f.cut == cut)
                .map_or(0, |f| f.count)
        };
        assert_eq!(cut(&live, Cut::Official), 2);
        assert_eq!(cut(&live, Cut::Trailer), 1);
        assert_eq!(cut(&live, Cut::Session), 1);
        // Scene facet under the cut lens: only Alpha's scene.
        assert_eq!(
            live.facets.scene,
            vec![SceneFacet {
                id: 1,
                name: "Seven & Eight".into(),
                hue: 120.0,
                count: 1
            }]
        );
        let scoped = run(&conn, &[("scene", "1"), ("length", "long")]);
        assert_eq!(titles(&scoped), vec!["Gamma"]);
        let length = |l: Length| {
            scoped
                .facets
                .length
                .iter()
                .find(|f| f.length == l)
                .unwrap()
                .count
        };
        assert_eq!((length(Length::Song), length(Length::Long)), (2, 1));
        let decades: Vec<(i64, usize)> = run(&conn, &[])
            .facets
            .decade
            .iter()
            .map(|d| (d.decade, d.count))
            .collect();
        assert_eq!(decades, vec![(1990, 1), (2010, 2), (2020, 1)]);
    }

    #[test]
    fn era_depth_text_and_paging() {
        let conn = seeded();
        assert_eq!(
            titles(&run(
                &conn,
                &[("era_from", "2010"), ("era_to", "2019"), ("sort", "year")]
            )),
            vec!["Beta", "Acustico Total"]
        );
        assert_eq!(titles(&run(&conn, &[("depth", "big")])), vec!["Alpha"]);
        assert_eq!(
            titles(&run(&conn, &[("q", "acustico")])),
            vec!["Acustico Total"]
        );
        assert_eq!(titles(&run(&conn, &[("q", "artist 8")])), vec!["Gamma"]);
        let page = run(
            &conn,
            &[("sort", "popular"), ("offset", "1"), ("limit", "2")],
        );
        assert_eq!(page.total, 4);
        // Popular is Alpha, Acustico, Beta, Gamma; Beta shares Alpha's artist,
        // so spacing moves Gamma ahead of it.
        assert_eq!(titles(&page), vec!["Acustico Total", "Gamma"]);
        assert_eq!(
            titles(&run(&conn, &[("sort", "versions"), ("limit", "1")])),
            vec!["Alpha"]
        );
        assert_eq!(
            titles(&run(&conn, &[("source", "saved")])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn artist_scope_and_building_flag() {
        let conn = seeded();
        let seven = run(&conn, &[("artist", "7")]);
        assert_eq!(titles(&seven), vec!["Alpha", "Beta"]);
        let count = |cut: Cut| {
            seven
                .facets
                .cut
                .iter()
                .find(|f| f.cut == cut)
                .map_or(0, |f| f.count)
        };
        assert_eq!(
            (count(Cut::Official), count(Cut::Live), count(Cut::Lyric)),
            (1, 1, 1)
        );
        assert_eq!(count(Cut::Trailer), 0, "artist 8's trailer is out of scope");
        assert!(seven.building, "stages ran by hand: no finished build");
        crate::services::video_intel::build::build_all(&conn).unwrap();
        assert!(!run(&conn, &[]).building);
    }

    fn row(rowid: i64, artist: i64) -> Row {
        Row {
            rowid,
            artist,
            scene: -1,
            mask: Cut::Official.bit(),
            programmable: true,
            year: None,
            popularity: 0,
            length: Length::Song,
            cut_count: 1,
            newest: 0,
            charted: false,
            text: "".into(),
            title_len: 0,
        }
    }

    fn artists_of(rows: &[&Row]) -> Vec<i64> {
        rows.iter().map(|r| r.artist).collect()
    }

    #[test]
    fn spacing_keeps_an_artist_out_of_any_six_in_a_row() {
        // Seven songs by artist 1 lead the ranking, then six other artists.
        let rows: Vec<Row> = (0..7)
            .map(|i| row(i, 1))
            .chain((7..13).map(|i| row(i, i)))
            .collect();
        let ranked: Vec<&Row> = rows.iter().collect();
        let spaced = space_artists(&ranked, rows.len());
        assert_eq!(spaced.len(), rows.len(), "nothing is dropped");
        assert_eq!(
            artists_of(&spaced),
            vec![1, 7, 8, 9, 10, 11, 1, 12, 1, 1, 1, 1, 1],
            "spaced while others remain, then the rest in rank order"
        );
        for window in spaced[..8].windows(SPACING_WINDOW) {
            let mut ids: Vec<i64> = window.iter().map(|r| r.artist).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), SPACING_WINDOW);
        }
        // Artist 1's songs keep their own order.
        let ones: Vec<i64> = spaced
            .iter()
            .filter(|r| r.artist == 1)
            .map(|r| r.rowid)
            .collect();
        assert_eq!(ones, (0..7).collect::<Vec<_>>());
    }

    #[test]
    fn spacing_is_stable_and_pages_are_slices_of_one_order() {
        let distinct: Vec<Row> = (0..10).map(|i| row(i, 100 + i)).collect();
        let ranked: Vec<&Row> = distinct.iter().collect();
        assert_eq!(
            space_artists(&ranked, 10)
                .iter()
                .map(|r| r.rowid)
                .collect::<Vec<_>>(),
            (0..10).collect::<Vec<_>>(),
            "no conflicts, no change"
        );
        let mixed: Vec<Row> = (0..40)
            .map(|i| row(i, [1, 1, 2, 1, 3, 2, 4][i as usize % 7]))
            .collect();
        let ranked: Vec<&Row> = mixed.iter().collect();
        let full: Vec<i64> = space_artists(&ranked, 40).iter().map(|r| r.rowid).collect();
        let first: Vec<i64> = space_artists(&ranked, 12).iter().map(|r| r.rowid).collect();
        assert_eq!(first, full[..12].to_vec());
        let unknown = [row(1, 0), row(2, 0)];
        let ranked: Vec<&Row> = unknown.iter().collect();
        assert_eq!(space_artists(&ranked, 2).len(), 2, "no artist is no clash");
    }

    #[test]
    fn best_first_spaces_a_prolific_artist_and_an_artist_scope_does_not() {
        let conn = conn();
        for id in 0..8 {
            add(
                &conn,
                100 + id,
                7,
                &format!("Hit {id}"),
                200,
                Some(90 - id as i32),
                None,
            );
        }
        for artist in 0..6 {
            add(
                &conn,
                200 + artist,
                20 + artist,
                "Other",
                200,
                Some(10),
                None,
            );
        }
        cuts::build(&conn).unwrap();
        songs::build(&conn).unwrap();
        for sort in ["relevance", "popular"] {
            let page = run(&conn, &[("sort", sort), ("limit", "8")]);
            let sevens = page
                .items
                .iter()
                .take(SPACING_WINDOW)
                .filter(|s| s.artist_id == Some(7))
                .count();
            assert_eq!(sevens, 1, "{sort}");
            let next = run(&conn, &[("sort", sort), ("offset", "4"), ("limit", "4")]);
            assert_eq!(titles(&next), titles(&page)[4..].to_vec(), "{sort} paging");
        }
        let scoped = run(&conn, &[("artist", "7"), ("limit", "3")]);
        assert_eq!(titles(&scoped), vec!["Hit 0", "Hit 1", "Hit 2"]);
        let newest = run(&conn, &[("sort", "versions"), ("limit", "3")]);
        assert_eq!(newest.items.len(), 3);
    }

    #[test]
    fn unknown_values_are_ignored() {
        let query = Query::from_params(&params(&[
            ("cut", "banana"),
            ("depth", "x"),
            ("limit", "100000"),
            ("offset", "-5"),
            ("sort", "?"),
        ]));
        assert_eq!(query.cut, None);
        assert_eq!(query.depth, None);
        assert_eq!(query.limit, MAX_LIMIT);
        assert_eq!(query.offset, 0);
        assert_eq!(query.sort, Sort::Relevance);
    }

    #[test]
    fn lights_rank_watched_then_library_then_charted() {
        let lights = Lights {
            lit: HashSet::from([1]),
            library: HashSet::from([1, 2]),
        };
        assert_eq!(lights.light(Some(1), true), 3);
        assert_eq!(lights.light(Some(2), false), 2);
        assert_eq!(lights.light(Some(3), true), 1);
        assert_eq!(lights.light(Some(3), false), 0);
        assert_eq!(lights.light(None, false), 0);
    }
}
