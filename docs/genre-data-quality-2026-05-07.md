# Genre Data Quality Audit — 2026-05-07

**Date:** 2026-05-07
**Scope:** Library tracks, fresh `noor.db` produced after the May 4 source-aware-scoring refactor.
**Status:** Investigation only. No fixes proposed inside this report — file a separate planning conversation.
**Prior baseline:** [genre-data-quality.md](genre-data-quality.md) (2026-04-30, pre-refactor pipeline).

---

## TL;DR

The May 4 refactor (`feat(genre): score source-aware tags and route context`, [6c924ef](#)) genuinely changed the data shape. Confidence is now per-tag, not flat. Mood/era/activity tags are routed to a separate table instead of being silently dropped. Some specific contamination cases are gone (Marcia Griffiths is no longer tagged as rock; The Gaslamp Killer no longer carries Psychedelic Rock).

**But three serious things show up in the fresh data:**

1. **Coverage collapsed from 86.6% → 50.9% of tracks tagged.** ~14,500 *favorited* tracks (already enriched by both Last.fm and MusicBrainz) ended up with zero genres. The new context-classifier filters more aggressively than the old "anything that resolves" path, and when MB returns nothing, those tracks now have nothing.
2. **Confidence values exceed 1.0 in 789 rows (2.0% of all assignments)** — `score_genre_tags` sums per-canonical scores without a final `min(1.0)` cap. Top values: 2.27 lastfm, 2.25 musicbrainz. Downstream code that treats `confidence` as a probability or a `[0,1]` weight is silently wrong on those rows.
3. **The psychedelic-rock leak that triggered this audit is still present in the new pipeline.** 1200 Micrograms still has `Psychedelic Rock` from lastfm at 0.292 across every track; Suntree, Ajja, Infected Mushroom, Khruangbin all the same. The new scorer correctly de-prioritises it (Psytrance now scores 0.75–1.5 on those tracks via MB) but the leak survives the 0.2 `min_score` floor and still pulls the artist into the Rock family when ancestor-Jaccard expands.

The refactor moved data quality forward on noise filtering but backward on coverage, and didn't fix the cross-family contradiction problem.

---

## Headline metrics — fresh DB vs prior baseline

| Metric | 2026-04-30 (old) | 2026-05-07 (new) | Δ |
| --- | ---: | ---: | ---: |
| Tracks total | 35,419 | 35,297 | -122 |
| Tracks with ≥1 genre | 30,675 (86.6%) | 17,963 (50.9%) | **-12,712** |
| Tracks with no genre, *favorited* | small | 14,493 | **+14,493** |
| Tracks with no genre, stream-only | 11 | 2,841 | +2,830 |
| Total genre assignments | 77,565 | 39,590 | **-37,975** |
| Last.fm rows | 51,412 | 11,376 | **-40,036** |
| MusicBrainz rows | 26,153 | 28,214 | +2,061 |
| Tidal rows | 0 | 0 | — |
| Spotify rows / `spotify_checked` | 0 / 0 | 0 / 0 | — |
| `track_context_tags` rows | (table didn't exist) | 15,605 | new |
| `lastfm_unresolved_tags` | 0 | 0 | — |
| Genres in taxonomy | 285 | 286 | +1 |
| Genres actually used | 241 | 205 | -36 |

**Reading:** Last.fm assignments fell by ~78%. The new context classifier is rejecting most last.fm tags as non-genre (era / mood / energy / noise) instead of forcing them into `track_genres` at flat 0.40. That's the right direction for noise — but it's the same axe that's now stripping favorited tracks bare when MB doesn't fill in.

### Source breakdown (fresh)

| Source | Rows | Tracks | Avg conf | Min | Max |
| --- | ---: | ---: | ---: | ---: | ---: |
| `musicbrainz` | 28,214 | 10,997 | 0.55 | 0.20 | **2.25** |
| `lastfm` | 11,376 | 9,185 | 0.37 | 0.20 | **2.27** |

Confidence is now genuinely distributed (full curve from 0.20 to >2.0), not the flat 0.40/0.85 of the old pipeline.

### Top genres (fresh)

| Genre | Uses | % |
| --- | ---: | ---: |
| Hip-Hop | 6,461 | **16.32%** |
| Rock | 3,867 | 9.77% |
| Pop | 3,734 | 9.43% |
| Electronic | 2,087 | 5.27% |
| Rhythm and Blues | 2,032 | 5.13% |
| Alternative Rock | 1,328 | 3.35% |
| Classic Rock | 1,185 | 2.99% |
| Psychedelic Rock | 827 | 2.09% |
| Jazz | 823 | 2.08% |
| Country | 677 | 1.71% |

Hip-Hop nearly doubled in relative share (was 9.4% → 16.3%) because the new scorer produces many more granular Hip-Hop sub-genres per track (Mac Miller now spans 25 distinct genres including Cloud Rap, Jazz Rap, East Coast, Conscious, Trap, etc).

### Root-family rollup (ancestors expanded)

| Family | Assignments |
| --- | ---: |
| Rock | 10,917 |
| Hip-Hop | 7,828 |
| Electronic | 5,840 |
| Pop | 4,900 |
| R&B and Soul | 3,407 |
| Folk and Country | 2,381 |
| Jazz | 1,473 |
| Blues | 1,130 |
| Reggae and Caribbean | 877 |
| Ambient and Experimental | 338 |

Rock-family is ~2× the size of Electronic-family despite the user's library being electronic-heavy. Some of that is genuinely lots of rock-tagged music; some is the persistent psy → "Psychedelic Rock" leak.

### Taxonomy depth distribution (fresh)

| Level | Genres used | Assignments | % |
| --- | ---: | ---: | ---: |
| 0 (root: Rock / Pop / Hip-Hop / …) | 10 | 17,623 | **44.5%** |
| 1 (Classic Rock, Deep House, …) | 145 | 20,837 | 52.6% |
| 2 (Tech House, Modal Jazz, …) | 50 | 1,130 | 2.9% |

Slightly worse top-heaviness than old DB (was 38.5% / 55.6% / 5.8%). Phase 2b ancestor-Jaccard math has even less depth to work with.

---

## What the May 4 refactor actually changed

The refactor introduced two new modules and rewired both enrichment paths:

- **`genre/scorer.rs`** (`score_genre_tags`) — per-tag scoring with explicit source × level weights. MB Genre @ Recording = 1.0; Last.fm Track @ Recording = 0.7; Last.fm Artist @ Artist = 0.18 (filtered by 0.2 floor). Includes log-scaled `confidence_from_count` and a `suppress_parents` step that downweights a parent genre when its child is also tagged.
- **`tags/context.rs`** (`classify_tag_context`) — routes raw tags into Genre / Mood / Energy / Occasion / TimeOfDay / Activity / Psychedelic / Tempo / Era / Noise. Only `Genre` continues to `track_genres`; non-noise non-genre flows to `track_context_tags` (new table, migration 028). Critically, a tag is routed to `Genre` only if `catalog.resolve_single(name).is_some()` — i.e. the canonical resolver already accepts it.

Both `lastfm/enrichment.rs` and `services/musicbrainz.rs` were rewritten to call this stack and write the computed score directly into `track_genres.confidence`.

**Wired-up status:** confirmed — both writer paths use `score_genre_tags`. The fresh-DB confidence curve and the populated `track_context_tags` are the evidence.

**Side-effects of the refactor that aren't obviously intended:**

- The bb4335f instrumentation (Apr 30) made `lastfm_unresolved_tags` capture canonical-resolution failures. The May 4 refactor 4 days later gates `TagContext::Genre` on `resolve_single().is_some()` — so unresolved tags now flow into `Noise` and never reach the unresolved-write branch. **Result: `lastfm_unresolved_tags` is structurally unreachable again, just by a different path than before.** 0 rows in fresh DB. The triage signal we added is back to dead.
- `score_genre_tags` accumulates per-canonical scores via `+=` and never re-clamps. Two strong tag sources hitting the same canonical (e.g. MB recording-level "psytrance" + MB release-group "psytrance") sum to >1.0. Then `suppress_parents` only adds parent rows; it doesn't cap children. **The `confidence` column is no longer bounded `[0,1]`.**

---

## Spot-check: 1200 Micrograms (the audit trigger)

| Track | Genre | Source | Confidence |
| --- | --- | --- | ---: |
| Acid For Nothing | Psytrance | mb | 1.50 |
| Acid For Nothing | Electronic | mb | 0.75 |
| Acid For Nothing | Psychedelic Rock | lastfm | 0.292 |
| Renaissance Superman | Psytrance | mb | 0.75 |
| Renaissance Superman | Electronic | mb | 0.75 |
| Renaissance Superman | Psychedelic Rock | lastfm | 0.292 |
| Speed Of Light | Psytrance | mb | 0.75 |
| Speed Of Light | Electronic | mb | 0.75 |
| Speed Of Light | Psychedelic Rock | lastfm | 0.292 |

The new pipeline correctly weights Psytrance highest. But `Psychedelic Rock` from last.fm at 0.292 is above the `min_score = 0.2` floor and gets persisted. Phase 2b ancestor-Jaccard then expands `Psychedelic Rock → Rock`, and the artist still surfaces in rock contexts. **Symptom unchanged from a UX perspective.**

The same leak persists (counts of `Psychedelic Rock` from lastfm in fresh DB):

| Artist | Tracks | Verdict |
| --- | ---: | --- |
| Jimi Hendrix | 69 | ✅ correct |
| The Brian Jonestown Massacre | 41 | ✅ correct |
| Suntree | 40 | ❌ psytrance |
| Khruangbin | 24 | ❌ Thai funk/dub |
| Traffic | 22 | ✅ correct |
| King Gizzard & The Lizard Wizard | 22 | ✅ correct |
| Tame Impala | 17 | ✅ correct |
| **Infected Mushroom** | **17** | **❌ psytrance** |
| The Doors | 16 | ✅ correct |
| **Ocean Alley** | **13** | ⚠️ surf/reggae rock — debatable |
| **Ajja** | **9** | **❌ psytrance** |
| 1200 Micrograms | (8 — listed above) | **❌ psytrance** |

The tag is correctly applied for the genuinely-psychedelic-rock acts. The cross-family contamination problem (psytrance acts inheriting `psychedelic rock` from last.fm crowd shorthand) survives the refactor.

---

## Spot-check: other artists

| Artist | Genres assigned | Verdict |
| --- | --- | --- |
| Marcia Griffiths | Reggae, Roots Reggae, Dancehall | ✅ **fixed** (was leaking via Rocksteady) |
| The Gaslamp Killer | Hip-Hop, Electronic, Downtempo, Dubstep | ✅ **fixed** (was Psychedelic Rock in old DB) |
| Bob Marley & The Wailers | Reggae, Roots Reggae, Rock, Blues, Dub, Ska, Dancehall, Afrobeats, Downtempo | ⚠️ Rock + Afrobeats still leaking |
| Khruangbin | Psychedelic Rock, Rock, Blues Rock, Country, Alt-Country, Americana | ❌ none of these fit |
| Mac Miller | 25 distinct genres including Cloud Rap, Jazz Rap, Trap, Conscious Hip-Hop, Trip-Hop, Indie Pop, Alternative Rock, Hard Rock, Synthpop, Glitch | ⚠️ rich but noisy — Hard Rock and Glitch are wrong |
| The Weeknd | Electronic, Pop, Contemporary R&B, R&B, Jazz, Dubstep, Trap, Hip-Hop, Synthpop, Trance, Rock, Ambient, Reggae | ❌ Reggae and Trance are wrong |
| Norah Jones | Jazz, Vocal Jazz, Blues, Pop, Folk | ✅ reasonable |
| Suntree | Progressive Trance, Psychedelic Rock | ❌ Psychedelic Rock wrong |
| Ace Ventura | Progressive Trance, Electronic, Psytrance | ✅ clean |
| Captain Hook | Progressive Trance, Psytrance, Dubstep, Glitch, Trance | ✅ reasonable |

The refactor cleaned up some artists (Marcia Griffiths, Gaslamp Killer, Captain Hook, Ace Ventura) and made others *more* contaminated (The Weeknd picking up Reggae+Trance, Mac Miller picking up Hard Rock+Glitch). Net direction: more granular tagging, more visible contradictions, more cross-family bleed.

---

## `track_context_tags` health check

The new table is populated and looks broadly sane:

| Context | Rows | Tracks | Distinct tags |
| --- | ---: | ---: | ---: |
| era | 11,391 | 9,504 | 13 |
| activity | 2,430 | 2,429 | 3 |
| energy | 816 | 793 | 12 |
| mood | 569 | 516 | 13 |
| occasion | 391 | 370 | 8 |
| time_of_day | 6 | 6 | 3 |
| tempo | 2 | 2 | 1 |

**Top context tags:**

| context | tag | uses |
| --- | --- | ---: |
| era | 70s | 3,399 |
| era | 60s | 3,385 |
| activity | **dance** | **2,377** |
| era | 80s | 2,179 |
| era | 90s | 1,710 |
| energy | chill | 560 |
| era | 10s | 306 |
| energy | mellow | 182 |
| occasion | club | 145 |
| mood | beautiful | 136 |

**One classification call worth challenging:** `dance` is in `ACTIVITY_TAGS` ([`tags/context.rs:122`](../noor-server/src/tags/context.rs#L122)). 2,377 occurrences. On last.fm "dance" is overwhelmingly used as a genre indicator (dance music) — by routing it to `activity` we strip the strongest electronic-genre signal from a lot of tracks. The same line bundles `dance` next to `dancing`, which IS clearly an activity. Worth deciding whether `dance` belongs there or should fall through to canonical resolution (it's already aliased — see below).

`time_of_day` and `tempo` are basically empty. Either the lists need expansion or last.fm tags rarely use them and they can be folded into other contexts.

---

## Coverage gap detail

The single biggest concrete issue:

| Bucket | Count |
| --- | ---: |
| Tracks with no genre | 17,334 |
| ↳ favorited / in favorited album | **14,493** |
| ↳ stream-only | 2,841 |

In the old pipeline, every favorited track that ran through last.fm enrichment got *something* — even if it was flat-0.40 noise. The radio always had a coherence signal to work with. In the new pipeline, when the new context classifier rejects every last.fm tag as non-Genre and MB returns no usable tags either, the track ends with a clean empty record.

**Implications for downstream consumers** (radio, Genre Galaxy, discovery):

- Phase 2b ancestor-Jaccard returns 0 against any seed-or-candidate with no genres → those tracks become "coherence dead".
- Whatever fallback existed when *some* genre was present (even if noisy) no longer fires for these 14k tracks.
- Genre Galaxy heatmaps will visibly thin out for any cluster whose membership relied on noisy last.fm coverage.

This is a product trade-off, not a bug — but it's the trade-off and it's worth deciding consciously.

---

## Stuck items (no progress since prior audit)

- **Tidal genre source**: 0 rows in `track_genres` with `source='tidal'`; 0 genres with `tidal_genre_id` set. Code path exists at `routes.rs:7901 → genre/builder.rs:69 → db/queries.rs:1040` but produces nothing live. See `docs/tidal-genre-source-investigation.md` (referenced by [bb4335f](#)). Highest-quality available source, still inactive.
- **Spotify enrichment**: 0 rows in `spotify_checked`, 0 rows with `source='spotify'`. Never run.
- **Manual assignments**: 0 rows.

---

## Top 5 issues, ranked by severity

### 1. 41% of favorited tracks now have zero genre data

**Severity: high.** The new context classifier raised the floor on tag quality and dropped coverage with it. 14,493 favorited tracks are coherence-dead. Anything radio/galaxy/discovery does that depends on `track_genres` is silently degraded for a meaningful fraction of the library.

### 2. `confidence` is no longer bounded to `[0,1]`

**Severity: high.** 789 rows (2.0%) have `confidence > 1.0`, max 2.27. Cause: `score_genre_tags` sums by canonical without a final clamp. Any consumer that interprets `confidence` as a probability, weights it, or uses it in a `WHERE confidence >= X` threshold is silently miscalibrated on those rows. Schema default is `1.0`; semantically the column is a sum-of-evidence score.

### 3. Psychedelic-rock cross-family leak survives the refactor

**Severity: medium-high.** Same failure mode as the prior audit's "Bob Marley → Classic Rock" finding. `psychedelic rock` from last.fm Track @ Recording scores 0.7 × ~0.42 = ~0.29, above `min_score = 0.2`. Affects ~5 psytrance/world acts visibly (Suntree, Infected Mushroom, Ajja, 1200 Micrograms, Khruangbin). The fix would be cross-family contradiction logic — none exists.

### 4. `lastfm_unresolved_tags` instrumentation is unreachable again

**Severity: medium.** Apr 30 fix made it capture canonical-resolution failures. May 4 refactor gates `TagContext::Genre` on `resolve_single().is_some()`, so canonical-failure tags now route to `Noise` instead. 0 rows in fresh DB. The triage signal we wanted is dead by a different mechanism than before.

### 5. `dance` is classified as activity, not genre

**Severity: medium.** 2,377 tags routed to `activity` that almost certainly meant the genre. `tags/context.rs:122` lumps `dance` with `dancing`, `running`, `driving`. This silently strips the strongest electronic-music signal from anything tagged just "dance" without "house"/"techno"/"trance". Worth at least confirming whether `dance` resolves through the alias path (it should — there's a `Dance Pop` genre and `dance` is a common electronic shorthand).

---

## Top 5 questions for the user

1. **Is the coverage drop acceptable?** 14,493 favorited tracks now have zero genres vs ~few in the old pipeline. Does the radio/galaxy quality with cleaner-but-sparser data feel better than dirtier-but-fuller data, or worse? Determines whether we tighten the new filter, loosen it, or add a fallback "anything resolved" rescue path for tracks that would otherwise be empty.
2. **Should `confidence` be clamped at 1.0?** The unclamped sum is conceptually meaningful (more evidence = higher score), but the column name says "confidence" which has a different mental model. Either rename the column to "score", or `min(1.0)` in the writer, or add a separate `evidence_score` column.
3. **Is cross-family contradiction worth implementing now?** The fix that would actually clear the 1200 Micrograms / Bob Marley class of issue: when an artist's high-confidence MB family is X (Reggae, Electronic), suppress low-confidence last.fm tags from incompatible family Y (Rock, Pop). This was the lever proposed in the prior audit and no longer needed for *coverage* reasons (the tags are already low-conf), but still leaking into family rollups.
4. **Should we re-route `lastfm_unresolved_tags` to capture the genuinely interesting tags?** With the new pipeline these are tags that pass the noise filter but aren't canonical and aren't in any context list — i.e. potential new aliases or new genres. Right now they fall into `Noise` silently. Worth adding a sink so we can actually mine them for taxonomy gaps.
5. **`dance` as activity vs genre — what's the intended call?** Cheap to flip; 2,400 rows of impact if we move it.

---

## Files & queries used

- DB: `<local-db>` (fresh, post-May-4-refactor)
- Comparison DB: `<comparison-db>` (older dev DB, mostly pre-refactor data)
- Code: [`noor-server/src/genre/scorer.rs`](../noor-server/src/genre/scorer.rs), [`noor-server/src/tags/context.rs`](../noor-server/src/tags/context.rs), [`noor-server/src/services/lastfm/enrichment.rs`](../noor-server/src/services/lastfm/enrichment.rs), [`noor-server/src/services/musicbrainz.rs`](../noor-server/src/services/musicbrainz.rs)
- Prior audit: [`docs/genre-data-quality.md`](genre-data-quality.md)
