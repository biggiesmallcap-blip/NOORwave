# Tidal Genre Source — Diagnostic Investigation

**Date:** 2026-04-30
**Scope:** Root-cause why `track_genres` has zero rows with `source='tidal'` despite the writer code path existing and being called per-track on every Tidal sync.
**Status:** Diagnostic only — no fixes proposed.

---

## TL;DR

**Possibility A is confirmed.** The Tidal v1 API endpoints noor-server uses (`/albums/{id}/tracks`, `/users/{id}/favorites/tracks`, `/playlists/{uuid}/tracks`) return **zero genre data** in their JSON. The track and album objects have 35 and 31 top-level keys respectively, none of them named `genre`, `subGenre`, `genres`, `subGenres`, `subgenres`, `tags`, or `mood`. The `#[serde(flatten)] extra: HashMap` on `TidalTrack` captures technical/structural fields like `bpm`, `popularity`, `mediaMetadata`, `audioModes`, `replayGain` etc. — there is no genre information to capture.

This was verified independently against three different endpoint shapes for three different sample tracks (pop / deep house / older catalog). All nine API responses agree: no genre keys exist.

The write path is wired correctly. `infer_tidal_track_genres` runs, finds nothing, and calls `replace_track_source_genres(track_id, &[], "tidal", 0.82)` which executes the `DELETE` half (no-op since no prior rows) and skips the `INSERT` half because `canonical_names.is_empty()` returns at [`db/queries.rs:1052`](../noor-server/src/db/queries.rs#L1052). Net effect: zero rows, every sync, forever.

**Possibility B (resolution drops it) is ruled out** — there is nothing to resolve. **Possibility C (writer not called) is ruled out** — the writer fires but receives an empty vec.

This is structural, not a bug in the writer. Tidal v1 simply does not surface per-track or per-album genre on these endpoints. Fixing it requires either (a) using a different Tidal API surface that *does* expose genre (e.g. the undocumented `/v2/pages/...` page-builder endpoints, or `/v1/genres/{id}/tracks` reverse-lookup), or (b) accepting Tidal as not a genre source and leaning on last.fm + musicbrainz for that signal.

---

## How the investigation was run

A standalone probe binary was used (out-of-workspace cargo crate). It loaded the persisted Tidal access token from `noor.db` (`service_auth WHERE service='tidal'`), hit three endpoint shapes per sample track — **A.** `/albums/{album_id}/tracks` (the production path, called from [`server/routes.rs:6487, 6544, 6657`](../noor-server/src/server/routes.rs)), **B.** `/tracks/{track_id}` (unused by client), **C.** `/albums/{album_id}` (unused by client) — printed top-level keys plus the keys surviving the `#[serde(flatten)] extra` filter, ran inlined copies of `extract_genre_candidates_from_extra` and `collect_genre_values` against the raw JSON, and emulated `GenreCatalog::resolve` against the live `genres` table + `aliases.json`.

The probe was deleted post-investigation; if it needs to be re-run, recreate from this description.

Run on 2026-04-30 against `noor.db` countryCode=AU, three samples:

| Label | Artist / Track | track_id | album_id |
| --- | --- | --- | --- |
| Pop | Doja Cat — "Go To Town" (Amala) | 86429365 | 86429364 |
| Deep house | Daft Punk — "Veridis Quo" (Discovery) | 1550556 | 1550545 |
| Older catalog | Frank Sinatra — "You and I" (Battle of the Bands) | 493306 | 493303 |

All nine HTTP calls returned 200 OK.

---

## Sample 1 — Doja Cat / "Go To Town" (Pop, 2018)

### A. `/albums/86429364/tracks` (production path)

35 top-level keys on the track item:

```
accessType, adSupportedStreamReady, album, allowStreaming, artist, artists,
audioModes, audioQuality, bpm, copyright, djReady, duration, editable,
explicit, id, isrc, key, keyScale, mediaMetadata, mixes, payToStream, peak,
popularity, premiumStreamingOnly, replayGain, spotlighted, stemReady,
streamReady, streamStartDate, title, trackNumber, upload, url, version,
volumeNumber
```

After stripping the structured fields the `TidalTrack` struct claims (`id`, `title`, `duration`, `trackNumber`, `volumeNumber`, `isrc`, `artist`, `artists`, `album`, `audioQuality`, `streamReady`), 24 keys land in `track.extra`:

```
accessType, adSupportedStreamReady, allowStreaming, audioModes, bpm, copyright,
djReady, editable, explicit, key, keyScale, mediaMetadata, mixes, payToStream,
peak, popularity, premiumStreamingOnly, replayGain, spotlighted, stemReady,
streamStartDate, upload, url, version
```

`extract_genre_candidates_from_extra(track.extra) = []`

The `track.album` field on this endpoint is a slim `TidalAlbumRef`. Its `extra` contains 2 keys: `vibrantColor`, `videoCover`. No genre.

`extract_genre_candidates_from_extra(track.album.extra) = []`

**Production effect: `infer_tidal_track_genres` returns `[]` → `replace_track_source_genres(track_id=1634, &[], "tidal", 0.82)` → DELETE fires (no-op), INSERT does not.**

### B. `/tracks/86429365` (single-track endpoint, unused)

35 top-level keys — same set as A. No `genre*` keys. Genre extraction returns `[]`.

### C. `/albums/86429364` (album endpoint, unused)

31 top-level keys:

```
adSupportedStreamReady, allowStreaming, artist, artists, audioModes,
audioQuality, copyright, cover, djReady, duration, explicit, id,
mediaMetadata, numberOfTracks, numberOfVideos, numberOfVolumes, payToStream,
popularity, premiumStreamingOnly, releaseDate, stemReady, streamReady,
streamStartDate, title, type, upc, upload, url, version, vibrantColor,
videoCover
```

No `genre*` keys. Genre extraction returns `[]`.

---

## Sample 2 — Daft Punk / "Veridis Quo" (Deep House / Electronic, 2001)

Identical surface to Sample 1. Same 35 keys on track endpoints, same 31 keys on album endpoint. Highest-priority candidate for "should obviously have a genre tag" — Daft Punk on a 2001 Electronic album — and Tidal exposes none of it on the v1 surface.

```
extract_genre_candidates_from_extra → []   (production path)
extract_genre_candidates_from_extra → []   (/tracks/{id})
extract_genre_candidates_from_extra → []   (/albums/{id})
```

`audioModes=["STEREO"]`, `mediaMetadata.tags=["LOSSLESS"]`, `popularity=76`, `bpm` present (numeric) — so Tidal *does* expose technical/audio metadata, just not editorial genre on this surface.

---

## Sample 3 — Frank Sinatra / "You and I" (Older catalog, 1998 reissue)

Identical surface again. 35 keys / 31 keys, no genre fields. `popularity=9`, `releaseDate="1998-06-16"`, `type="ALBUM"`. Lower API popularity but the schema is the same — nothing genre-shaped is returned.

```
extract_genre_candidates_from_extra → []   (production path)
extract_genre_candidates_from_extra → []   (/tracks/{id})
extract_genre_candidates_from_extra → []   (/albums/{id})
```

---

## Why the data ends up at zero rows

Three layers were instrumented; the failure is at the API layer:

```
   Tidal v1 API
        │   /albums/{id}/tracks  →  35 keys, 0 are genre-shaped
        ▼
   serde flatten into TidalTrack.extra: HashMap<String, Value>
        │   24 keys captured, none genre-shaped
        ▼
   infer_tidal_track_genres(track)            [routes.rs:7920]
        │   extract_genre_candidates_from_extra(track.extra)        → []
        │   extract_genre_candidates_from_extra(track.album.extra)  → []
        │   collect_clear_genres([])                                → Vec::new()
        ▼
   replace_track_source_genres(..., &[], "tidal", 0.82)      [queries.rs:1040]
        │   DELETE WHERE track_id=X AND source='tidal'   ← always fires (no-op for new rows)
        │   if canonical_names.is_empty() { return Ok(0); }   ← always returns here
        ▼
   No INSERT into track_genres.
```

The writer is correct. The pipeline upstream of it has nothing to write.

---

## Recommendation: cheap fix or structural rework?

**There is no cheap fix that uses the current API surface.** The production endpoints simply do not return genre data. The available paths to actually obtain Tidal-side genre are:

1. **Tidal `/v2/pages/...` page-builder endpoints** (e.g. `/v2/pages/album/{id}`, `/v2/pages/artist/{id}`). Used by Tidal's own desktop/mobile clients to render album pages. These return module-structured JSON that includes a `GENRE_HEADER` / `GENRES` module on most albums. **Undocumented**, may include rate-limit and auth-scope wrinkles, and the schema can change without notice. Some open-source Tidal wrappers (e.g. `python-tidal`) call these endpoints with a `deviceType=BROWSER` parameter. Would require a new client method, response-shape parsing, and a fallback for albums where the module isn't present.

2. **Tidal `/v1/genres/{id}/tracks` reverse-lookup**. Doesn't tell us "what genres does track X have"; tells us "give me tracks in genre Y". Can be combined with a per-genre crawl + intersection to produce a mapping. Bandwidth-heavy, slow, indirect, and only works for pre-defined Tidal genres. Not a real solution for individual track tagging.

3. **Tidal app/web UI scraping** — not viable.

4. **Accept the status quo.** Tidal is not a genre source for this product. The data is good enough on last.fm + musicbrainz for the current scope (86.6% library coverage). The `genres.tidal_genre_id` column is dead, and the `'tidal'` source value can be retired or repurposed. The writer code path can be left in place for free in case Tidal adds genre fields to a future API; or it can be removed to avoid running a no-op DELETE on every sync.

**Cost ranking, low → high:**

- **Lowest cost: do nothing**, document that Tidal is not a genre source. The DELETE call per track per sync is one round-trip on a row that doesn't exist; harmless. No code change needed. Update the schema comment to reflect that `source='tidal'` and `tidal_genre_id` are reserved-but-empty.
- **Medium cost: switch to the v2 pages endpoint.** Adds one extra API call per album during sync, plus parsing logic for the page module shape. Yields probably 70–90% of albums tagged at confidence higher than last.fm (call it 0.75). Brittle to Tidal API changes.
- **High cost: scrape genres reverse-lookup.** Build a "tracks-by-genre" cache from the Tidal genre tree. Slow first sync, useful afterwards. Doesn't work for sub-genres outside Tidal's published genre tree.

The decision hinges on whether you want Tidal to be a high-confidence genre source in addition to last.fm and musicbrainz, or whether you accept those two as the complete answer. From the prior data-quality investigation, the two-source mix is already weighted more by last.fm (51,412 rows at conf 0.40) than by musicbrainz (26,153 rows at conf 0.85). Adding a Tidal-page-derived source at conf ~0.75 on every album would meaningfully change the weighting — but it would require the v2 endpoints and the brittleness that comes with them.

This is a planning conversation, not a fix-now decision.

---

## Appendix — `lastfm_unresolved_tags` writer verification

Side check: the genre data-quality investigation flagged that `lastfm_unresolved_tags` has 0 rows. Reading the writer at [`services/lastfm/enrichment.rs:35-62`](../noor-server/src/services/lastfm/enrichment.rs#L35) confirms the table is **structurally write-dead in normal operation**, not "broken" in the bug sense.

The path is:

```rust
fn resolve_to_genre_id(tag: &str, conn: &Connection) -> Option<i64> {
    if !should_keep_tag(tag, conn) { return None; }                     // (1)
    let resolution = embedded_builder().resolve(tag);
    let canonical = resolution.canonical_name()?;                       // (2)
    match conn.query_row("SELECT id FROM genres WHERE name = ?1", ...) {
        Ok(id) => Some(id),
        Err(_) => {                                                     // (3)
            // Canonical name found but not in DB — taxonomy has a gap.
            conn.execute("INSERT INTO lastfm_unresolved_tags ...");
            None
        }
    }
}
```

For a row to land in `lastfm_unresolved_tags`, all three conditions must hold: (1) tag passes the filter, (2) the in-memory `GenreCatalog` returns a canonical name for it, AND (3) that canonical name is **not** present in the `genres` table.

But the in-memory catalog and the DB rows are seeded from the **same** `taxonomy.json`:

- `GenreCatalog::from_embedded()` (in `genre/mappings.rs`) reads the file via `include_str!("../../../genre-taxonomy/taxonomy.json")`.
- `ensure_taxonomy_loaded()` (in `genre/taxonomy.rs:14-29`) reads the same embedded JSON at server start and `INSERT OR IGNORE`s every node into the `genres` table.

So: if the catalog can resolve a tag to a canonical name X, then X exists in `taxonomy.json`, which means the seed code inserted it into `genres`, which means the SELECT at line 43 succeeds, which means we never reach the `Err(_)` branch that writes to `lastfm_unresolved_tags`.

The branch is reachable only by failure modes that don't occur in steady state: a partial taxonomy.json change between seed and a later run with the binary rebuilt (catalog has X, DB doesn't), or an INSERT OR IGNORE conflict during seed (e.g. slug collision). Neither occurs here.

The actually-interesting bucket — "tags that the canonical resolver could not match at all" — is dropped silently at line 41 (the `?` on `canonical_name()`) without being logged anywhere. That is where the real "tags we don't know about" data goes: into `/dev/null`. So the table is empty not because resolution is perfect, but because the table only logs taxonomy DB-vs-catalog drift, not actual resolution failures.

**Verdict on the side check:** writer is not broken; it is correctly implemented but its trigger condition is unreachable in practice. The user-facing claim "0 rows means resolution catches everything" is technically false — it means "DB seed agrees with the catalog", which is trivially true. If the goal is to surface unresolved tags for taxonomy-curation purposes, a different log point would be needed (at line 41, before the `?`). Not in scope to fix here.
