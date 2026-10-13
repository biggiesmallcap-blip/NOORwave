//! Genre spread: label propagation from tagged artists over the graph with
//! an inverse-frequency prior and hop decay. Writes
//! video_artist_facets.genres_json / genre_hops and video_scenes.tags_json.
//!
//! Only a fifth of artists carry a tag of their own (TIDAL seed genres,
//! library genres, the Last.fm cache). Untagged artists borrow from tagged
//! neighbours, one hop at full strength and a second at 0.6, with rare tags
//! weighted up so "pop" does not flood every artist. A borrowed tag is kept
//! only when it agrees with what the artist's scene is mostly tagged as, so
//! a stray edge never makes a metal band "classical".

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use rusqlite::{Connection, params};
use serde_json::Value;

use super::scenes::{self, ArtistGraph};
use crate::services::video_discovery::names::name_key;

const HOP_DECAY: f64 = 0.6;
const MAX_TAGS: usize = 4;
const SEED_TAGS: usize = 8;
/// A tag must reach this share of an artist's best tag to be listed.
const KEEP_SHARE: f64 = 0.3;
/// Scene majority: tags carried by at least this share of the scene's
/// labelled members (by weight), and no more than this many.
const SCENE_SUPPORT_SHARE: f64 = 0.06;
const SCENE_MAJORITY_TAGS: usize = 15;
const SCENE_HINT_TAGS: usize = 4;
/// A tag on fewer tagged artists than this is too rare to spread.
const MIN_TAG_ARTISTS: usize = 2;
const LASTFM_MIN_WEIGHT: f64 = 10.0;

/// Tags that describe listening habits, places or decades rather than a
/// sound.
const STOP_TAGS: &[&str] = &[
    "seen live",
    "favorites",
    "favourites",
    "favorite",
    "favourite",
    "all",
    "spotify",
    "under 2000 listeners",
    "albums i own",
    "my music",
    "love",
    "beautiful",
    "awesome",
    "cool",
    "fun",
    "chill",
    "sexy",
    "epic",
    "catchy",
    "party",
    "summer",
    "male vocalists",
    "female vocalists",
    "male vocalist",
    "female vocalist",
    "male",
    "female",
    "various artists",
    "american",
    "usa",
    "us",
    "uk",
    "british",
    "english",
    "canadian",
    "australian",
    "other",
    "unknown",
    "music",
];

/// Fold a raw tag into its canonical spelling, or None when it says nothing
/// about the sound.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let mut folded = String::with_capacity(raw.len());
    for c in raw.trim().chars().flat_map(char::to_lowercase) {
        match c {
            '&' => folded.push_str(" and "),
            '\'' => {}
            c if c.is_alphanumeric() => folded.push(c),
            _ => folded.push(' '),
        }
    }
    let tag = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    let tag = match tag.as_str() {
        "hiphop" | "hip hop" | "rap and hip hop" | "hip hop rap" => "hip hop".to_string(),
        "rnb" | "r and b" | "r n b" | "rhythm and blues" | "contemporary r and b" => {
            "r&b".to_string()
        }
        "drum n bass" | "drum and bass" | "dnb" | "d and b" | "drumnbass" => {
            "drum and bass".to_string()
        }
        "k pop" | "kpop" => "k-pop".to_string(),
        "j pop" | "jpop" => "j-pop".to_string(),
        "singer songwriter" | "singersongwriter" => "singer-songwriter".to_string(),
        "synth pop" => "synthpop".to_string(),
        "lo fi" | "lofi" => "lo-fi".to_string(),
        "post punk" => "post-punk".to_string(),
        "electronica" => "electronic".to_string(),
        "edm" | "electronic dance music" => "edm".to_string(),
        _ => tag,
    };
    let decade = tag.ends_with('s')
        && tag.len() <= 5
        && tag[..tag.len() - 1].chars().all(|c| c.is_ascii_digit());
    // "my top songs", "my favourites": a listener's shelf, not a sound.
    let personal = tag.starts_with("my ");
    if tag.chars().count() < 2 || decade || personal || STOP_TAGS.contains(&tag.as_str()) {
        return None;
    }
    Some(tag)
}

/// Tag vectors per artist, each normalised so its strongest tag is 1.
pub type Seeds = HashMap<i64, Vec<(String, f64)>>;

fn add_seed(raw: &mut HashMap<i64, BTreeMap<String, f64>>, artist: i64, tag: &str, weight: f64) {
    if artist <= 0 || !weight.is_finite() || weight <= 0.0 {
        return;
    }
    if let Some(tag) = normalize_tag(tag) {
        let slot = raw.entry(artist).or_default().entry(tag).or_default();
        *slot = slot.max(weight);
    }
}

fn finish_seeds(raw: HashMap<i64, BTreeMap<String, f64>>) -> Seeds {
    raw.into_iter()
        .filter_map(|(artist, tags)| {
            let mut tags: Vec<(String, f64)> = tags.into_iter().collect();
            tags.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            tags.truncate(SEED_TAGS);
            let top = tags.first()?.1;
            Some((
                artist,
                tags.into_iter().map(|(tag, w)| (tag, w / top)).collect(),
            ))
        })
        .collect()
}

/// Every artist's own tags: TIDAL seed genres by rank, library genres by
/// track share, Last.fm cache tags (matched by name key) by weight.
pub fn load_seeds(conn: &Connection) -> Result<Seeds> {
    let mut raw: HashMap<i64, BTreeMap<String, f64>> = HashMap::new();
    {
        let mut stmt =
            conn.prepare("SELECT seed_tidal_id, genre_name, rank FROM video_seed_genres")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (artist, tag, rank) = row?;
            add_seed(
                &mut raw,
                artist,
                &tag,
                1.0 / (1.0 + 0.25 * rank.max(0) as f64),
            );
        }
    }
    {
        let mut stmt = conn.prepare(
            "SELECT a.tidal_id, g.name, COUNT(*)
               FROM track_genres tg
               JOIN tracks t ON t.id = tg.track_id
               JOIN artists a ON a.id = t.artist_id
               JOIN genres g ON g.id = tg.genre_id
              WHERE a.tidal_id > 0
              GROUP BY a.tidal_id, g.name",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut library: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
        for row in rows {
            let (artist, tag, count) = row?;
            library.entry(artist).or_default().push((tag, count));
        }
        for (artist, tags) in library {
            let most = tags.iter().map(|(_, c)| *c).max().unwrap_or(1).max(1) as f64;
            for (tag, count) in tags {
                add_seed(&mut raw, artist, &tag, count as f64 / most);
            }
        }
    }
    {
        let mut by_key: HashMap<String, Vec<i64>> = HashMap::new();
        let mut stmt = conn.prepare(
            "SELECT artist_tidal_id, name_key FROM video_artist_state
              WHERE name_key <> '' AND artist_tidal_id > 0",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (artist, key) = row?;
            by_key.entry(key).or_default().push(artist);
        }
        let mut stmt = conn.prepare("SELECT artist_name, tags_json FROM lastfm_artist_cache")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (name, tags) = row?;
            let Some(artists) = by_key.get(&name_key(&name)) else {
                continue;
            };
            let Ok(Value::Array(tags)) = serde_json::from_str::<Value>(&tags) else {
                continue;
            };
            for (rank, pair) in tags.iter().take(SEED_TAGS).enumerate() {
                let (Some(tag), Some(weight)) = (
                    pair.get(0).and_then(Value::as_str),
                    pair.get(1).and_then(Value::as_f64),
                ) else {
                    continue;
                };
                if weight < LASTFM_MIN_WEIGHT {
                    continue;
                }
                let weight = (weight / 100.0).min(1.0) / (1.0 + 0.15 * rank as f64);
                for artist in artists {
                    add_seed(&mut raw, *artist, tag, weight);
                }
            }
        }
    }
    Ok(finish_seeds(raw))
}

/// Genres for one artist and the hop they came from (0 = its own).
#[derive(Debug, Clone, PartialEq)]
pub struct Spread {
    pub genres: Vec<String>,
    pub hops: u8,
}

#[derive(Debug, Default)]
pub struct SpreadResult {
    pub artists: HashMap<i64, Spread>,
    /// Scene id -> hint tags, most distinctive first.
    pub scene_tags: BTreeMap<i64, Vec<String>>,
}

fn top_tags(
    scores: &BTreeMap<usize, f64>,
    allowed: Option<&[usize]>,
    names: &[String],
) -> Vec<String> {
    let mut ranked: Vec<(usize, f64)> = scores
        .iter()
        .filter(|(tag, score)| **score > 0.0 && allowed.is_none_or(|set| set.contains(tag)))
        .map(|(tag, score)| (*tag, *score))
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let Some(best) = ranked.first().map(|r| r.1) else {
        return Vec::new();
    };
    ranked
        .into_iter()
        .take_while(|(_, score)| *score >= best * KEEP_SHARE)
        .take(MAX_TAGS)
        .map(|(tag, _)| names[tag].clone())
        .collect()
}

/// The whole stage as a pure function. `scene_of` maps artists with videos
/// to their scene id.
pub fn spread(graph: &ArtistGraph, seeds: &Seeds, scene_of: &HashMap<i64, i64>) -> SpreadResult {
    // Intern tags; count how many tagged artists carry each.
    let mut tag_index: BTreeMap<&str, usize> = BTreeMap::new();
    for tags in seeds.values() {
        for (tag, _) in tags {
            let next = tag_index.len();
            tag_index.entry(tag.as_str()).or_insert(next);
        }
    }
    let mut names = vec![String::new(); tag_index.len()];
    for (tag, index) in &tag_index {
        names[*index] = tag.to_string();
    }
    let mut seeded: HashMap<i64, Vec<(usize, f64)>> = HashMap::new();
    let mut artists_with = vec![0usize; names.len()];
    let mut ids: Vec<&i64> = seeds.keys().collect();
    ids.sort_unstable();
    for id in ids {
        let vector: Vec<(usize, f64)> = seeds[id]
            .iter()
            .map(|(tag, weight)| (tag_index[tag.as_str()], *weight))
            .collect();
        for (tag, _) in &vector {
            artists_with[*tag] += 1;
        }
        seeded.insert(*id, vector);
    }
    let tagged = seeded.len().max(1) as f64;
    // Inverse label frequency, scaled into (0, 1]: a tag everyone has counts
    // least, a tag few have counts most. Tags on a single artist never spread.
    let prior: Vec<f64> = artists_with
        .iter()
        .map(|count| {
            if *count < MIN_TAG_ARTISTS {
                0.0
            } else {
                (1.0 + tagged / *count as f64).ln() / (1.0 + tagged).ln().max(f64::MIN_POSITIVE)
            }
        })
        .collect();

    // Hop 1 from tagged artists, hop 2 from hop-1 artists at 0.6.
    let mut hop1: HashMap<i64, BTreeMap<usize, f64>> = HashMap::new();
    for (index, id) in graph.ids.iter().enumerate() {
        if seeded.contains_key(id) {
            continue;
        }
        let mut scores: BTreeMap<usize, f64> = BTreeMap::new();
        let (targets, weights) = graph.csr.row(index);
        for (&target, &weight) in targets.iter().zip(weights) {
            if let Some(vector) = seeded.get(&graph.ids[target as usize]) {
                for (tag, value) in vector {
                    if prior[*tag] > 0.0 {
                        *scores.entry(*tag).or_default() += weight * value * prior[*tag];
                    }
                }
            }
        }
        if !scores.is_empty() {
            hop1.insert(*id, scores);
        }
    }
    let normalized = |scores: &BTreeMap<usize, f64>| -> Vec<(usize, f64)> {
        let top = scores.values().copied().fold(0.0, f64::max);
        if top <= 0.0 {
            return Vec::new();
        }
        scores.iter().map(|(tag, s)| (*tag, s / top)).collect()
    };
    let hop1_vectors: HashMap<i64, Vec<(usize, f64)>> = hop1
        .iter()
        .map(|(id, scores)| (*id, normalized(scores)))
        .collect();
    let mut hop2: HashMap<i64, BTreeMap<usize, f64>> = HashMap::new();
    for (index, id) in graph.ids.iter().enumerate() {
        if seeded.contains_key(id) || hop1.contains_key(id) {
            continue;
        }
        let mut scores: BTreeMap<usize, f64> = BTreeMap::new();
        let (targets, weights) = graph.csr.row(index);
        for (&target, &weight) in targets.iter().zip(weights) {
            if let Some(vector) = hop1_vectors.get(&graph.ids[target as usize]) {
                for (tag, value) in vector {
                    *scores.entry(*tag).or_default() += HOP_DECAY * weight * value;
                }
            }
        }
        if !scores.is_empty() {
            hop2.insert(*id, scores);
        }
    }

    // Scene majority: what the scene's members are tagged as, their own
    // tags at full weight, borrowed ones at half (hop 1) and a quarter
    // (hop 2), so a scene with few tagged artists still has a say.
    let mut scene_support: BTreeMap<i64, (BTreeMap<usize, f64>, f64)> = BTreeMap::new();
    for (artist, scene) in scene_of {
        let (vector, weight) = if let Some(vector) = seeded.get(artist) {
            (vector.clone(), 1.0)
        } else if let Some(scores) = hop1.get(artist) {
            (normalized(scores), 0.5)
        } else if let Some(scores) = hop2.get(artist) {
            (normalized(scores), 0.25)
        } else {
            continue;
        };
        let entry = scene_support.entry(*scene).or_default();
        entry.1 += weight;
        for (tag, value) in vector {
            *entry.0.entry(tag).or_default() += value * weight;
        }
    }
    let mut majority: HashMap<i64, Vec<usize>> = HashMap::new();
    let mut scene_tags = BTreeMap::new();
    for (scene, (support, members)) in &scene_support {
        let floor = (SCENE_SUPPORT_SHARE * members).max(1.0);
        let mut ranked: Vec<(usize, f64)> = support
            .iter()
            .filter(|(_, weight)| **weight >= floor)
            .map(|(tag, weight)| (*tag, *weight))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        ranked.truncate(SCENE_MAJORITY_TAGS);
        let mut hints: Vec<(usize, f64)> = ranked
            .iter()
            .map(|(tag, weight)| (*tag, weight * (0.3 + 0.7 * prior[*tag])))
            .collect();
        hints.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        scene_tags.insert(
            *scene,
            hints
                .iter()
                .take(SCENE_HINT_TAGS)
                .map(|(tag, _)| names[*tag].clone())
                .collect::<Vec<_>>(),
        );
        majority.insert(*scene, ranked.into_iter().map(|(tag, _)| tag).collect());
    }

    let mut artists = HashMap::new();
    let everyone: std::collections::BTreeSet<i64> = scene_of
        .keys()
        .chain(seeded.keys())
        .chain(hop1.keys())
        .chain(hop2.keys())
        .copied()
        .collect();
    for artist in everyone {
        // Artists outside every scene keep what their few edges say.
        let scene = scene_of.get(&artist).copied();
        if let Some(vector) = seeded.get(&artist) {
            let scores: BTreeMap<usize, f64> = vector.iter().copied().collect();
            let genres = top_tags(&scores, None, &names);
            if !genres.is_empty() {
                artists.insert(artist, Spread { genres, hops: 0 });
            }
            continue;
        }
        let allowed = scene.and_then(|s| majority.get(&s)).map(Vec::as_slice);
        for (hops, layer) in [(1u8, &hop1), (2u8, &hop2)] {
            if let Some(scores) = layer.get(&artist) {
                let genres = top_tags(scores, allowed, &names);
                if !genres.is_empty() {
                    artists.insert(artist, Spread { genres, hops });
                }
                break;
            }
        }
    }
    SpreadResult {
        artists,
        scene_tags,
    }
}

fn load_scene_of(conn: &Connection) -> Result<(Vec<i64>, HashMap<i64, i64>)> {
    let mut stmt = conn.prepare("SELECT artist_tidal_id, scene_id FROM video_artist_facets")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let all = rows.iter().map(|(id, _)| *id).collect();
    let scene_of = rows
        .into_iter()
        .filter_map(|(id, scene)| scene.map(|scene| (id, scene)))
        .collect();
    Ok((all, scene_of))
}

/// Rebuild this stage's rows. Returns the number of artists given genres.
pub fn build(conn: &Connection) -> Result<usize> {
    let (all, scene_of) = load_scene_of(conn)?;
    let graph = scenes::cached_graph(conn)?;
    let seeds = load_seeds(conn)?;
    let result = spread(&graph, &seeds, &scene_of);
    let tx = conn.unchecked_transaction()?;
    let mut written = 0;
    {
        let mut update = tx.prepare(
            "UPDATE video_artist_facets SET genres_json = ?2, genre_hops = ?3
              WHERE artist_tidal_id = ?1",
        )?;
        for id in &all {
            match result.artists.get(id) {
                Some(spread) => {
                    update.execute(params![
                        id,
                        serde_json::to_string(&spread.genres)?,
                        spread.hops
                    ])?;
                    written += 1;
                }
                None => {
                    update.execute(params![id, "[]", Option::<i64>::None])?;
                }
            }
        }
        let mut tags = tx.prepare("UPDATE video_scenes SET tags_json = ?2 WHERE scene_id = ?1")?;
        for (scene, hints) in &result.scene_tags {
            tags.execute(params![scene, serde_json::to_string(hints)?])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// Share of charted artists (with a scene) that carry at least one genre.
/// The census computes the same number for the app; this one serves tests.
#[cfg(test)]
pub fn coverage(conn: &Connection) -> Result<f64> {
    let (charted, covered): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(genres_json <> '[]'), 0)
           FROM video_artist_facets WHERE charted = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(if charted > 0 {
        covered as f64 / charted as f64
    } else {
        0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeds(entries: &[(i64, &[(&str, f64)])]) -> Seeds {
        entries
            .iter()
            .map(|(id, tags)| (*id, tags.iter().map(|(t, w)| (t.to_string(), *w)).collect()))
            .collect()
    }

    #[test]
    fn tags_fold_to_one_spelling_and_drop_noise() {
        assert_eq!(normalize_tag("Hip-Hop").as_deref(), Some("hip hop"));
        assert_eq!(normalize_tag("R&B").as_deref(), Some("r&b"));
        assert_eq!(
            normalize_tag("Drum & Bass").as_deref(),
            Some("drum and bass")
        );
        assert_eq!(normalize_tag("K-Pop").as_deref(), Some("k-pop"));
        assert_eq!(
            normalize_tag(" Indie  Rock ").as_deref(),
            Some("indie rock")
        );
        for noise in ["seen live", "80s", "1990s", "British", "x", ""] {
            assert_eq!(normalize_tag(noise), None, "{noise}");
        }
    }

    #[test]
    fn genres_spread_two_hops_with_decay_and_a_rarity_prior() {
        // 1 and 2 are tagged; 3 touches both, 4 sits behind 3, 5 is three
        // hops out. Every artist is in scene 1.
        let graph = ArtistGraph::from_rows(&[
            (1, 3, 0, 0.9),
            (2, 3, 0, 0.9),
            (3, 4, 0, 0.9),
            (4, 5, 0, 0.9),
            (6, 7, 0, 0.9),
            (8, 9, 0, 0.9),
        ]);
        let seeds = seeds(&[
            (1, &[("pop", 1.0), ("reggaeton", 0.9)]),
            (2, &[("pop", 1.0), ("reggaeton", 0.9)]),
            (6, &[("pop", 1.0)]),
            (8, &[("pop", 1.0)]),
        ]);
        let scene_of: HashMap<i64, i64> = (1..=5).map(|id| (id, 1)).collect();
        let result = spread(&graph, &seeds, &scene_of);
        let three = &result.artists[&3];
        assert_eq!(three.hops, 1);
        assert_eq!(three.genres[0], "reggaeton", "the rarer tag leads");
        assert!(three.genres.contains(&"pop".to_string()));
        assert_eq!(result.artists[&4].hops, 2);
        assert!(!result.artists.contains_key(&5), "two hops at most");
        assert_eq!(result.artists[&1].hops, 0);
        assert_eq!(result.scene_tags[&1][0], "reggaeton");
    }

    #[test]
    fn borrowed_tags_must_agree_with_the_scene() {
        // Artist 3 sits in the rock scene (with 1 and 2) but has one edge to
        // a classical artist in another scene.
        let graph = ArtistGraph::from_rows(&[
            (1, 2, 0, 0.9),
            (3, 10, 0, 0.9),
            (3, 1, 0, 0.3),
            (10, 11, 0, 0.9),
        ]);
        let seeds = seeds(&[
            (1, &[("rock", 1.0)]),
            (2, &[("rock", 1.0), ("metal", 0.8)]),
            (10, &[("classical", 1.0)]),
            (11, &[("classical", 1.0)]),
        ]);
        let scene_of: HashMap<i64, i64> = [(1, 1), (2, 1), (3, 1), (10, 2), (11, 2)]
            .into_iter()
            .collect();
        let result = spread(&graph, &seeds, &scene_of);
        assert_eq!(result.artists[&3].genres, vec!["rock".to_string()]);
    }

    #[test]
    fn build_writes_genres_and_scene_hints() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                 VALUES (1, 3, 'x', 'lastfm', 0.9), (2, 3, 'x', 'lastfm', 0.9);
             INSERT INTO video_seed_genres (seed_tidal_id, genre_name, rank)
                 VALUES (1, 'Reggaeton', 0), (2, 'Reggaeton', 0), (2, 'Seen Live', 1);
             INSERT INTO video_artist_facets (artist_tidal_id, name, scene_id, charted)
                 VALUES (1, 'One', 5, 1), (2, 'Two', 5, 1), (3, 'Three', 5, 1), (4, 'Four', NULL, 0);
             INSERT INTO video_scenes (scene_id, name) VALUES (5, 'Five');",
        )
        .unwrap();
        assert_eq!(build(&conn).unwrap(), 3);
        let (genres, hops): (String, Option<i64>) = conn
            .query_row(
                "SELECT genres_json, genre_hops FROM video_artist_facets WHERE artist_tidal_id = 3",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((genres.as_str(), hops), (r#"["reggaeton"]"#, Some(1)));
        let tags: String = conn
            .query_row(
                "SELECT tags_json FROM video_scenes WHERE scene_id = 5",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tags, r#"["reggaeton"]"#);
        assert!((coverage(&conn).unwrap() - 1.0).abs() < 1e-9);
    }
}
