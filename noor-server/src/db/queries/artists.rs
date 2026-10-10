//! Artist queries and the public Spotify stats cache.

use super::*;

// ─── Artists ──────────────────────────────────────────────

// ─── Spotify public stats (richer cache reads/writes) ─────

#[derive(Debug, Clone, Default)]
pub struct CachedTrackStatsRow {
    pub spotify_track_id: Option<String>,
    pub playcount: Option<i64>,
    pub stats_fetched_at: Option<i64>,
    pub null_cached_at: Option<i64>,
}

/// Read raw spotify cache state for a batch of ISRCs.
///
/// Returns one row per *input* ISRC (including unknowns, so callers can tell
/// "never seen" apart from "negative-cached"). TTL policy is the caller's
/// responsibility - this is intentionally just a window into the tables.
pub fn get_cached_spotify_track_stats_for_isrcs(
    conn: &Connection,
    isrcs: &[String],
) -> Result<HashMap<String, CachedTrackStatsRow>> {
    const CHUNK: usize = 500;
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for isrc in isrcs {
        let trimmed = isrc.trim();
        if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
            continue;
        }
        keys.push(trimmed.to_string());
    }
    if keys.is_empty() {
        return Ok(HashMap::new());
    }

    let mut out: HashMap<String, CachedTrackStatsRow> = keys
        .iter()
        .map(|k| (k.clone(), CachedTrackStatsRow::default()))
        .collect();

    for chunk in keys.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(",");

        // ISRC -> spotify_track_id + (optional) playcount/fetched_at via LEFT JOIN
        let sql = format!(
            "SELECT m.isrc, m.spotify_track_id, s.playcount, s.fetched_at
             FROM spotify_isrc_map m
             LEFT JOIN spotify_track_stats s ON s.spotify_track_id = m.spotify_track_id
             WHERE m.isrc IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })?;
        for r in rows {
            let (isrc, tid, pc, fa) = r?;
            if let Some(slot) = out.get_mut(&isrc) {
                slot.spotify_track_id = Some(tid);
                slot.playcount = pc;
                slot.stats_fetched_at = fa;
            }
        }

        // Negative cache lookup (no join: spotify_null_cache is keyed by ISRC).
        let null_sql = format!(
            "SELECT isrc, cached_at FROM spotify_null_cache WHERE isrc IN ({placeholders})"
        );
        let mut null_stmt = conn.prepare(&null_sql)?;
        let null_rows = null_stmt.query_map(params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for r in null_rows {
            let (isrc, cached_at) = r?;
            if let Some(slot) = out.get_mut(&isrc) {
                slot.null_cached_at = Some(cached_at);
            }
        }
    }

    Ok(out)
}

#[derive(Debug, Clone)]
pub struct CachedArtistStatsRow {
    pub monthly_listeners: Option<i64>,
    pub followers: Option<i64>,
    pub world_rank: Option<i64>,
    pub top_cities_json: Option<String>,
    pub fetched_at: i64,
}

pub fn get_cached_spotify_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
) -> Result<Option<CachedArtistStatsRow>> {
    let row = conn
        .query_row(
            "SELECT monthly_listeners, followers, world_rank, top_cities_json, fetched_at
             FROM spotify_artist_stats
             WHERE spotify_artist_id = ?1",
            params![spotify_artist_id],
            |row| {
                Ok(CachedArtistStatsRow {
                    monthly_listeners: row.get(0)?,
                    followers: row.get(1)?,
                    world_rank: row.get(2)?,
                    top_cities_json: row.get(3)?,
                    fetched_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// Returns `(Option<spotify_artist_id>, resolved_at)`. `Some(None)` (i.e. row
/// exists with NULL spotify_artist_id) means negative-cached.
pub fn get_spotify_artist_map(
    conn: &Connection,
    tidal_artist_id: &str,
) -> Result<Option<(Option<String>, i64)>> {
    let row = conn
        .query_row(
            "SELECT spotify_artist_id, resolved_at FROM spotify_artist_map
             WHERE tidal_artist_id = ?1",
            params![tidal_artist_id],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    Ok(row)
}

pub fn upsert_spotify_isrc_map(
    conn: &Connection,
    isrc: &str,
    spotify_track_id: &str,
    resolved_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_isrc_map (isrc, spotify_track_id, resolved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(isrc) DO UPDATE SET
            spotify_track_id = excluded.spotify_track_id,
            resolved_at      = excluded.resolved_at",
        params![isrc, spotify_track_id, resolved_at],
    )?;
    Ok(())
}

/// Store playcount as-is (zero is preserved; the resolver may treat zero as
/// "retry sooner" but the cache layer doesn't second-guess the upstream).
pub fn upsert_spotify_track_stats(
    conn: &Connection,
    spotify_track_id: &str,
    playcount: i64,
    fetched_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_track_stats (spotify_track_id, playcount, fetched_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(spotify_track_id) DO UPDATE SET
            playcount  = excluded.playcount,
            fetched_at = excluded.fetched_at",
        params![spotify_track_id, playcount, fetched_at],
    )?;
    Ok(())
}

pub fn upsert_spotify_null_cache(conn: &Connection, isrc: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_null_cache (isrc, cached_at)
         VALUES (?1, ?2)
         ON CONFLICT(isrc) DO UPDATE SET cached_at = excluded.cached_at",
        params![isrc, now],
    )?;
    Ok(())
}

pub fn clear_spotify_null_cache(conn: &Connection, isrc: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM spotify_null_cache WHERE isrc = ?1",
        params![isrc],
    )?;
    Ok(())
}

pub fn upsert_spotify_artist_map(
    conn: &Connection,
    tidal_artist_id: &str,
    spotify_artist_id: Option<&str>,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_artist_map (tidal_artist_id, spotify_artist_id, resolved_at)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(tidal_artist_id) DO UPDATE SET
            spotify_artist_id = excluded.spotify_artist_id,
            resolved_at       = excluded.resolved_at",
        params![tidal_artist_id, spotify_artist_id, now],
    )?;
    Ok(())
}

/// Upsert artist stats; `COALESCE(excluded.col, col)` preserves any previously
/// known non-null value when the incoming fetch omitted that field (Spotify
/// drops `monthly_listeners` for some artists, but we don't want to forget a
/// number we'd already learned).
///
/// On a fresh INSERT this writes whatever the caller passes for each column,
/// including explicit NULL when the caller passes `None`. The schema declares
/// those columns nullable with no DEFAULT, so explicit-NULL and default-NULL
/// produce identical rows today. If a future migration adds a non-NULL DEFAULT
/// to any of these columns, callers that pass `None` will clobber that default
/// with NULL; the helper would then need a parallel "partial upsert" variant.
pub fn upsert_spotify_artist_stats(
    conn: &Connection,
    spotify_artist_id: &str,
    monthly_listeners: Option<i64>,
    followers: Option<i64>,
    world_rank: Option<i64>,
    top_cities_json: Option<&str>,
    fetched_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO spotify_artist_stats
            (spotify_artist_id, monthly_listeners, followers, world_rank, top_cities_json, fetched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(spotify_artist_id) DO UPDATE SET
            monthly_listeners = COALESCE(excluded.monthly_listeners, spotify_artist_stats.monthly_listeners),
            followers         = COALESCE(excluded.followers,         spotify_artist_stats.followers),
            world_rank        = COALESCE(excluded.world_rank,        spotify_artist_stats.world_rank),
            top_cities_json   = COALESCE(excluded.top_cities_json,   spotify_artist_stats.top_cities_json),
            fetched_at        = excluded.fetched_at",
        params![
            spotify_artist_id,
            monthly_listeners,
            followers,
            world_rank,
            top_cities_json,
            fetched_at
        ],
    )?;
    Ok(())
}

// Canonical sibling of `favorite_predicate`'s favorite_only branch (hand-
// duplicated because the artist queries join `albums` as `al`). Keep the two in
// sync: the album-favorite branch is gated on `is_library = 1` so transient
// resolver/discovery imports don't leak into artist-detail library surfaces.
pub(super) const ARTIST_LIBRARY_TRACK_WHERE: &str =
    "(t.is_favorite = 1 OR (COALESCE(al.is_favorite, 0) = 1 AND t.is_library = 1))";

pub(super) fn artist_library_track_predicate() -> &'static str {
    ARTIST_LIBRARY_TRACK_WHERE
}

/// Wrap a galaxy `track_genres` fragment so only curated-library tracks (the
/// exact predicate the Library grid uses, via `ARTIST_LIBRARY_TRACK_WHERE`)
/// contribute to Genre Galaxy aggregation. Hidden enrichment fill
/// (is_library = 0) keeps feeding radio/discovery through the unwrapped
/// fragments; the galaxy shows the user's curated taste only. Output shape
/// matches `filter_subquery`: `(track_id, genre_id, source, confidence)`.
pub(super) fn galaxy_library_gate(inner: &str) -> String {
    format!(
        "SELECT tg.track_id, tg.genre_id, tg.source, tg.confidence
         FROM ({inner}) tg
         JOIN tracks t ON t.id = tg.track_id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE {ARTIST_LIBRARY_TRACK_WHERE}"
    )
}

/// Turns artist photos stored as bare TIDAL image ids ("3a503460-3914-...")
/// into image URLs (750px: artist photos have no 640 size). An import path once stored the id itself, which the app
/// loaded as a relative path, so those artists showed only an initial.
/// Idempotent and cheap; runs at every startup and finds nothing once healed.
pub fn repair_bare_artist_photos(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE artists
            SET photo_url = 'https://resources.tidal.com/images/'
                || replace(photo_url, '-', '/') || '/750x750.jpg'
          WHERE photo_url LIKE '________-____-____-____-____________'
            AND length(photo_url) = 36",
        [],
    )?)
}

/// Liner-note facts for an album page: its TIDAL id, the stored label (an
/// empty string means "looked up, TIDAL had none") and its year.
pub fn get_album_credits(
    conn: &Connection,
    album_id: i64,
) -> Result<Option<(Option<i64>, Option<String>, Option<i32>)>> {
    Ok(conn
        .query_row(
            "SELECT tidal_id, label, year FROM albums WHERE id = ?1",
            params![album_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?)
}

pub fn set_album_label(conn: &Connection, album_id: i64, label: &str) -> Result<()> {
    conn.execute(
        "UPDATE albums SET label = ?2 WHERE id = ?1",
        params![album_id, label],
    )?;
    Ok(())
}

/// The label line from a TIDAL album's copyright ("(P) 2003 Parlophone
/// Records Ltd" -> "Parlophone Records Ltd"): copyright marks and years are
/// trimmed so the line reads as a name. Empty when TIDAL sends none.
pub fn label_from_copyright(copyright: Option<&str>) -> String {
    let Some(text) = copyright else {
        return String::new();
    };
    let mut rest = text.trim();
    loop {
        let before = rest;
        for mark in ["\u{2117}", "\u{a9}", "(P)", "(p)", "(C)", "(c)"] {
            if let Some(stripped) = rest.strip_prefix(mark) {
                rest = stripped.trim_start();
            }
        }
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 4 {
            rest = rest[digits..].trim_start_matches([',', ' ', '-']);
        }
        if rest == before {
            break;
        }
    }
    rest.trim().to_string()
}

/// Sort key for artist names: a leading "The " is ignored, so "The Beatles"
/// files under B.
pub(super) const ARTIST_SORT_KEY: &str =
    "CASE WHEN lower(a.name) LIKE 'the %' THEN substr(a.name, 5) ELSE a.name END";

/// Artist order shared by the list and its A to Z index: names that start
/// with a letter first (case-insensitive), then digits and symbols, so
/// "*NSYNC" and "070 Shake" no longer lead the list.
pub(super) fn artist_order_clause(dir: &str) -> String {
    format!(
        "CASE WHEN {key} GLOB '[A-Za-z]*' THEN 0 ELSE 1 END, {key} COLLATE NOCASE {dir}, a.id",
        key = ARTIST_SORT_KEY
    )
}

/// First list offset for each initial in the artist order, plus the total.
/// Initials are A-Z; everything else is "#" and sorts last.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArtistLetterOffset {
    pub letter: String,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtistLetterIndex {
    pub total: i64,
    pub letters: Vec<ArtistLetterOffset>,
}

pub fn get_artist_letter_index(conn: &Connection) -> Result<ArtistLetterIndex> {
    let sql = format!(
        "SELECT {key} FROM artists a ORDER BY {order}",
        key = ARTIST_SORT_KEY,
        order = artist_order_clause("ASC")
    );
    let mut stmt = conn.prepare(&sql)?;
    let keys = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut letters: Vec<ArtistLetterOffset> = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let letter = match key.trim_start().chars().next() {
            Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase().to_string(),
            _ => "#".to_string(),
        };
        if letters
            .last()
            .map(|entry| entry.letter != letter)
            .unwrap_or(true)
        {
            letters.push(ArtistLetterOffset {
                letter,
                offset: index as i64,
            });
        }
    }
    Ok(ArtistLetterIndex {
        total: keys.len() as i64,
        letters,
    })
}

pub fn get_artists(
    conn: &Connection,
    sort_by: &str,
    sort_dir: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Artist>> {
    // Name is the only artist sort today; keep the parameter for the API shape.
    let _ = sort_by;
    let dir = if sort_dir == "asc" { "ASC" } else { "DESC" };

    let sql = format!(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         ORDER BY {order}
         LIMIT ?1 OFFSET ?2",
        order = artist_order_clause(dir)
    );

    let mut stmt = conn.prepare(&sql)?;
    let artists = stmt
        .query_map(params![limit, offset], |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(artists)
}

/// Single artist row plus library-side counts (tracks belonging to this
/// artist, distinct albums those tracks span). Counts reflect the local
/// library only. TIDAL-side totals come from the discography handler.
pub fn get_artist_with_counts(
    conn: &Connection,
    artist_id: i64,
) -> Result<Option<(Artist, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.tidal_id, a.ytmusic_id, a.soundcloud_id,
                a.name, a.name_sort, a.biography, a.photo_url
         FROM artists a
         WHERE a.id = ?1",
    )?;
    let artist = stmt
        .query_row(params![artist_id], |row| {
            Ok(Artist {
                id: row.get(0)?,
                tidal_id: row.get(1)?,
                ytmusic_id: row.get(2)?,
                soundcloud_id: row.get(3)?,
                name: row.get(4)?,
                name_sort: row.get(5)?,
                biography: row.get(6)?,
                photo_url: row.get(7)?,
            })
        })
        .optional()?;

    let Some(artist) = artist else {
        return Ok(None);
    };

    let track_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE t.artist_id = ?1 AND {}",
            artist_library_track_predicate()
        ),
        params![artist_id],
        |row| row.get(0),
    )?;
    let album_count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT t.album_id) FROM tracks t
             LEFT JOIN albums al ON t.album_id = al.id
             WHERE t.artist_id = ?1
               AND t.album_id IS NOT NULL
               AND {}",
            artist_library_track_predicate()
        ),
        params![artist_id],
        |row| row.get(0),
    )?;

    Ok(Some((artist, track_count, album_count)))
}

pub fn get_artist_tidal_id(conn: &Connection, artist_id: i64) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT tidal_id FROM artists WHERE id = ?1")?;
    let tidal_id = stmt
        .query_row(params![artist_id], |row| row.get::<_, Option<i64>>(0))
        .optional()?
        .flatten();
    Ok(tidal_id)
}

pub fn get_known_artist_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = format!("SELECT tidal_id, id FROM artists WHERE tidal_id IN ({placeholders})");
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tidal_id, local_id) = row?;
        map.insert(tidal_id, local_id);
    }
    Ok(map)
}

pub fn get_known_album_tidal_ids(
    conn: &Connection,
    tidal_ids: &[i64],
) -> Result<HashMap<i64, i64>> {
    if tidal_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = placeholders(tidal_ids.len());
    let sql = if crate::db::catalogue::enabled(conn)? {
        format!(
            "SELECT tidal_id,album_id FROM tidal_album_aliases WHERE tidal_id IN ({placeholders})"
        )
    } else {
        format!("SELECT tidal_id,id FROM albums WHERE tidal_id IN ({placeholders})")
    };
    let params = params_from_iter(tidal_ids.iter().copied());
    let mut stmt = conn.prepare(&sql)?;
    let mut map = HashMap::new();
    let rows = stmt.query_map(params, |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (tidal_id, local_id) = row?;
        map.insert(tidal_id, local_id);
    }
    Ok(map)
}

pub(super) fn get_artist_tracks_matching(
    conn: &Connection,
    artist_id: i64,
    extra_where: &str,
) -> Result<Vec<Track>> {
    let projection = track_projection("a");
    let sql = format!(
        "SELECT {projection}
         FROM tracks t
         LEFT JOIN artists a ON t.artist_id = a.id
         LEFT JOIN albums al ON t.album_id = al.id
         WHERE t.artist_id = ?1{extra_where}
         ORDER BY
            al.year ASC,
            COALESCE(t.disc_number, 1) ASC,
            COALESCE(t.track_number, 999999) ASC,
            t.title COLLATE NOCASE ASC"
    );
    let mut stmt = conn.prepare(&sql)?;

    let tracks = stmt
        .query_map(params![artist_id], track_from_row)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(tracks)
}
