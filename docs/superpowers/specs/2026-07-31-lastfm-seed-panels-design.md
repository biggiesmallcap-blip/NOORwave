# Last.fm seed panels: a rolling deck on the track mural

Status: approved, not yet implemented
Date: 2026-07-31

## Problem

The "Last.fm recommended tracks" mural shows 20 tracks that all come from a
single seed. That is the right shape - one seed means one genre, a coherent
"because you played X" set rather than a soup of twelve unrelated reasons - but
it has two consequences worth fixing:

- Eleven of the twelve computed seeds never reach the user. They exist as a
  fallback chain (the first productive seed wins) and nothing else.
- The mural's only variety mechanism is the 6h seed rotation. Whatever it
  served last cycle is gone with no way back. The 2h view rotation that gives
  the artist and album rails their variety is a no-op here: `rotatingWindow`
  returns the list unchanged when it already fits, and the mural holds exactly
  `PANEL_LIMIT` (20) items.

## What we are building

The mural becomes a deck of panels. Each panel is one seed's ~20 similar
tracks, unchanged in character from today. A `< 1/6 >` pager walks back through
previous cycles. Each 6h rebuild pushes a new panel onto the front and drops
the oldest.

Scope is the track mural only. The artist and album rails keep their 50 and 44
items behind the existing 2h `rotatingWindow`, and are not touched.

## Data model

New table, migration 061:

```sql
CREATE TABLE IF NOT EXISTS recommendation_track_panels (
    id           INTEGER PRIMARY KEY,   -- monotonic; ORDER BY id DESC is newest-first
    seed_artist  TEXT NOT NULL,
    seed_title   TEXT NOT NULL,
    seed_reason  TEXT NOT NULL,
    payload_json TEXT NOT NULL,         -- the resolved items, artwork and ids included
    built_at     INTEGER NOT NULL
);
```

Separate from `provider_recommendation_cache` rather than nested inside its
payload. That blob is a flat array the rails filter by `entity_type`, and
nesting panels in it would complicate every reader for one writer's benefit.
A separate table also makes eviction a single `DELETE`.

`PANEL_HISTORY_LIMIT = 6`.

## Transport

`/api/home/recommendations` keeps its current shape exactly. The track shelf
gains one field:

```json
"panel": { "index": 1, "count": 6, "reason": "...", "built_at": 1785456029 }
```

Paging fetches a single panel:

```
GET /api/home/recommendations/track-panel/{index}
  -> { index, count, reason, built_at, items: [...] }
```

Panels are deliberately NOT inlined into the Home payload. Six panels is ~120
track items in every response, which then lands in the persisted query cache;
a full localStorage is a known way to brick this app's boot (an unguarded
`setItem` at module init throws and takes the whole frontend with it). Fetching
on demand keeps that footprint flat, and a panel read is one indexed row.

Index is 1-based and clamped server-side to `[1, count]`.

## Build flow

Slots into the existing progressive-publish pipeline in `rebuild_lastfm_shelf`:

1. Track stage builds the newest panel, **skipping the seed that produced the
   current panel 1**, so a day with no listening does not hand back the same
   mix twice.
2. Publish as the track shelf, exactly as today, AND insert as a panel row.
   Evict rows past `PANEL_HISTORY_LIMIT`.
3. Artist and album stages run unchanged.
4. **Then**, if fewer than `PANEL_HISTORY_LIMIT` panels exist, backfill the
   remainder from the next productive seeds. Background, after the visible
   shelves have published, so a fresh install reaches a full pager within the
   first session without delaying anything on screen.

Steady state is one new panel per 6h rebuild, so an ordinary rebuild costs what
it does today.

## Behaviour

- Boot lands on 1/6, the newest. Pager position is session state and is not
  persisted - "you boot the app and it's a mix".
- Play all / Queue all act on the panel currently shown. That is the "quick
  playlist".
- A thin seed yielding fewer than 20 items still renders; `ChartMural` has
  layouts for 1 through 20.
- No interaction with "View all": `hasMoreThanShelf` is `items.length > 20` and
  the mural is exactly 20, so the track shelf has no View all link today and
  gains none.

## Testing

Rust:
- panel insert ordering and newest-first read
- eviction at `PANEL_HISTORY_LIMIT`
- the seed-differs guard picks the next seed when the newest panel used it
- endpoint index bounds and clamping
- backfill stops at the limit and does not run when already full

Frontend:
- contract test that boot renders the newest panel
- pager fetches by index and does not refetch a panel it already has in memory
- Play all acts on the shown panel, not on panel 1

## Risks

- **Migration id collision.** 061 is free as of writing, but several worktrees
  run concurrently here and two branches can both claim it. Re-check before
  merge.
- **Stale ids in old panels.** A 30h-old panel can hold TIDAL ids that have
  since died. Same exposure as today's 6h cached payload, no worse, and the
  existing resolve-on-play path handles it.
- **Backfill cost on a fresh install.** Five extra panels means up to 100 extra
  artwork resolutions on first run. It is background and post-publish, so it
  should not be felt, but it is the one place this design adds real work.

## Out of scope

- Panels for the artist or album rails.
- Persisting pager position across boots.
- Content-level dedup between panels (the seed-differs guard is the whole
  mechanism; two different seeds returning overlapping tracks is acceptable).
