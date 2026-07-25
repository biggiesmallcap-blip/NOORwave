# Liked videos: a library surface

A library surface for the videos of songs you already liked. Distinct from the
editorial `/videos` discovery work (that fans out over *artists'* videos to
surface things you don't have; this is the opposite - a wall of videos for likes
you already own). They share only the video queue and the persistent dock. Ships
independently.

## What it is

The mirror of the /library **Albums** grid, but for videos. Every liked song that
has an official video becomes cards on a wall. Live takes, covers and alternates
all count and each gets its own card - a richer wall beats a deduped one. Most
liked songs have no video, so this is a real but partial subset of the library,
and that's the honest framing: "the videos among your likes", not a mirror of
every like.

Measured on the dev library (2026-07-25): 4,276 liked tracks across 2,350
distinct artists, 2,346 of which (99.8%) already carry `artists.tidal_id`.

## Entry and layout

- **Entry:** its own route, `/videos/liked`, reached from a header-action link on
  /videos next to the existing `TIDAL editorial` link
  (`frontend/src/routes/videos/+page.svelte:765`). Not a mode inside /videos:
  that page is a 1,375-line state machine (browse mode, editorial layer, session
  snapshot, restore) and a third mode there buys regressions, not reuse.
- **View:** same card grid + filter-pill language as the Albums library view.
  Type in the search field to refine; tick filter pills for genre and year.
- **Default sort:** most recently liked first (`tracks.date_added DESC`), with
  A-Z by video title as the secondary sort option. A wall you revisit wants the
  new stuff at the top.
- **Filter pills:**
  - **Genre** from `track_primary_genre` on the liked track, joined to `genres`
    for the label (the view already ranks musicbrainz > spotify > lastfm).
    Coverage on the dev library: 3,900 / 4,276 liked tracks (91%).
  - **Year** from the liked song's **album year** (`albums.year`), NOT the
    video's own release date. The parser drops video `releaseDate` today and we
    are deliberately not adding it for v1 - album year is close enough for a
    filter pill and needs no new parsing or live-payload check. Two known
    imperfections: a re-release can show its album's year rather than the
    original video's, and coverage is only 2,651 / 4,276 (62%). A card with no
    album year is simply excluded while a year pill is ticked - no "Unknown"
    pill.
- **Actions** (all through the existing video queue + autoplay-next, zero new
  playback code): Play all, Shuffle, Play genre (the ticked-genre subset). Each
  builds a `VideoSessionItem[]` and calls `playVideo(item, { queue, sourceLabel })`
  from `frontend/src/lib/stores/video_session.ts`.

## Indexing: per-artist fan-out, background and self-healing

The resolve runs under the global 4-inflight TIDAL semaphore
(`services/tidal/client.rs:15`), so it must never block a request. Model it on
the `auto_enrich` self-healing pass.

**One call per liked *artist*, not per liked track.** `get_artist_videos`
(`services/tidal/client.rs:513`, already the workhorse behind `video_sets`)
returns an artist's whole video catalog in one call. That single call resolves
every liked song by that artist at once and gives a definitive "no video" for
artists with none. Versus one video *search* per liked track this is 2,350 calls
instead of 4,276, exact artist matching by `tidal_id` instead of fuzzy name
matching, and a proven code path.

Artists without a `tidal_id` (4 on the dev library) are skipped in v1. No
per-track `search_videos` fallback: it would reintroduce exactly the per-track
fan-out this design exists to avoid.

- **Sync finishes -> enqueue, don't resolve.** Reuse the `auto_enrich` shape
  exactly: a `LibrarySynced` event listener plus a daily catch-up interval, both
  calling a `run_if_idle` gated by a new `library_video_scan_running` atomic on
  `SharedState`. No "unindexed" marker column is needed - the work query below is
  the queue.
- **Work query.** An artist needs scanning when it has liked tracks and any of:
  no scan row, a scan older than 90 days, or liked tracks added after
  `scanned_at`. That last clause is load-bearing: without it a new like by an
  already-scanned artist would never resolve.
- **Scan state is per artist** (`library_video_scans`, ~2,350 rows), which
  replaces the per-track negative cache entirely. "This liked track has no video"
  is implied by "its artist was scanned and nothing matched".
- **Logged out / no TIDAL tokens:** the pass returns 0 and does nothing, the way
  `build_missing_sets` does. The view degrades to an empty wall with a "connect
  TIDAL" empty state.
- **Manual "Refresh"** affordance on the view for the impatient, but the default
  path is fully automatic. The view also reports scan progress (artists scanned /
  artists with liked tracks) so a first run on an existing library reads as
  filling in, not as broken.

## Matching

Artist identity is exact (the videos came from that artist's TIDAL id). Title
matching is loose on purpose: normalize both sides, strip alt-version and
featured-artist segments to a base title, and accept on base-title equality or
`strsim` Jaro-Winkler at or above 0.90. The normalizer and alt-version token
lists already exist at `noor-server/src/library/duplicates.rs:90` and above;
promote what is needed to `pub(crate)` rather than writing a second one.
`strsim` is already a dependency.

Accepting live/cover/alternate takes is the point, so a "Song (Live at X)" video
matching liked "Song" is a hit, not noise. Every hit becomes its own row and its
own card.

## Schema (migration 057)

Migration ids are positional (`id` = index in the `MIGRATIONS` slice), so this
must stay the tail of the array. 056 is `video_history` from the editorial
video work; if a rebase ever lands another migration after this one, renumber
rather than reorder.

```sql
CREATE TABLE IF NOT EXISTS library_video_scans (
    artist_id   INTEGER PRIMARY KEY REFERENCES artists(id) ON DELETE CASCADE,
    scanned_at  TEXT NOT NULL,
    video_count INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS library_videos (
    track_id         INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
    tidal_video_id   INTEGER NOT NULL,
    video_title      TEXT    NOT NULL,
    duration_seconds INTEGER,
    image_id         TEXT,
    match_score      REAL    NOT NULL,
    suppressed       INTEGER NOT NULL DEFAULT 0,
    created_at       TEXT    NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (track_id, tidal_video_id)
);
CREATE INDEX IF NOT EXISTS idx_library_videos_suppressed
    ON library_videos(suppressed, track_id);
```

`video_title`, `duration_seconds` and `image_id` are persisted rather than
re-fetched: without them the grid needs a live TIDAL call per card just to draw
a thumbnail. No `matched` column - `library_videos` only ever holds hits, and
"searched, no video" lives in `library_video_scans`.

Artwork follows the TIDAL sizing rules in AGENTS.md (valid sizes only, `onerror`
wired on every `img`).

The grid reads `library_videos WHERE suppressed = 0`, joins `tracks` for
title/artist and `date_added`, `albums.year` for the year pill, and
`track_primary_genre` -> `genres` for the genre pill.

## Feedback: wrong-match correction (not taste feedback)

This surface has nothing to re-rank, so discovery-style like / not-interested has
no place here. The feedback that *does* fit is match correction: loose matching
will occasionally attach the wrong video to a song, so each card gets a
**"wrong match / hide this"** action that sets `suppressed = 1`. The re-scan
upsert must never reset `suppressed`, or the 90-day re-check would resurrect
every hidden card. Cheap, and the honest counterpart to keeping matching loose.

## Endpoints

- `GET  /api/videos/liked` - grid rows plus `{ scanned_artists, total_artists,
  running }` progress.
- `POST /api/videos/liked/refresh` - kick `run_if_idle`, returns whether a pass
  started.
- `POST /api/videos/liked/hide` `{ track_id, tidal_video_id }` - set
  `suppressed = 1`.

## Tests

- Rust: migration test (`apply_migrations_up_to`), work-query selection
  (unscanned / stale / new-like-after-scan / suppressed-survives-rescan), and
  title-match accept/reject cases.
- Frontend: `frontend/scripts/liked-videos-contract.test.mjs` covering the
  request shape, filter-pill wiring, and that Play all / Shuffle / Play genre go
  through `playVideo` with a queue.

## Not in v1

Video-own release-year parsing (using album year instead); watch history for
these (shares nothing with the editorial `video_history`); a `search_videos`
fallback for artists without a `tidal_id`; per-song caps on how many videos one
liked track may contribute.
