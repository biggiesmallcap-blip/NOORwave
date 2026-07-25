//! The liked-videos library: videos for songs the user already favorited.
//!
//! The mirror of the /library Albums grid, but for videos. Distinct from the
//! editorial video sets (`services/video_sets.rs`), which fan out over artists'
//! videos to surface things the user does *not* have. This is the opposite: a
//! wall built only from likes they already own. The two share the video queue
//! and the persistent dock, nothing else.
//!
//! ## Why the scan unit is the artist
//!
//! One `/artists/{id}/videos` call returns an artist's whole video catalog, so
//! it resolves every liked song by that artist at once and gives a definitive
//! "no video" for artists with none. On the dev library that is 2,350 calls
//! covering 4,276 liked tracks, against 4,276 calls for a per-track video
//! search - and the artist is matched by TIDAL id rather than by name, so only
//! the title match is ever fuzzy.
//!
//! That also removes the need for a per-track negative cache: "this liked track
//! has no video" is implied by "its artist was scanned and nothing matched".
//! `library_video_scans` is the whole ledger.
//!
//! ## Matching
//!
//! Artist identity is exact. Titles go through `library::duplicates::base_title`,
//! which strips featured-artist and variant segments, so liked "Song" matches a
//! "Song (Live at Wembley)" video - accepting live takes, covers and alternates
//! is the point, and each hit becomes its own card. Anything that survives that
//! with a Jaro-Winkler score at or above `TITLE_MATCH_FLOOR` is kept.
//!
//! Pure functions over pre-fetched data plus small persistence helpers; the
//! TIDAL fan-out itself lives in the route module, under the global 4-inflight
//! semaphore.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use strsim::jaro_winkler;
use tracing::{debug, info, warn};

use crate::SharedState;
use crate::library::duplicates::base_title;
use crate::services::tidal::client::{TidalArtistVideo, TidalClient};

/// Re-check an artist this long after its last scan, in case TIDAL added a
/// video since. Long, because a miss is the overwhelmingly common answer and
/// re-asking is pure cost.
pub const RESCAN_AFTER_DAYS: i64 = 90;

/// Videos requested per artist. TIDAL caps a page well below this for all but
/// the largest catalogs, and an artist with more videos than this is not worth
/// a second round trip on a background pass.
pub const VIDEOS_PER_ARTIST: i32 = 50;

/// Jaro-Winkler floor for accepting a video title against a liked track title,
/// applied after both sides are reduced to their base title. Loose enough for
/// punctuation and subtitle drift, tight enough that a different song by the
/// same artist does not match.
pub const TITLE_MATCH_FLOOR: f64 = 0.90;

/// Artists resolved per pass. A first run on an existing library has thousands
/// of artists to get through; capping the burst keeps it a trickle in the
/// background instead of a sustained hammering of TIDAL, and the next trigger
/// picks up where this one stopped because the ledger is the queue.
pub const SCAN_BATCH_CAP: usize = 200;

/// Spacing between artist-videos calls. Matches the pacing `tidal::repair`
/// settled on for its own background sweep.
const SCAN_CALL_SPACING: Duration = Duration::from_millis(120);

/// `tracks.date_added` is ISO-8601 with a `+0000` offset that SQLite's date
/// functions cannot parse, while `scanned_at` is `datetime('now')` format.
/// Comparing them raw is wrong twice over: `julianday()` returns NULL on the
/// former, and `'T' > ' '` at index 10 makes any same-day like sort after any
/// same-day scan, which would re-scan that artist on every pass until midnight.
/// Trimming to seconds and swapping the separator puts both in one comparable
/// UTC format.
const DATE_ADDED_NORMALIZED: &str = "replace(substr(t.date_added, 1, 19), 'T', ' ')";

/// An artist with liked tracks whose videos need looking up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanTarget {
    pub artist_id: i64,
    pub tidal_artist_id: i64,
}

/// A liked song to find videos for.
#[derive(Debug, Clone)]
pub struct LikedTrack {
    pub track_id: i64,
    pub title: String,
}

/// One accepted (liked track, video) pair. Several may share a `track_id`.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoMatch {
    pub track_id: i64,
    pub tidal_video_id: i64,
    pub video_title: String,
    pub duration_seconds: Option<i64>,
    pub image_id: Option<String>,
    pub match_score: f64,
}

/// One card on the wall. Shaped so the client can lift it straight into a
/// `TidalSearchVideo` for the video queue: same `tidal_id` / `duration_ms` /
/// `artwork_url` fields, so Play all and Shuffle need no new playback code.
#[derive(Debug, Clone, Serialize)]
pub struct LikedVideoRow {
    pub track_id: i64,
    pub tidal_video_id: i64,
    pub video_title: String,
    pub track_title: String,
    pub artist_name: Option<String>,
    pub artist_id: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Built at 640x640 like every other artwork URL the backend emits; the
    /// grid downsizes through `upscaleTidalArtwork`.
    pub artwork_url: Option<String>,
    pub album_year: Option<i64>,
    pub genre: Option<String>,
    pub liked_at: Option<String>,
}

/// How far along the background pass is, for the view's progress line.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ScanProgress {
    pub scanned_artists: i64,
    pub total_artists: i64,
}

// ── Work selection ───────────────────────────────────────────────────────────

/// Artists that need a video lookup: they have liked tracks, they carry a TIDAL
/// id, and either they have never been scanned, their scan has aged out, or a
/// track was liked after the last scan.
///
/// That last clause is load-bearing. Scanning per artist means an artist that
/// has been seen once would otherwise never be revisited, so a newly liked song
/// by an artist already on the wall would never resolve.
pub fn artists_needing_scan(conn: &Connection) -> Result<Vec<ScanTarget>> {
    let sql = format!(
        "SELECT a.id, a.tidal_id
           FROM artists a
           JOIN tracks t ON t.artist_id = a.id AND t.is_favorite = 1
           LEFT JOIN library_video_scans s ON s.artist_id = a.id
          WHERE a.tidal_id IS NOT NULL
          GROUP BY a.id
         HAVING s.scanned_at IS NULL
             OR s.scanned_at < datetime('now', '-{RESCAN_AFTER_DAYS} days')
             OR MAX({DATE_ADDED_NORMALIZED}) > s.scanned_at
          ORDER BY s.scanned_at IS NOT NULL, a.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ScanTarget {
                artist_id: row.get(0)?,
                tidal_artist_id: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The liked songs belonging to one artist, i.e. what a single artist-videos
/// call has to satisfy.
pub fn liked_tracks_for_artist(conn: &Connection, artist_id: i64) -> Result<Vec<LikedTrack>> {
    let mut stmt = conn.prepare(
        "SELECT id, title FROM tracks WHERE artist_id = ?1 AND is_favorite = 1 AND title IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([artist_id], |row| {
            Ok(LikedTrack {
                track_id: row.get(0)?,
                title: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn scan_progress(conn: &Connection) -> Result<ScanProgress> {
    let total: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT a.id)
           FROM artists a
           JOIN tracks t ON t.artist_id = a.id AND t.is_favorite = 1
          WHERE a.tidal_id IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    let scanned: i64 = conn.query_row(
        "SELECT COUNT(*)
           FROM library_video_scans s
          WHERE EXISTS (
                SELECT 1 FROM tracks t
                 WHERE t.artist_id = s.artist_id AND t.is_favorite = 1)",
        [],
        |row| row.get(0),
    )?;
    Ok(ScanProgress {
        scanned_artists: scanned.min(total),
        total_artists: total,
    })
}

// ── Matching ─────────────────────────────────────────────────────────────────

/// Pair an artist's videos against that artist's liked songs. Every accepted
/// pair is kept, so one liked song can yield several cards and one video can
/// answer for several liked songs (a track present on two albums, say).
pub fn match_videos(tracks: &[LikedTrack], videos: &[TidalArtistVideo]) -> Vec<VideoMatch> {
    let track_keys: Vec<(usize, String)> = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| (i, base_title(&t.title)))
        .filter(|(_, key)| !key.is_empty())
        .collect();

    let mut matches = Vec::new();
    for video in videos {
        let video_key = base_title(&video.title);
        if video_key.is_empty() {
            continue;
        }
        for (idx, track_key) in &track_keys {
            let score = if *track_key == video_key {
                1.0
            } else {
                jaro_winkler(track_key, &video_key)
            };
            if score < TITLE_MATCH_FLOOR {
                continue;
            }
            matches.push(VideoMatch {
                track_id: tracks[*idx].track_id,
                tidal_video_id: video.id,
                video_title: video.title.clone(),
                duration_seconds: Some(video.duration),
                image_id: video.image_id.clone(),
                match_score: score,
            });
        }
    }
    matches
}

// ── Persistence ──────────────────────────────────────────────────────────────

/// Record one artist's scan: upsert its hits and stamp the ledger. Both happen
/// in one transaction so a crash mid-write cannot leave an artist marked
/// scanned with none of its videos stored.
///
/// The upsert deliberately leaves `suppressed` alone. A user who hid a wrong
/// match expects it to stay hidden, and without this the 90-day re-check would
/// resurrect every correction they ever made.
/// Takes `&Connection` rather than `&mut` so the caller can hold the shared
/// pooled connection for just this write. The TIDAL call that produced
/// `matches` happens outside any lock.
pub fn store_artist_scan(conn: &Connection, artist_id: i64, matches: &[VideoMatch]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO library_videos
                 (track_id, tidal_video_id, video_title, duration_seconds, image_id, match_score)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(track_id, tidal_video_id) DO UPDATE SET
                 video_title      = excluded.video_title,
                 duration_seconds = excluded.duration_seconds,
                 image_id         = excluded.image_id,
                 match_score      = excluded.match_score",
        )?;
        for m in matches {
            stmt.execute(params![
                m.track_id,
                m.tidal_video_id,
                m.video_title,
                m.duration_seconds,
                m.image_id,
                m.match_score,
            ])?;
        }
        tx.execute(
            "INSERT INTO library_video_scans (artist_id, scanned_at, video_count)
             VALUES (?1, datetime('now'), ?2)
             ON CONFLICT(artist_id) DO UPDATE SET
                 scanned_at  = excluded.scanned_at,
                 video_count = excluded.video_count",
            params![artist_id, matches.len() as i64],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// The wall. Ordered most recently liked first: a surface you revisit wants the
/// new arrivals at the top, and the client re-sorts A-Z on demand.
pub fn load_wall(conn: &Connection) -> Result<Vec<LikedVideoRow>> {
    let mut stmt = conn.prepare(
        "SELECT lv.track_id,
                lv.tidal_video_id,
                lv.video_title,
                t.title,
                ar.name,
                ar.id,
                lv.duration_seconds,
                lv.image_id,
                al.year,
                g.name,
                t.date_added
           FROM library_videos lv
           JOIN tracks t              ON t.id = lv.track_id AND t.is_favorite = 1
           LEFT JOIN artists ar       ON ar.id = t.artist_id
           LEFT JOIN albums al        ON al.id = t.album_id
           LEFT JOIN track_primary_genre pg ON pg.track_id = t.id
           LEFT JOIN genres g         ON g.id = pg.primary_genre_id
          WHERE lv.suppressed = 0
          ORDER BY t.date_added DESC, t.title ASC, lv.tidal_video_id ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let duration_seconds: Option<i64> = row.get(6)?;
            let image_id: Option<String> = row.get(7)?;
            Ok(LikedVideoRow {
                track_id: row.get(0)?,
                tidal_video_id: row.get(1)?,
                video_title: row.get(2)?,
                track_title: row.get(3)?,
                artist_name: row.get(4)?,
                artist_id: row.get(5)?,
                duration_ms: duration_seconds.map(|s| s * 1000),
                artwork_url: TidalClient::get_artwork_url(&image_id, 640),
                album_year: row.get(8)?,
                genre: row.get(9)?,
                liked_at: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// "Wrong match / hide this". Returns false when the pair is not on the wall,
/// so the route can answer 404 rather than pretending.
pub fn suppress(conn: &Connection, track_id: i64, tidal_video_id: i64) -> Result<bool> {
    let changed = conn.execute(
        "UPDATE library_videos SET suppressed = 1
          WHERE track_id = ?1 AND tidal_video_id = ?2",
        params![track_id, tidal_video_id],
    )?;
    Ok(changed > 0)
}

// ── Background pass ──────────────────────────────────────────────────────────

/// Resolve videos for liked artists that need it, in the background.
///
/// Wired next to `auto_enrich::run_if_idle` in `main.rs`: it fires on the
/// `LibrarySynced` broadcast and on the daily catch-up interval, so a sync only
/// ever has to finish - it never waits on TIDAL. A per-process atomic
/// (`library_video_scan_running`) makes overlapping triggers cheap no-ops.
///
/// Returns immediately after spawning. Without a TIDAL session there is nothing
/// to fetch, so it skips quietly and the next trigger after the user connects
/// picks the work back up.
pub async fn run_if_idle(state: SharedState) {
    let (db, running, tokens, tidal_http) = {
        let s = state.read().await;
        (
            s.db.clone(),
            s.library_video_scan_running.clone(),
            s.tidal_tokens.clone(),
            s.tidal_http_client.clone(),
        )
    };

    if running.load(Ordering::SeqCst) {
        debug!(target: "noor.library_videos", "already running, skipping");
        return;
    }

    let Some(tokens) = tokens else {
        debug!(target: "noor.library_videos", "TIDAL not connected, skipping");
        return;
    };

    let targets = match db.with_conn(artists_needing_scan) {
        Ok(targets) => targets,
        Err(e) => {
            warn!(target: "noor.library_videos", error = %e, "could not select work");
            return;
        }
    };
    if targets.is_empty() {
        debug!(target: "noor.library_videos", "every liked artist is up to date");
        return;
    }

    let batch: Vec<ScanTarget> = targets.into_iter().take(SCAN_BATCH_CAP).collect();
    info!(
        target: "noor.library_videos",
        artists = batch.len(),
        "resolving videos for liked artists"
    );
    running.store(true, Ordering::SeqCst);

    tokio::spawn(async move {
        let client = TidalClient::with_http(
            tidal_http,
            tokens.access_token.clone(),
            tokens.country_code.clone(),
        );

        let mut scanned = 0usize;
        let mut hits = 0usize;
        for target in batch {
            let tracks = match db.with_conn(|conn| liked_tracks_for_artist(conn, target.artist_id))
            {
                Ok(tracks) if !tracks.is_empty() => tracks,
                // The artist lost its likes between selection and now, or the
                // read failed. Either way there is nothing to match against;
                // leave it unscanned rather than banking an empty answer.
                _ => continue,
            };

            let videos = match client
                .get_artist_videos(target.tidal_artist_id, VIDEOS_PER_ARTIST, 0)
                .await
            {
                Ok(page) => page.items,
                Err(e) => {
                    // Do not stamp the ledger on failure: an unstamped artist is
                    // simply picked up again next pass.
                    debug!(
                        target: "noor.library_videos",
                        artist_id = target.artist_id,
                        error = %e,
                        "artist videos fetch failed, leaving unscanned"
                    );
                    continue;
                }
            };

            let matches = match_videos(&tracks, &videos);
            hits += matches.len();
            if let Err(e) = db.with_conn(|conn| store_artist_scan(conn, target.artist_id, &matches))
            {
                warn!(
                    target: "noor.library_videos",
                    artist_id = target.artist_id,
                    error = %e,
                    "could not store scan"
                );
                continue;
            }
            scanned += 1;
            tokio::time::sleep(SCAN_CALL_SPACING).await;
        }

        running.store(false, Ordering::SeqCst);
        info!(
            target: "noor.library_videos",
            scanned, hits,
            "liked-video pass complete"
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn video(id: i64, title: &str) -> TidalArtistVideo {
        TidalArtistVideo {
            id,
            title: title.to_string(),
            duration: 210,
            image_id: Some(format!("img-{id}")),
            artist: None,
            album: None,
            extra: Default::default(),
        }
    }

    fn liked(track_id: i64, title: &str) -> LikedTrack {
        LikedTrack {
            track_id,
            title: title.to_string(),
        }
    }

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO artists (id, name, tidal_id) VALUES (1, 'Anchor', 5001);
             INSERT INTO tracks (id, title, artist_id, is_favorite, date_added)
             VALUES (10, 'Song', 1, 1, '2024-07-22T03:55:48.611+0000');",
        )
        .unwrap();
        conn
    }

    #[test]
    fn live_takes_and_alternates_all_match_the_liked_song() {
        let tracks = vec![liked(10, "Song")];
        let videos = vec![
            video(900, "Song"),
            video(901, "Song (Live at Wembley)"),
            video(902, "Song - Acoustic Version"),
        ];

        let matches = match_videos(&tracks, &videos);

        assert_eq!(matches.len(), 3, "each take earns its own card");
        assert!(matches.iter().all(|m| m.track_id == 10));
        assert_eq!(
            matches[0].match_score, 1.0,
            "the plain title is an exact base match"
        );
        assert_eq!(matches[1].image_id.as_deref(), Some("img-901"));
        assert_eq!(matches[1].duration_seconds, Some(210));
    }

    #[test]
    fn a_different_song_by_the_same_artist_does_not_match() {
        let tracks = vec![liked(10, "Song")];
        let videos = vec![video(900, "Something Else Entirely")];

        assert!(match_videos(&tracks, &videos).is_empty());
    }

    #[test]
    fn featured_artist_noise_still_matches() {
        let tracks = vec![liked(10, "Song")];
        let videos = vec![video(900, "Song (feat. Someone)")];

        let matches = match_videos(&tracks, &videos);
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn unscanned_artist_is_work() {
        let conn = setup();
        let targets = artists_needing_scan(&conn).unwrap();
        assert_eq!(
            targets,
            vec![ScanTarget {
                artist_id: 1,
                tidal_artist_id: 5001,
            }]
        );
    }

    #[test]
    fn a_scanned_artist_is_not_rescanned_until_something_changes() {
        let conn = setup();
        store_artist_scan(&conn, 1, &[]).unwrap();

        assert!(
            artists_needing_scan(&conn).unwrap().is_empty(),
            "a fresh scan with no hits is still a complete answer"
        );

        // A like added after the scan brings the artist back. This is the only
        // way a new like by an already-scanned artist ever resolves.
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, is_favorite, date_added)
             VALUES (11, 'Newly Liked', 1, 1, datetime('now', '+1 hour'))",
            [],
        )
        .unwrap();
        assert_eq!(artists_needing_scan(&conn).unwrap().len(), 1);
    }

    #[test]
    fn a_same_day_like_does_not_loop_the_scan() {
        // date_added's `T` separator sorts above `datetime('now')`'s space, so
        // a naive string compare would re-queue this artist on every pass for
        // the rest of the day.
        let conn = setup();
        conn.execute(
            "UPDATE tracks SET date_added =
                 replace(datetime('now', '-1 minute'), ' ', 'T') || '.000+0000'
             WHERE id = 10",
            [],
        )
        .unwrap();
        store_artist_scan(&conn, 1, &[]).unwrap();

        assert!(
            artists_needing_scan(&conn).unwrap().is_empty(),
            "a like from earlier today is older than today's scan"
        );
    }

    #[test]
    fn a_stale_scan_is_rechecked() {
        let conn = setup();
        store_artist_scan(&conn, 1, &[]).unwrap();
        conn.execute(
            "UPDATE library_video_scans
                SET scanned_at = datetime('now', '-91 days') WHERE artist_id = 1",
            [],
        )
        .unwrap();

        assert_eq!(artists_needing_scan(&conn).unwrap().len(), 1);
    }

    #[test]
    fn an_artist_without_a_tidal_id_is_skipped() {
        let conn = setup();
        conn.execute("UPDATE artists SET tidal_id = NULL WHERE id = 1", [])
            .unwrap();

        assert!(artists_needing_scan(&conn).unwrap().is_empty());
    }

    #[test]
    fn rescanning_does_not_resurrect_a_hidden_card() {
        let conn = setup();
        let matches = match_videos(
            &liked_tracks_for_artist(&conn, 1).unwrap(),
            &[video(900, "Song")],
        );
        store_artist_scan(&conn, 1, &matches).unwrap();

        assert!(suppress(&conn, 10, 900).unwrap());
        assert!(load_wall(&conn).unwrap().is_empty());

        // The 90-day re-check writes the same hit again.
        store_artist_scan(&conn, 1, &matches).unwrap();

        assert!(
            load_wall(&conn).unwrap().is_empty(),
            "a correction outlives the rescan that would undo it"
        );
    }

    #[test]
    fn the_wall_carries_what_a_card_needs_to_draw() {
        let conn = setup();
        conn.execute_batch(
            "INSERT INTO albums (id, title, year) VALUES (7, 'The Album', 1999);
             UPDATE tracks SET album_id = 7 WHERE id = 10;",
        )
        .unwrap();
        let matches = match_videos(
            &liked_tracks_for_artist(&conn, 1).unwrap(),
            &[video(900, "Song (Live)")],
        );
        store_artist_scan(&conn, 1, &matches).unwrap();

        let wall = load_wall(&conn).unwrap();
        assert_eq!(wall.len(), 1);
        let card = &wall[0];
        assert_eq!(card.video_title, "Song (Live)");
        assert_eq!(card.track_title, "Song");
        assert_eq!(card.artist_name.as_deref(), Some("Anchor"));
        assert_eq!(card.album_year, Some(1999));
        assert_eq!(card.duration_ms, Some(210_000), "seconds become ms here");
        assert_eq!(
            card.artwork_url.as_deref(),
            Some("https://resources.tidal.com/images/img/900/640x640.jpg"),
            "the backend always emits 640; the grid downsizes"
        );
    }

    #[test]
    fn suppressing_something_absent_reports_it() {
        let conn = setup();
        assert!(!suppress(&conn, 10, 12345).unwrap());
    }

    #[test]
    fn progress_counts_only_artists_with_likes() {
        let conn = setup();
        conn.execute(
            "INSERT INTO artists (id, name, tidal_id) VALUES (2, 'No Likes', 5002)",
            [],
        )
        .unwrap();

        let before = scan_progress(&conn).unwrap();
        assert_eq!(before.total_artists, 1);
        assert_eq!(before.scanned_artists, 0);

        store_artist_scan(&conn, 1, &[]).unwrap();
        let after = scan_progress(&conn).unwrap();
        assert_eq!(after.scanned_artists, 1);
        assert_eq!(after.total_artists, 1);
    }
}
