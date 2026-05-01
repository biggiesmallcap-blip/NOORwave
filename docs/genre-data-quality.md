# Genre Data Quality Investigation

**Date:** 2026-04-30
**Scope:** Library tracks only. Non-library / streaming-only seed-resolution is being addressed separately.
**Status:** Investigation only — no fixes proposed. Use this report to inform a follow-up planning conversation.

---

## Summary

Genre data quality is **mediocre, with structural issues that materially degrade Phase 2b coherence scoring** — but not so broken that the weighted-Jaccard math is meaningless. Three findings dominate:

1. **Coverage is uneven.** 86.6% of tracks have at least one genre, but the gap is concentrated on stream-only / non-favorite tracks (e.g. Amy Shark's only library track has zero genres because enrichment runs only on favorites). When a stream-only track is the seed, every candidate scores Jaccard=0 against it.
2. **Last.fm crowd-tagging pollutes the data.** Bob Marley & The Wailers has "Classic Rock" tagged on 73 tracks (last.fm conf=0.40); The Weeknd has "Jazz" on 10 tracks (musicbrainz conf=0.85). The tag filter is conservative — it blocks "classic" alone but accepts "classic rock" because that string matches a real taxonomy node, so any artist popularly tagged "classic rock" inherits Rock-family lineage.
3. **The taxonomy is flat and Tidal-disconnected.** Max depth is 2 (3 levels including root); 38% of all assignments land at the top level (Rock / Hip-Hop / Pop). Despite `source='tidal'` being the schema default, **zero rows in `track_genres` actually have source='tidal'** — the Tidal genre extraction path exists in code but has produced no live data. All 77,565 assignments come from last.fm (66%) or musicbrainz (34%).

The Phase 2b scoring math is sound for tracks that have well-tagged seeds. The issue is upstream: the data feeding it is shaped like a small flat enrichment instead of the curated three-level taxonomy the math was designed for.

---

## Ingestion pipeline

```
External Data Sources
├─ Tidal (streaming metadata)
│  └─ HTTP API → TidalTrack.extra HashMap (keys: genre, subGenre, etc)
│     └─ infer_tidal_track_genres()  [routes.rs:7901]
│        └─ collect_clear_genres()   [genre/builder.rs:69]
│           └─ resolve via embedded GenreCatalog (closed taxonomy)
│              └─ replace_track_source_genres(source='tidal', conf=0.82)
│                 ├─ DELETE WHERE track_id=X AND source='tidal'
│                 └─ INSERT OR REPLACE
│
├─ Last.fm (crowd-voted tags, ONLY for favorites/album tracks)
│  └─ Top-N tags per track + artist, MAX_TAGS_PER_TRACK=5
│     ├─ tag_filter::should_keep_tag()  [tag_filter.rs:117]
│     │   stop list (52) + locale list (38) + decade regex + 40-char cap + artist-name match
│     ├─ resolve_to_genre_id()   [enrichment.rs:35]
│     │   normalize → exact → alias → fuzzy(Jaro-Winkler ≥0.92)
│     ├─ should_insert() hierarchy filter (drops parents of existing children)
│     └─ INSERT ... ON CONFLICT DO UPDATE SET confidence=MAX(...)
│         source='lastfm', conf=0.40
│
├─ Spotify (album + artist genres, enrichment runs only on favorites)
│  └─ canonicalize_or_passthrough() — title-cases unrecognised strings
│     ├─ INSERT OR IGNORE INTO genres (...parent_id=NULL) ← can grow taxonomy
│     └─ INSERT OR IGNORE track_genres (source='spotify', conf=1.0)
│         [Currently spotify_checked has 0 rows — never run on this DB]
│
├─ MusicBrainz (ISRC lookup)
│  └─ write_genres()  [musicbrainz.rs:106]
│     └─ closed-taxonomy resolution + INSERT OR IGNORE
│         source='musicbrainz', conf=0.85 (ISRC) or 0.55 (inferred)
│
├─ ACRCloud (audio fingerprint) — NEVER writes to track_genres
├─ Local file scanner (library/scanner.rs) — STUB, not implemented
└─ Manual UI assign — assign_genre_to_tracks(), source='manual', conf=1.0
```

Key write points:

| Source | Confidence | Mechanism | Code |
| --- | --- | --- | --- |
| `tidal` | 0.82 | DELETE+INSERT | `routes.rs:7864` → `db/queries.rs:1040` |
| `lastfm` | 0.40 | UPSERT on confidence MAX | `services/lastfm/enrichment.rs:318` |
| `spotify` | 1.00 | INSERT OR IGNORE (can grow taxonomy) | `services/spotify/enrichment.rs:322` |
| `musicbrainz` | 0.85 / 0.55 | INSERT OR IGNORE | `services/musicbrainz.rs:130` |
| `manual` | 1.00 | INSERT OR REPLACE | `db/queries.rs:1031` |

Taxonomy is loaded once at server start from `genre-taxonomy/taxonomy.json` via `ensure_taxonomy_loaded()` in `genre/taxonomy.rs:14`. Aliases live in memory only (`genre-taxonomy/aliases.json`, 108 entries) and are not persisted in the DB.

---

## Coverage stats

### Totals

| Metric | Value |
| --- | --- |
| Total tracks | 35,419 |
| Tracks with ≥1 genre | 30,675 (86.61%) |
| Total genre assignments | 77,565 |
| Genres in taxonomy table | 285 |
| Genres actually used in `track_genres` | 241 (44 unused) |
| `lastfm_unresolved_tags` rows | **0** ← suspicious, see issues |
| Genres with `tidal_genre_id` set | **0** (column unused) |

### Genres-per-track distribution

| Genres on track | # tracks | % of tagged tracks |
| --- | --- | --- |
| 1 | 7,485 | 24.4% |
| 2 | 9,030 | 29.4% |
| 3 | 7,532 | 24.6% |
| 4 | 4,429 | 14.4% |
| 5 | 1,613 | 5.3% |
| 6 | 469 | 1.5% |
| 7 | 108 | 0.4% |
| 8 | 8 | 0.03% |
| 9 | 1 | <0.01% |

Median = 2, mean = 2.53, max = 9.

### Source breakdown

| Source | Assignments | Distinct tracks | Avg confidence |
| --- | --- | --- | --- |
| `lastfm` | 51,412 (66%) | 26,774 | 0.41 |
| `musicbrainz` | 26,153 (34%) | 9,782 | 0.85 |
| `tidal` | **0** | 0 | — |
| `spotify` | **0** | 0 | — |
| `manual` | **0** | 0 | — |

### Top 10 genres (≈51% of all assignments)

| Genre | Uses | % of all assignments |
| --- | --- | --- |
| Rock | 7,708 | 9.94% |
| Hip-Hop | 7,289 | 9.40% |
| Pop | 6,312 | 8.14% |
| Classic Rock | 3,579 | 4.61% |
| Rhythm and Blues | 2,849 | 3.67% |
| Folk | 2,623 | 3.38% |
| Jazz | 2,531 | 3.26% |
| Funk | 2,246 | 2.90% |
| Blues | 2,083 | 2.69% |
| Electronic | 1,997 | 2.57% |

Top 10 cover **50.56%** of all assignments. Long tail is moderately healthy (50% spread across 231 other genres).

### Tracks with no genres, by track origin

| Track source | Tracks with no genre |
| --- | --- |
| `tidal` (favorites/albums) | 4,733 |
| `tidal_stream` (stream-only) | 11 |

Of the 35,419 tracks, **31,994 are in `lastfm_checked`** and **35,333 in `musicbrainz_checked`**, but only 26,774 actually got last.fm genres and 9,782 got musicbrainz genres — so **~73% of musicbrainz lookups returned nothing usable** (no ISRC match or no MB tags). `spotify_checked` is empty: Spotify enrichment has never run.

### Assignments by depth of assigned genre

| Taxonomy level | Genre count | Assignments | % of total |
| --- | --- | --- | --- |
| 0 (top: Rock / Pop / Hip-Hop / …) | 14 | 29,889 | 38.5% |
| 1 (Classic Rock, Deep House, …) | 187 | 43,145 | 55.6% |
| 2 (Tech House, Modal Jazz, …) | 84 | 4,531 | 5.8% |

**6,115 tracks (20% of tagged tracks) only have top-level genres** — when these are in a coherence comparison, the only signal is "share Rock" or "share Pop", which is very coarse.

---

## Quality spot-check (10 sample tracks)

| Artist | Track | Genres assigned | Verdict |
| --- | --- | --- | --- |
| Doja Cat | Go To Town | Pop, Hip-Hop, Rhythm and Blues (lastfm) | ✅ reasonable |
| Ariana Grande | My Everything | Electronic, Pop (mb), Rhythm and Blues, Americana (lastfm) | ⚠️ "Americana" is wrong |
| RÜFÜS DU SOL | (no track matched "Solace") | — | (artist not in sample) |
| ZHU | Automatic | House, Deep House, Trance (lastfm) | ✅ reasonable |
| Amy Shark | I Said Hi | **(none)** | ❌ stream-only, unenriched |
| Tame Impala | Let It Happen | Psychedelic Rock, Synthpop (mb) | ✅ reasonable |
| Miles Davis | Drad Dog | Jazz (mb), Bebop, Blues (lastfm) | ✅ reasonable |
| Frank Sinatra | You and I | Jazz, Swing, Vocal Jazz (lastfm) | ✅ reasonable |
| Mac Miller | Best Day Ever | Hip-Hop only | ⚠️ thin — Mac Miller has jazz-rap, lo-fi, soul tracks |
| Lunar C | Pocket Full Of Fuckall | Grime, Hip-Hop (lastfm) | ✅ but see "uniformity" issue below |
| The Weeknd | Real Life | Electronic, Dubstep, Pop, Contemporary R&B, R&B, **Jazz** (mb 0.85) | ❌ Jazz is wrong |
| Bob Marley | (any of 73 tracks) | Reggae, Roots Reggae (correct) + **Classic Rock** (lastfm 0.40) | ❌ Classic Rock is wrong |
| The Teskey Brothers | Crying Shame | Country, Blues (lastfm) | ⚠️ should also have Soul / Roots Rock |
| Jack Johnson | Subplots | Rock, Soft Rock (mb) | ✅ reasonable |

### Within-artist uniformity

Distinct genre signatures across an artist's catalogue:

| Artist | Distinct sigs | Tracks |
| --- | --- | --- |
| Mac Miller | 21 | 191 |
| The Weeknd | 20 | 56 |
| Ariana Grande | 15 | 32 |
| Jack Johnson | 14 | 67 |
| Tame Impala | 12 | 19 |
| Frank Sinatra | 9 | 175 |
| Doja Cat | 5 | 35 |
| **Lunar C** | **1** | **88** |

Lunar C illustrates the artist-level-tagging artefact: every track inherits the artist's last.fm genre soup, with zero per-track variation. For coherence scoring this means within-Lunar-C transitions are perfectly coherent (good) but the system can't distinguish his harder grime tracks from his more melodic ones (limit of the data).

---

## Taxonomy coverage analysis

`genre-taxonomy/taxonomy.json`:

| Level | Count | Examples |
| --- | --- | --- |
| 0 (root families) | 14 | Electronic, Rock, Pop, Hip-Hop, Jazz, Blues, Classical, Folk and Country, R&B and Soul, Reggae and Caribbean, World, Latin, Soundtrack and Screen, Ambient and Experimental |
| 1 (sub-genres) | 187 | Classic Rock, Indie Rock, Deep House, Trip-Hop |
| 2 (leaf specialisations) | 84 | Tech House, Modal Jazz, Anarcho Punk, Future Garage |
| **Total** | **285** | |

**Max depth = 2.** Ancestor expansion (Phase 2b weighted Jaccard) at most adds 1–2 ancestors per assigned genre. A depth-2 leaf gets {leaf, parent, grandparent}; a depth-1 gets {self, parent}; a depth-0 gets {self}. The bonus from the ancestor weight (0.7 in `genre/jaccard.rs:35`) is therefore quite limited in absolute terms.

**44 unused taxonomy genres** include 6 of the 14 root families (`Ambient and Experimental`, `Folk and Country`, `R&B and Soul`, `Reggae and Caribbean`, `Soundtrack and Screen`) — these were intended as parent buckets but the leaf usage routes around them. This means the root-family rollups some downstream code might assume don't actually populate.

**0 orphan genres** in `track_genres` (every assigned genre exists in the taxonomy table). That's because the only writers that *could* create new genres on the fly — Spotify and manual — have produced 0 rows. If Spotify enrichment ever runs, expect orphaned `parent_id=NULL` genre rows to appear.

**0 genres with `tidal_genre_id` set.** The column exists in the schema but no code populates it. The Tidal write-path keys by canonical name, not by Tidal genre ID, so the column is effectively dead.

`genre-taxonomy/aliases.json`: 108 entries. Coverage examples: drum-and-bass has 4 variants, house has 9, hip-hop has 3. Notable gap: only `"rnb"` aliases to "Rhythm and Blues" — `"r&b"` and `"r-and-b"` are not aliased, which means raw Spotify/last.fm strings using punctuation may silently fail to resolve unless caught by the fuzzy-match step (Jaro-Winkler ≥0.92).

---

## Last.fm tag interaction findings

- Last.fm enrichment **only runs for favorites and tracks in favorited albums** (`lastfm/enrichment.rs:169`). Stream-only / scrobbled-but-not-favourited tracks get zero last.fm genres. This is the proximate cause of Amy Shark's empty record.
- The filter (`services/lastfm/tag_filter.rs:117`) blocks 52 stop tags ("seen live", "favourite", "amazing", "classic", "epic", …) and 38 locale tags ("american", "british", "tokyo", …), reject decade markers via regex, drop tags >40 chars, and drop tags that match a known artist name.
- **Multi-word tags like "classic rock" pass the filter** because the stop list is exact-match. This is why Bob Marley has Classic Rock on 73 tracks: last.fm crowd tagged him "classic rock" (in the loose "old and great" sense), the filter accepts it, and the taxonomy resolves it to the literal genre `Classic Rock`.
- Per-track tag cap is `MAX_TAGS_PER_TRACK = 5` (`enrichment.rs:21`). After filtering, the top 5 tags by last.fm score are kept.
- `lastfm_unresolved_tags` is a curation log for tags that pass the filter but have no taxonomy match. **It currently contains 0 rows.** Either the resolution path is too lenient (fuzzy + alias catches everything), the table was cleared, or the write path is broken. Worth verifying — this is the only signal we have for "what tags would we add to the taxonomy if we wanted broader coverage".
- Within `track_genres`, **49,697 of 51,412 last.fm rows have confidence=0.40 and 1,715 have 0.70**. The 0.70 rows likely come from the upsert promoting matching musicbrainz/tidal entries (`ON CONFLICT DO UPDATE SET confidence = MAX(...)`); they suggest a small fraction of last.fm tags overlap with higher-confidence sources.

---

## Normalisation / compression findings

The user's intuition that "the DB is doing some kind of compression/sorting" is partly correct but not in the way they suspected:

1. **String normalisation on resolution** (`genre/mappings.rs:210`, `:346-353`):
   - trim → ASCII lowercase → non-alphanumeric → space → collapse whitespace.
   - "Hip-Hop" / "hip hop" / "HIP-HOP" all collapse to `"hip hop"` for lookup; the canonical name stored in `genres.name` is the title-cased taxonomy form.
2. **Compound input splitting** (`mappings.rs:215`): inputs with `,` `;` `/` are split per-segment, each resolved independently. If any segment fails, the resolution flags `unresolved_segments` but the rest still flow through.
3. **Per-track de-duplication** is via the `(track_id, genre_id)` primary key of `track_genres`. Multiple sources writing the same genre upsert into one row (last.fm uses `ON CONFLICT DO UPDATE SET confidence = MAX(...)`).
4. **Hierarchy de-noising** (`enrichment.rs:103-123`): `should_insert()` drops a candidate genre if it's a *parent* of one the track already has. So a track tagged "Deep House" won't get the parent "House" added. This makes the per-track set more specific but also means **the assignment-by-depth distribution understates the real ancestor coverage** — when ancestors are expanded at scoring time, the picture gets thicker.
5. **`canonicalize_many()` is all-or-nothing** (`builder.rs:31-52`): if any input is ambiguous, the whole batch returns `None`. `collect_clear_genres()` (used by Tidal) silently drops ambiguous entries. Different ingestion paths therefore have different failure modes.
6. **Tidal re-sync is destructive**: `replace_track_source_genres()` does `DELETE WHERE source='tidal'` then re-inserts, so a Tidal re-sync wipes any prior Tidal rows for that track but leaves last.fm/mb rows alone.

There is **no aggressive global compression** (no row-cap, no genre merging, no sorting that would change semantics). The artefact "Lunar C: 1 distinct signature across 88 tracks" is *not* the DB compressing data — it's the upstream last.fm tags being identical because they're computed at artist level and copied to every track.

---

## Top 5 issues, ranked by severity

### 1. Stream-only / non-favorite tracks have no genre data at all

**Severity: high** — this is the *direct* cause of the user's bad queue when Amy Shark was the seed.

Last.fm and musicbrainz enrichment both gate on `is_favorite=1` or "track is in a favorited album" (`services/lastfm/enrichment.rs:169`, similar gate on the musicbrainz path). Tidal genre extraction depends on `track.extra` having genre keys, which in this DB has produced zero rows. Spotify enrichment hasn't run.

Result: any track with `source='tidal_stream'` or any non-favorited Tidal track has a real chance of zero genres. There are 4,744 such tracks (13.4% of the library). When one of them is the seed, every coherence comparison returns Jaccard=0 and the radio falls back to whatever non-genre signal exists.

### 2. Last.fm "classic rock" tag pollutes the entire pre-2000 catalogue

**Severity: high** — directly visible in the user's queue (Bob Marley → "Classic Rock" wired together with Mac Miller transitions).

`Bob Marley & The Wailers` has 73 tracks tagged `Classic Rock` from last.fm at confidence 0.40. The tag survives `tag_filter::should_keep_tag()` because the stop list is exact-match ("classic" alone is blocked, "classic rock" is not), and `Classic Rock` is a real taxonomy node. Likely affects many older artists. With confidence 0.40 the entry survives even when musicbrainz adds the correct `Reggae` at 0.85.

Files: `services/lastfm/tag_filter.rs:1-152`, `genre-taxonomy/taxonomy.json` (Classic Rock under Rock).

### 3. The `tidal` source has produced zero rows; Tidal genre IDs are unused

**Severity: medium-high** — the Tidal write-path is the highest-quality source (curated, structured) and it's silently inactive.

`schema.rs` defines `genres.tidal_genre_id` and `track_genres.source DEFAULT 'tidal'`, the agent confirmed `routes.rs:7864` calls `replace_track_source_genres(..., 'tidal', 0.82)` during Tidal sync, and `infer_tidal_track_genres()` exists at `routes.rs:7901`. But the live DB has 0 rows with `source='tidal'` and 0 genres with `tidal_genre_id` set.

Either the Tidal API is not returning genre keys in `track.extra` (worth confirming with a fresh sync log), `collect_clear_genres()` is dropping everything as ambiguous, or this code path is wired up but never reached for the existing library. Without Tidal data, the system is leaning entirely on last.fm (noisy, low-conf) and musicbrainz (sparse — only 28% of tracks got any).

### 4. Taxonomy is shallow (max depth 2) and 38% of assignments are at the root

**Severity: medium** — bounds how much the ancestor-weighted Jaccard math can actually do.

The Phase 2b coherence model assumes meaningful taxonomic depth so that `Tech House` and `Deep House` can share `House` as an ancestor. With only 14 root families and 84 leaf nodes, and 38% of assignments landing directly at the root, two tracks tagged simply `Rock` will share `{Rock}` exactly and score very high regardless of whether one is Bob Marley and one is Tame Impala. Six of the 14 root buckets are unused for direct assignment (`Reggae and Caribbean`, `R&B and Soul`, `Folk and Country`, `Soundtrack and Screen`, `Ambient and Experimental`, `World`), which means rollup queries that assume those parents see assignments will return nothing.

### 5. Within-artist uniformity for some artists collapses per-track signal

**Severity: medium** — visible in the Lunar C row of the queue.

When last.fm artist tags get applied wholesale (Lunar C: 1 signature across 88 tracks), the per-track genre signal is collapsed to "the artist's average style". That makes radio transitions within Lunar C trivially coherent but blocks the system from telling that one Lunar C track sits closer to UK Hip-Hop and another closer to Trap. Most affected when an artist has a wide stylistic range (Mac Miller has 21 sigs / 191 tracks — better; Lunar C has 1 / 88 — worst).

---

## Top 5 questions for the user

1. **Should non-favorite / stream-only tracks be enriched?** Currently last.fm and musicbrainz only run on favorites and album tracks. Is that a deliberate cost-control choice (last.fm rate limits, Spotify quota), or an oversight? If we enriched every played track on first scrobble, we'd close the Amy Shark gap but multiply API calls by ~10×.
2. **Is the Tidal genre source actually broken, or has it just never been re-synced?** Worth running one Tidal sync for a known track and watching whether `track.extra` carries `genre`/`subGenre`. If it does and the DB is still empty, the writer is broken; if it doesn't, the path is a no-op for this account/region. This is the single biggest lever for raising data quality.
3. **How aggressive should last.fm filtering be?** Current filter accepts any multi-word string that exact-matches the taxonomy. We could add minimum-vote thresholds (last.fm tag count), require ≥2 sources agree before persisting, or hard-blocklist known-bad combinations like "(reggae artist) + Classic Rock". What's the appetite for false-negatives vs the noise we have now?
4. **Should the taxonomy go deeper, or should we lean harder on the genres we have?** The flat shape limits ancestor-Jaccard usefulness. Options: (a) deepen to 4–5 levels for the popular families (Rock has dozens of recognised micro-genres; the current taxonomy stops at "Indie Rock"); (b) accept the depth as-is and tune the ancestor weight up/down; (c) add cross-family edges (e.g. "Trip-Hop" should share weight with both Hip-Hop and Electronic). Which direction matches the product vision?
5. **Per-track vs per-artist tagging — is this a problem worth solving?** Last.fm artist-level tags get copied to every track. For some artists (Mac Miller) the system also has track-level last.fm and mb tags so per-track variation survives. For others (Lunar C, Bob Marley) it's fully artist-level. Should the radio code treat artist-uniform tags as a weaker signal, or is it acceptable that some artists are "one block" for coherence purposes?
