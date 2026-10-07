//! Which catalog videos a station may play, with the listener facts the
//! picker needs. Reads only local tables.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::Connection;

use super::pick::Candidate;
use super::{StationId, Vibe};
use crate::services::video_discovery::names::name_key;
use crate::services::video_discovery::{graph, roots};
use crate::services::video_radio::video_song_key;
use crate::services::video_sets::VideoCandidate;

pub const VARIOUS_ARTISTS_ID: i64 = 2935;
const MIN_DURATION_S: i64 = 90;
const MIN_TRACKS_PER_ARTIST: i64 = 2;
const EXCLUDED_TYPES: [&str; 2] = ["Interview", "Promotional"];
const EXCLUDED_TITLE_WORDS: [&str; 4] = ["trailer", "teaser", "interview", "behind the scenes"];
const LIVE_TITLE_WORDS: [&str; 5] = ["live", "unplugged", "acoustic", "session", "in concert"];
const FEAT_MARKERS: [&str; 3] = ["feat.", "ft.", " with "];

/// Genre names and context tags that define each vibe. Tune here.
pub fn vibe_terms(vibe: Vibe) -> (&'static [&'static str], &'static [&'static str]) {
    match vibe {
        Vibe::Psychedelic => (
            &[
                "Psychedelic Rock",
                "Neo-Psychedelia",
                "Acid Rock",
                "Trip-Hop",
                "Space Rock",
            ],
            &[],
        ),
        Vibe::Atmosphere => (
            &["Ambient", "Dream Pop", "Shoegaze", "Downtempo"],
            &["chill", "chilled", "mellow", "dreamy"],
        ),
        Vibe::Dance => (&[], &["dance", "club", "party"]),
        Vibe::Mellow => (&[], &["mellow", "beautiful", "sad", "melancholy"]),
        Vibe::Dark => (
            &["Darkwave", "Gothic Rock", "Industrial", "Dark Ambient"],
            &["dark"],
        ),
    }
}

fn has_word(haystack: &str, word: &str) -> bool {
    haystack.match_indices(word).any(|(index, _)| {
        let before = haystack[..index].chars().next_back();
        let after = haystack[index + word.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Never part of any station: compilations, clips, interviews and promos.
pub fn is_excluded(video: &VideoCandidate) -> bool {
    let title = video.title.to_lowercase();
    video.artist_id == Some(VARIOUS_ARTISTS_ID)
        || video
            .artist_name
            .as_deref()
            .is_some_and(|name| name_key(name) == "various artists")
        || video.duration_s.is_some_and(|d| d < MIN_DURATION_S)
        || video
            .video_type
            .as_deref()
            .is_some_and(|kind| EXCLUDED_TYPES.contains(&kind))
        || EXCLUDED_TITLE_WORDS
            .iter()
            .any(|word| has_word(&title, word))
}

fn is_duet(video: &VideoCandidate) -> bool {
    let title = video.title.to_lowercase();
    !video.featured_artist_ids.is_empty() || FEAT_MARKERS.iter().any(|m| title.contains(m))
}

fn is_live(video: &VideoCandidate) -> bool {
    let title = video.title.to_lowercase();
    video.video_type.as_deref() == Some("Live")
        || LIVE_TITLE_WORDS.iter().any(|word| has_word(&title, word))
}

/// What the listener has watched, skipped, likes and plays.
#[derive(Debug, Default)]
pub struct Listener {
    pub watched: HashSet<i64>,
    pub watched_artists: HashSet<i64>,
    pub skips: HashMap<i64, u32>,
    pub relevance: HashMap<i64, f64>,
    pub liked: HashSet<i64>,
    pub artist_plays: HashMap<i64, i64>,
}

pub fn load_listener(conn: &Connection) -> Result<Listener> {
    let mut listener = Listener::default();
    {
        let mut stmt = conn.prepare(
            "SELECT tidal_video_id, artist_tidal_id, duration_watched_ms, video_duration_ms
               FROM video_history",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        for row in rows {
            let (video, artist, watched_ms, duration_ms) = row?;
            listener.watched.insert(video);
            if let Some(artist) = artist.filter(|id| *id > 0) {
                listener.watched_artists.insert(artist);
            }
            let skipped = watched_ms.is_some_and(|watched_ms| {
                roots::is_skip(&roots::WatchRow {
                    artist_id: 0,
                    age_days: 0.0,
                    watched_ms,
                    duration_ms,
                    completed: false,
                })
            });
            if skipped {
                *listener.skips.entry(video).or_default() += 1;
            }
        }
    }
    let liked = roots::liked_roots(conn)?;
    listener.liked = liked.iter().map(|root| root.artist_id).collect();
    let enjoyed = roots::enjoyed_roots(conn, &listener.liked)?;
    let graph = graph::cached(conn)?;
    let pairs = |roots: &[roots::Root]| {
        roots
            .iter()
            .map(|root| (root.artist_id, root.weight))
            .collect::<Vec<_>>()
    };
    for (artist, relevance) in graph
        .propagate(&pairs(&liked), 2)
        .into_iter()
        .chain(graph.propagate(&pairs(&enjoyed), 3))
    {
        let score = listener.relevance.entry(artist).or_insert(0.0);
        *score = score.max(relevance.score);
    }
    let mut stmt = conn.prepare(
        "SELECT a.tidal_id, COUNT(*) FROM listen_history lh
           JOIN tracks t ON t.id = lh.track_id
           JOIN artists a ON a.id = t.artist_id
          WHERE a.tidal_id > 0 AND COALESCE(lh.source, '') NOT IN ('radio', 'automix')
          GROUP BY a.tidal_id",
    )?;
    for row in stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))? {
        let (artist, plays) = row?;
        listener.artist_plays.insert(artist, plays);
    }
    Ok(listener)
}

/// Catalog rows, optionally limited to some artists, minus the exclusions.
fn catalog(
    conn: &Connection,
    artists: Option<&HashSet<i64>>,
    extra_where: &str,
) -> Result<Vec<VideoCandidate>> {
    let ids_json = artists
        .map(|ids| serde_json::to_string(&ids.iter().collect::<Vec<_>>()))
        .transpose()?;
    let sql = format!(
        "SELECT item_json FROM video_catalog
          WHERE (?1 IS NULL OR artist_tidal_id IN (SELECT value FROM json_each(?1)))
          {extra_where}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([ids_json], |row| row.get::<_, String>(0))?;
    let mut out = Vec::new();
    for json in rows {
        if let Ok(video) = serde_json::from_str::<VideoCandidate>(&json?)
            && !is_excluded(&video)
        {
            out.push(video);
        }
    }
    Ok(out)
}

fn artist_set(conn: &Connection, sql: &str, param: &str) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([param], |row| row.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<HashSet<_>, _>>()?)
}

fn artists_by_genre_slug(conn: &Connection, slug: &str) -> Result<HashSet<i64>> {
    artist_set(
        conn,
        &format!(
            "SELECT a.tidal_id FROM artists a
               JOIN tracks t ON t.artist_id = a.id
               JOIN track_genres tg ON tg.track_id = t.id
               JOIN genres g ON g.id = tg.genre_id
              WHERE a.tidal_id > 0 AND g.slug = ?1
              GROUP BY a.tidal_id HAVING COUNT(DISTINCT t.id) >= {MIN_TRACKS_PER_ARTIST}
             UNION
             SELECT vg.seed_tidal_id FROM video_seed_genres vg
               JOIN genres g ON g.name = vg.genre_name COLLATE NOCASE
              WHERE g.slug = ?1"
        ),
        slug,
    )
}

fn artists_by_genre_names(conn: &Connection, names: &[&str]) -> Result<HashSet<i64>> {
    if names.is_empty() {
        return Ok(HashSet::new());
    }
    artist_set(
        conn,
        &format!(
            "SELECT a.tidal_id FROM artists a
               JOIN tracks t ON t.artist_id = a.id
               JOIN track_genres tg ON tg.track_id = t.id
               JOIN genres g ON g.id = tg.genre_id
              WHERE a.tidal_id > 0 AND g.name IN (SELECT value FROM json_each(?1))
              GROUP BY a.tidal_id HAVING COUNT(DISTINCT t.id) >= {MIN_TRACKS_PER_ARTIST}"
        ),
        &serde_json::to_string(names)?,
    )
}

fn artists_by_tags(conn: &Connection, tags: &[&str]) -> Result<HashSet<i64>> {
    if tags.is_empty() {
        return Ok(HashSet::new());
    }
    artist_set(
        conn,
        &format!(
            "SELECT a.tidal_id FROM artists a
               JOIN tracks t ON t.artist_id = a.id
               JOIN track_context_tags ct ON ct.track_id = t.id
              WHERE a.tidal_id > 0 AND ct.normalized_tag IN (SELECT value FROM json_each(?1))
              GROUP BY a.tidal_id HAVING COUNT(DISTINCT t.id) >= {MIN_TRACKS_PER_ARTIST}"
        ),
        &serde_json::to_string(tags)?,
    )
}

fn chart_key(artist: &str, title: &str) -> String {
    video_song_key(None, Some(&name_key(artist)), title)
}

/// Best chart position per (artist, song) on the latest snapshot of each chart.
fn chart_ranks(conn: &Connection) -> Result<HashMap<String, i64>> {
    let mut stmt = conn.prepare(
        "SELECT e.artist, e.title, e.rank FROM chart_entries e
          WHERE e.entity_type = 'track'
            AND e.snapshot_id IN (SELECT MAX(id) FROM chart_snapshots GROUP BY source_key)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut ranks: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (artist, title, rank) = row?;
        let lead = artist
            .split([',', '&'])
            .next()
            .unwrap_or(&artist)
            .split(" feat")
            .next()
            .unwrap_or(&artist)
            .split(" ft.")
            .next()
            .unwrap_or(&artist)
            .to_string();
        for name in [artist.as_str(), lead.as_str()] {
            let best = ranks.entry(chart_key(name, &title)).or_insert(rank);
            *best = (*best).min(rank);
        }
    }
    Ok(ranks)
}

fn candidate(video: VideoCandidate, listener: &Listener, chart_rank: Option<i64>) -> Candidate {
    let artist = video.artist_id.unwrap_or(0);
    Candidate {
        watched: listener.watched.contains(&video.tidal_id),
        skips: listener.skips.get(&video.tidal_id).copied().unwrap_or(0),
        relevance: listener.relevance.get(&artist).copied().unwrap_or(0.0),
        liked_artist: listener.liked.contains(&artist),
        artist_plays: listener.artist_plays.get(&artist).copied().unwrap_or(0),
        chart_rank,
        video,
    }
}

pub fn candidates(
    conn: &Connection,
    listener: &Listener,
    station: &StationId,
) -> Result<Vec<Candidate>> {
    let videos = match station {
        StationId::WildCard | StationId::Shuffle => catalog(conn, None, "")?,
        StationId::BigOnes => catalog(
            conn,
            None,
            "AND json_extract(item_json, '$.popularity') IS NOT NULL",
        )?,
        StationId::DeepCuts => catalog(conn, Some(&listener.liked), "")?,
        StationId::Genre(slug) => catalog(conn, Some(&artists_by_genre_slug(conn, slug)?), "")?,
        StationId::Vibe(vibe) => {
            let (genres, tags) = vibe_terms(*vibe);
            let mut artists = artists_by_genre_names(conn, genres)?;
            artists.extend(artists_by_tags(conn, tags)?);
            catalog(conn, Some(&artists), "")?
        }
        StationId::Duets => catalog(conn, None, "")?
            .into_iter()
            .filter(is_duet)
            .collect(),
        StationId::Live => catalog(conn, None, "")?
            .into_iter()
            .filter(is_live)
            .collect(),
        StationId::Spotlight(artist) => catalog(conn, Some(&HashSet::from([*artist])), "")?,
        StationId::Charts => {
            let ranks = chart_ranks(conn)?;
            if ranks.is_empty() {
                return Ok(Vec::new());
            }
            return Ok(catalog(conn, None, "")?
                .into_iter()
                .filter_map(|video| {
                    let rank = ranks
                        .get(&chart_key(
                            video.artist_name.as_deref().unwrap_or(""),
                            &video.title,
                        ))
                        .copied()?;
                    Some(candidate(video, listener, Some(rank)))
                })
                .collect());
        }
    };
    Ok(videos
        .into_iter()
        .map(|video| candidate(video, listener, None))
        .collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rusqlite::params;

    pub(crate) fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn
    }

    pub(crate) fn add_video(conn: &Connection, video: &VideoCandidate) {
        conn.execute(
            "INSERT INTO video_catalog (tidal_video_id, artist_tidal_id, artist_name, item_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                video.tidal_id,
                video.artist_id,
                video.artist_name,
                serde_json::to_string(video).unwrap()
            ],
        )
        .unwrap();
    }

    pub(crate) fn video(id: i64, artist: i64) -> VideoCandidate {
        VideoCandidate {
            tidal_id: id,
            title: format!("Song {id}"),
            duration_s: Some(200),
            artist_id: Some(artist),
            artist_name: Some(format!("Artist {artist}")),
            ..Default::default()
        }
    }

    fn pool_ids(conn: &Connection, station: StationId) -> Vec<i64> {
        let listener = load_listener(conn).unwrap();
        let mut ids: Vec<i64> = candidates(conn, &listener, &station)
            .unwrap()
            .iter()
            .map(|c| c.video.tidal_id)
            .collect();
        ids.sort();
        ids
    }

    fn library_artist_with_genre(
        conn: &Connection,
        local_id: i64,
        tidal_id: i64,
        genre: &str,
        tracks: i64,
    ) {
        conn.execute(
            "INSERT INTO artists (id, tidal_id, name) VALUES (?1, ?2, ?3)",
            params![local_id, tidal_id, format!("Artist {tidal_id}")],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO genres (name, slug) VALUES (?1, ?2)",
            params![genre, genre.to_lowercase().replace(' ', "-")],
        )
        .unwrap();
        let genre_id: i64 = conn
            .query_row("SELECT id FROM genres WHERE name = ?1", [genre], |r| {
                r.get(0)
            })
            .unwrap();
        for n in 0..tracks {
            let track_id = local_id * 100 + n;
            conn.execute(
                "INSERT INTO tracks (id, artist_id, title) VALUES (?1, ?2, ?3)",
                params![track_id, local_id, format!("Track {track_id}")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO track_genres (track_id, genre_id) VALUES (?1, ?2)",
                params![track_id, genre_id],
            )
            .unwrap();
        }
    }

    #[test]
    fn exclusions_apply_to_every_station() {
        let conn = conn();
        let mut various = video(1, VARIOUS_ARTISTS_ID);
        various.artist_name = Some("Various Artists".into());
        let mut short = video(2, 20);
        short.duration_s = Some(45);
        let mut interview = video(3, 30);
        interview.video_type = Some("Interview".into());
        let mut trailer = video(4, 40);
        trailer.title = "Official Trailer".into();
        for v in [various, short, interview, trailer, video(5, 50)] {
            add_video(&conn, &v);
        }
        assert_eq!(pool_ids(&conn, StationId::Shuffle), vec![5]);
        assert_eq!(pool_ids(&conn, StationId::WildCard), vec![5]);
    }

    #[test]
    fn genre_stations_need_two_library_tracks_per_artist() {
        let conn = conn();
        library_artist_with_genre(&conn, 1, 100, "Psychedelic Rock", 2);
        library_artist_with_genre(&conn, 2, 200, "Psychedelic Rock", 1);
        add_video(&conn, &video(1, 100));
        add_video(&conn, &video(2, 200));
        assert_eq!(
            pool_ids(&conn, StationId::Genre("psychedelic-rock".into())),
            vec![1]
        );
        assert_eq!(pool_ids(&conn, StationId::Vibe(Vibe::Psychedelic)), vec![1]);
    }

    #[test]
    fn mood_vibes_come_from_context_tags() {
        let conn = conn();
        conn.execute(
            "INSERT INTO artists (id, tidal_id, name) VALUES (1, 100, 'Artist 100')",
            [],
        )
        .unwrap();
        for track in [11, 12] {
            conn.execute(
                "INSERT INTO tracks (id, artist_id, title) VALUES (?1, 1, 'T')",
                [track],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO track_context_tags (track_id, tag, normalized_tag, context, source)
                 VALUES (?1, 'Dark', 'dark', 'mood', 'lastfm')",
                [track],
            )
            .unwrap();
        }
        add_video(&conn, &video(1, 100));
        add_video(&conn, &video(2, 200));
        assert_eq!(pool_ids(&conn, StationId::Vibe(Vibe::Dark)), vec![1]);
    }

    #[test]
    fn duets_and_live_match_facts_and_titles() {
        let conn = conn();
        let mut featured = video(1, 10);
        featured.featured_artist_ids = vec![20];
        let mut titled = video(2, 30);
        titled.title = "Song feat. Someone".into();
        let mut live = video(3, 40);
        live.title = "Song (Live at Wembley)".into();
        let mut deliver = video(4, 50);
        deliver.title = "Deliverance".into();
        for v in [featured, titled, live, deliver] {
            add_video(&conn, &v);
        }
        assert_eq!(pool_ids(&conn, StationId::Duets), vec![1, 2]);
        assert_eq!(pool_ids(&conn, StationId::Live), vec![3]);
    }

    #[test]
    fn charts_match_by_artist_and_song() {
        let conn = conn();
        conn.execute(
            "INSERT INTO chart_snapshots (id, source_key, region, period, chart_date, fetched_at, status)
             VALUES (1, 'spotify_daily', 'global', 'daily', '2026-10-06', 0, 'ok')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chart_entries (snapshot_id, rank, artist, title) VALUES
                 (1, 3, 'Artist 10 feat. Guest', 'Song 1'),
                 (1, 1, 'Artist 20', 'Nothing on camera')",
            [],
        )
        .unwrap();
        let mut official = video(1, 10);
        official.title = "Song 1 (Official Video)".into();
        add_video(&conn, &official);
        add_video(&conn, &video(2, 20));
        let listener = load_listener(&conn).unwrap();
        let pool = candidates(&conn, &listener, &StationId::Charts).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!((pool[0].video.tidal_id, pool[0].chart_rank), (1, Some(3)));
    }

    #[test]
    fn the_listener_knows_watches_and_skips() {
        let conn = conn();
        conn.execute_batch(
            "INSERT INTO video_history (tidal_video_id, artist_tidal_id, duration_watched_ms, video_duration_ms)
                 VALUES (1, 10, 5000, 200000), (1, 10, 4000, 200000), (2, 20, 190000, 200000),
                        (3, 30, NULL, NULL);",
        )
        .unwrap();
        let listener = load_listener(&conn).unwrap();
        assert_eq!(listener.watched, HashSet::from([1, 2, 3]));
        assert_eq!(listener.skips.get(&1), Some(&2));
        assert!(!listener.skips.contains_key(&2));
        assert!(
            !listener.skips.contains_key(&3),
            "an unfinished row is not a skip"
        );
    }
}
