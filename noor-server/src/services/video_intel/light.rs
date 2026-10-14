//! Light is knowledge: per artist 0 uncharted, 1 dim (charted, untouched),
//! 2 warm (library artist or one hop from a lit or library artist), 3 lit
//! (watched, liked or saved). Computed per request from the listener's
//! current state; cheap enough to never cache.
//!
//! Each level is the most the listener knows: a watched artist is lit even
//! off the map, a library artist on the uncharted ring is still warm.

use std::collections::HashSet;

use anyhow::Result;
use rusqlite::Connection;

use super::scenes::ArtistGraph;
use crate::services::video_discovery::roots::LIKED_ARTISTS_SQL;

pub const UNCHARTED: u8 = 0;
pub const DIM: u8 = 1;
pub const WARM: u8 = 2;
pub const LIT: u8 = 3;

#[derive(Debug, Default, Clone)]
pub struct LightMap {
    lit: HashSet<i64>,
    warm: HashSet<i64>,
}

impl LightMap {
    /// Warm is the library plus one hop (either direction) from anything lit
    /// or in the library.
    pub fn new(lit: HashSet<i64>, library: &HashSet<i64>, graph: &ArtistGraph) -> Self {
        let mut warm = library.clone();
        for id in lit.iter().chain(library.iter()) {
            warm.extend(graph.neighbors(*id).map(|(neighbor, _, _)| neighbor));
        }
        Self { lit, warm }
    }

    pub fn light(&self, artist_id: i64, charted: bool) -> u8 {
        if self.lit.contains(&artist_id) {
            LIT
        } else if self.warm.contains(&artist_id) {
            WARM
        } else if charted {
            DIM
        } else {
            UNCHARTED
        }
    }

    pub fn is_lit(&self, artist_id: i64) -> bool {
        self.lit.contains(&artist_id)
    }
}

fn ids(conn: &Connection, sql: &str) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, Option<i64>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().flatten().filter(|id| *id > 0).collect())
}

/// Watched (any play in video history), liked tracks and albums, saved
/// videos.
pub fn lit_artists(conn: &Connection) -> Result<HashSet<i64>> {
    let mut lit = ids(
        conn,
        "SELECT DISTINCT artist_tidal_id FROM video_history WHERE artist_tidal_id > 0",
    )?;
    lit.extend(ids(conn, LIKED_ARTISTS_SQL)?);
    Ok(lit)
}

/// Every artist with a track in the library.
pub fn library_artists(conn: &Connection) -> Result<HashSet<i64>> {
    ids(
        conn,
        "SELECT a.tidal_id FROM artists a
          WHERE a.tidal_id > 0 AND EXISTS (SELECT 1 FROM tracks t WHERE t.artist_id = a.id)",
    )
}

pub fn load(conn: &Connection, graph: &ArtistGraph) -> Result<LightMap> {
    Ok(LightMap::new(
        lit_artists(conn)?,
        &library_artists(conn)?,
        graph,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_ranks_lit_over_warm_over_dim_over_uncharted() {
        let graph = ArtistGraph::from_rows(&[(1, 2, 0, 0.9), (3, 4, 1, 0.5), (5, 6, 0, 0.9)]);
        let lit = HashSet::from([1, 9]);
        let library = HashSet::from([4, 8]);
        let light = LightMap::new(lit, &library, &graph);
        assert_eq!(light.light(1, true), LIT);
        assert_eq!(light.light(9, false), LIT, "a watched artist off the map");
        assert_eq!(light.light(2, true), WARM, "one hop from lit");
        assert_eq!(
            light.light(3, true),
            WARM,
            "one hop from the library, reverse edge"
        );
        assert_eq!(light.light(8, false), WARM, "library on the ring");
        assert_eq!(light.light(5, true), DIM);
        assert_eq!(light.light(7, false), UNCHARTED);
    }

    #[test]
    fn load_reads_history_likes_saves_and_library() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, tidal_id, name) VALUES (1, 101, 'Liked'), (2, 102, 'Library'), (3, 103, 'Empty');
             INSERT INTO tracks (id, artist_id, title, is_favorite) VALUES (1, 1, 'A', 1), (2, 2, 'B', 0);
             INSERT INTO video_history (tidal_video_id, artist_tidal_id, artist_name) VALUES (9, 201, 'Watched');
             INSERT INTO saved_videos (tidal_video_id, item_json) VALUES (8, '{\"artist_id\":301}');
             INSERT INTO video_related_artists (seed_tidal_id, related_tidal_id, name, source, weight)
                 VALUES (500, 201, 'x', 'tidal', 0.9);",
        )
        .unwrap();
        let graph = ArtistGraph::load(&conn).unwrap();
        let light = load(&conn, &graph).unwrap();
        for lit in [101, 201, 301] {
            assert_eq!(light.light(lit, true), LIT, "{lit}");
        }
        assert_eq!(light.light(102, true), WARM);
        assert_eq!(light.light(500, true), WARM);
        assert_eq!(
            light.light(103, true),
            DIM,
            "an artist with no tracks is not library"
        );
    }
}
