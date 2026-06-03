# Daily Chart Ingestion Plan

## Goal

Build a daily chart ingestion pipeline that gives `/charts` richer, Kworb-style market views while preserving NOORwave's playback contract: every chart row should resolve to local library data when possible, then to a playable TIDAL candidate, then to a pending external row if unresolved.

Kworb is useful as a product reference because it shows a country by service matrix and per-service daily chart pages. It should not be the default production dependency unless source permission is confirmed. Prefer official or already-integrated APIs first, then treat third-party HTML as an optional, rate-limited fallback.

## Observed Reference Shape

Kworb is not one chart. It is a broad music data site with several useful shapes:

- Home overview and navigation across iTunes, worldwide, artists, charts, radio, Spotify, YouTube, and trending.
- Current cross-provider country matrix.
- Global artist ranking that combines Apple Music, Spotify, iTunes, YouTube, Shazam, and Deezer signals.
- Spotify daily and weekly country charts with totals and track history links.
- Worldwide and European iTunes and Apple Music song and album charts.
- YouTube video rankings, language filters, artist pages, country pages, and top lists.
- Radio estimates with audience, audience delta, formats, peak audience, and cross-provider positions.
- Artist detail pages showing where a track or artist is charting across services and countries.

Kworb `/charts` shows a matrix by country and source:

- Country
- iTunes top song
- Spotify top song
- Apple Music top song
- YouTube top video
- Shazam top song
- Deezer top song

Kworb Spotify country pages expose richer daily rows:

- Rank and rank movement
- Artist and title
- Days on chart
- Peak position
- Daily streams and stream delta
- Seven-day streams and delta
- Total streams
- Spotify track id embedded in the track-history URL

## Source Catalog

Model chart ingestion as a source catalog, not a one-off Spotify feature. Each source has a stable key, parser, resolver policy, retention policy, and UI capability flag.

Initial source families:

- `charts_matrix`: current top item by country and provider. Good for a fast "what is number one everywhere" view.
- `spotify_daily`: daily track charts by region. Best first playable source because Spotify rows carry strong identifiers.
- `spotify_weekly`: weekly track charts by region. Same resolver path as daily.
- `spotify_totals`: accumulated Spotify totals and historical track stats. Useful for context, not urgent for playback.
- `global_artist`: combined artist points by provider and top country. Good for artist discovery and "global heat".
- `itunes_worldwide`: worldwide and European song or album points. Mostly display plus TIDAL resolve by artist and title.
- `apple_music_worldwide`: worldwide and European song or album points. Mostly display plus TIDAL resolve by artist and title.
- `youtube_daily`: 24-hour video views and likes. Should route through video surfaces, not normal track playback.
- `radio_estimates`: audience and format estimates. Good for trend context and cross-provider comparison.
- `artist_positions`: per-artist cross-service positions. Better as an artist detail enhancement than the first `/charts` page.

Each source family can support several regions, periods, and entity types:

- `entity_type`: `track`, `album`, `artist`, `video`
- `period`: `live`, `daily`, `weekly`, `totals`
- `region`: `global`, ISO country code, or provider-specific region key

## Source Strategy

### Phase 0: Catalog and Snapshot Shell

Add the shared schema, source registry, snapshot read API, and resolver queue without enabling every provider. This keeps the first backend change small and gives `/charts` a stable local data contract.

The first UI can read local snapshots even before all sources exist.

### Phase 1: Charts Matrix and Spotify Daily Charts

Use the country matrix as the browse model and Spotify daily as the first detailed playable source. The `/charts` page should not read as "Spotify charts". It should read as a market pulse surface with provider columns for iTunes, Spotify, Apple Music, YouTube, Shazam, and Deezer.

Provider priority for detailed playable rows:

1. Official Spotify Charts daily CSV or page data where accessible.
2. Existing Sportify playlist/detail endpoints for metadata enrichment only.
3. Kworb Spotify daily pages only if allowed, behind a feature flag.

Do not block UI requests on external fetches. Ingestion runs in the background and `/charts` reads local snapshots.

### Phase 2: Weekly, Totals, and Artist Heat

Add weekly Spotify snapshots, Spotify totals, and global artist rankings. These are useful for trend context but do not need to resolve every row to playback before they can be displayed.

### Phase 3: Last.fm Continuity

Keep the existing Last.fm trending shelf as a live fallback and comparison source. Last.fm remains useful for broad "now moving" trends but should not be the only charts data source.

### Phase 4: Extra Sources

Add providers only when we can resolve rows reliably:

- Apple Music: display-only until we have a resolver path.
- iTunes: display-only or TIDAL resolve by artist and title.
- YouTube: video route integration, not normal track playback.
- Shazam and Deezer: display and TIDAL resolve by artist and title.

## Backend Data Model

Add new append-only migrations:

### `chart_sources`

- `id INTEGER PRIMARY KEY`
- `source_key TEXT UNIQUE NOT NULL`
- `display_name TEXT NOT NULL`
- `provider TEXT NOT NULL`
- `enabled INTEGER NOT NULL DEFAULT 1`
- `default_region TEXT`
- `refresh_interval_hours INTEGER NOT NULL DEFAULT 24`
- `last_success_at INTEGER`
- `last_error TEXT`

Example `source_key` values:

- `charts_matrix`
- `spotify_daily`
- `spotify_weekly`
- `spotify_totals`
- `lastfm_worldwide`
- `global_artist`
- `apple_music_daily`
- `itunes_daily`
- `youtube_daily`
- `radio_estimates`
- `artist_positions`

### `chart_snapshots`

- `id INTEGER PRIMARY KEY`
- `source_key TEXT NOT NULL`
- `region TEXT NOT NULL`
- `period TEXT NOT NULL`
- `chart_date TEXT NOT NULL`
- `fetched_at INTEGER NOT NULL`
- `etag TEXT`
- `content_hash TEXT`
- `status TEXT NOT NULL`
- unique `(source_key, region, period, chart_date)`

`period` starts with `daily` and `weekly`.

### `chart_entries`

- `id INTEGER PRIMARY KEY`
- `snapshot_id INTEGER NOT NULL`
- `rank INTEGER NOT NULL`
- `rank_delta INTEGER`
- `artist TEXT NOT NULL`
- `title TEXT NOT NULL`
- `entity_type TEXT NOT NULL DEFAULT 'track'`
- `album TEXT`
- `external_track_id TEXT`
- `external_artist_id TEXT`
- `external_video_id TEXT`
- `external_url TEXT`
- `streams INTEGER`
- `stream_delta INTEGER`
- `views INTEGER`
- `likes INTEGER`
- `audience REAL`
- `audience_delta REAL`
- `points REAL`
- `points_delta REAL`
- `seven_day_streams INTEGER`
- `total_streams INTEGER`
- `days_on_chart INTEGER`
- `peak_rank INTEGER`
- `provider_positions_json TEXT`
- `raw_json TEXT`
- unique `(snapshot_id, rank)`

### `chart_entry_resolutions`

- `entry_id INTEGER PRIMARY KEY`
- `external_candidate_id INTEGER`
- `local_track_id INTEGER`
- `tidal_id INTEGER`
- `status TEXT NOT NULL`
- `score REAL`
- `resolved_at INTEGER`
- `attempts INTEGER NOT NULL DEFAULT 0`
- `last_error TEXT`

`status` values:

- `local`
- `tidal`
- `pending`
- `unresolved`
- `not_playable`

Use `external_candidate_id` to point at the existing `external_track_candidates` table when a row is not already a local track. Do not create a parallel candidate system for chart rows. The chart resolution table should store chart-specific state and rank context, not duplicate the long-lived external candidate resolver.

## Ingestion Flow

1. Scheduler picks due `(source_key, region, period)` jobs once per day.
2. Provider fetches raw data with timeout, user-agent, and rate limit.
3. Parser normalizes rows into `ChartEntrySeed`.
4. Snapshot transaction inserts `chart_snapshots` and `chart_entries`.
5. Resolver job starts after insert and processes entries in bounded batches.
6. Resolution order:
   - Match local by external id where available.
   - Match local by ISRC where available.
   - Match local by normalized artist and title.
   - Resolve to TIDAL using existing search or direct id path.
   - Leave as pending external when unresolved.
7. UI reads snapshot immediately, then receives updated resolution state via polling or WebSocket event.

## Daily Scheduling

Start simple:

- On server startup, enqueue any chart source whose latest successful snapshot is older than 20 hours.
- Run an hourly lightweight scheduler tick inside `noor-server`.
- Fetch daily charts at most once per source and region per day.
- Provide `POST /api/charts/refresh` for manual refresh in dev only or admin-only later.

No OS scheduler is required. This keeps portable and installed Windows behavior identical.

## API Contract

Add:

- `GET /api/charts/sources`
- `GET /api/charts/snapshots?source=spotify_daily&period=daily&region=AU&limit=50`
- `GET /api/charts/snapshot/{id}`
- `GET /api/charts/matrix?region_group=main`
- `GET /api/charts/artists?source=global_artist&limit=100`
- `POST /api/charts/refresh`

Keep existing `GET /api/charts` as compatibility. It can map to the latest Last.fm snapshot or live Last.fm fallback until the new UI is ready.

`ChartEntry` should gain:

- `rank`
- `rank_delta`
- `period`
- `region`
- `chart_date`
- `source_key`
- `source_label`
- `streams`
- `stream_delta`
- `resolution_status`

## Frontend Plan

Change `/charts` from two unrelated blocks into three sections:

1. `Now moving`: existing Last.fm trending shelf.
2. `Market pulse`: Kworb-style country/provider matrix backed by local snapshots.
3. `Chart playlists`: existing Spotify editorial playlist cards.

The first backend slice remains the additive snapshot endpoint because it is the safest TDD target. The first product UI should still show the market-pulse shape, even when data is empty, so it is clear that Spotify is only one provider.

First visible `Market pulse` shell:

- Rows: `Global`, `US`, `UK`, `AU`, `CA`, and `NZ` first
- Columns: `iTunes`, `Spotify`, `Apple Music`, `YouTube`, `Shazam`, `Deezer`
- Cell state: top item, loading, unavailable, unresolved, or source not enabled
- Click behavior: open the provider and region drilldown when a detailed snapshot exists

First detailed `Daily charts` drilldown:

- Source: `Spotify`
- Period: `Daily`
- Region: selected from the matrix or quick switches
- Layout: compact ranked list, not playlist cards

Daily row shape:

- Rank and movement
- Artwork thumbnail when available
- Title and artist
- Source metric, starting with streams
- Resolution state: in library, playable, resolving, or unresolved
- Play and context menu actions only when the row is resolved enough

The Kworb-style matrix should become the default mode after the matrix snapshot endpoint exists:

- Rows are countries.
- Columns are providers.
- Cells show the current top track, artist, or video.
- Clicking a cell opens the detailed snapshot for that provider and region.
- Resolved cells play or queue directly.
- Unresolved cells offer search, TIDAL resolve, or external open actions.

Daily chart cards should:

- Render immediately from local snapshots.
- Show rank and movement.
- Show streams when present.
- Show views, likes, audience, or points when the selected source provides them.
- Use existing `TrendingShelf` / `ChartMural` playback behavior.
- Show `Resolving...` only for pending rows, not for the whole grid.

## Resolver Rules

Reuse existing helpers where possible:

- `buildTrackMenu`
- `buildTidalTrackMenu`
- pending external queue behavior
- TIDAL search and import paths

Never insert `tidal_id = 0` into durable storage. `0` can remain a frontend placeholder only.

For daily snapshots, resolution is best-effort and incremental. A failed TIDAL lookup should not poison the chart row forever. Retry unresolved rows daily with capped attempts.

## Rollout Steps

1. Add schema and models.
2. Add `ChartProvider` trait and provider capability flags.
3. Add `ChartsMatrixProvider` and `SpotifyDailyProvider`.
4. Add parser tests using saved fixture HTML or CSV.
5. Add snapshot insert and stale-source scheduler.
6. Add resolver job with bounded concurrency.
7. Add `/api/charts/snapshots` and `/api/charts/matrix` read endpoints.
8. Update `/charts` UI to read latest daily snapshot and matrix data.
9. Add manual refresh and diagnostics.
10. Add weekly, totals, artist heat, YouTube, radio, and iTunes/Apple Music in separate PR-sized steps.
11. Consider optional Kworb provider behind `NOOR_CHARTS_KWORB=1` only after source permission and robots review.

## TDD Tracer Bullets

Use vertical slices. Do not write all tests first.

### Slice 1: Latest Snapshot Read

Public behavior: `GET /api/charts/snapshots?source=spotify_daily&period=daily&region=AU&limit=2` returns the latest local snapshot, sorted by rank, without network access.

Red:

- Add a backend route test that seeds two `chart_snapshots` for the same source and region, then asserts the endpoint returns only the newest `chart_date`.
- Assert rank order and `resolution_status`.
- Assert unresolved rows do not expose `tidal_id = 0`.

Green:

- Add append-only schema.
- Add minimal insert helpers for test setup.
- Add the read endpoint and DTO.

Refactor:

- Keep the existing `GET /api/charts` route unchanged.
- Extract DTO conversion only after the route passes.

### Slice 2: Provider Matrix Read

Public behavior: `GET /api/charts/matrix?region_group=main` returns the latest top cell for each configured region and provider without network access.

Red:

- Add a backend route test that seeds latest snapshots for `itunes_daily`, `spotify_daily`, `apple_music_daily`, `youtube_daily`, `shazam_daily`, and `deezer_daily`.
- Assert response rows are countries and columns are providers.
- Assert missing providers return an explicit empty cell, not a route failure.

Green:

- Add a matrix read query over existing snapshot tables.
- Add `/api/charts/matrix`.
- Keep provider availability and source freshness in the response.

### Slice 3: Idempotent Daily Upsert

Public behavior: ingesting the same source, region, period, and chart date twice does not duplicate rows.

Red:

- Add a backend test that calls the snapshot upsert API twice with changed stream counts.
- Assert there is one snapshot and one row per rank.
- Assert updated metric values are visible through the read endpoint.

Green:

- Add transactional snapshot upsert.
- Replace entries for the snapshot or upsert by rank, but keep the external contract stable.

### Slice 4: Spotify Daily Parser

Public behavior: a saved Spotify daily fixture becomes normalized chart rows with rank, title, artist, streams, external id, and date.

Red:

- Add a parser fixture test using saved HTML or CSV.
- Assert the parser handles rank movement and missing optional metrics.

Green:

- Add `SpotifyDailyProvider` parser only.
- Keep fetcher separate so parser tests never hit the network.

### Slice 5: Resolver Candidate Link

Public behavior: an unresolved chart row is linked to `external_track_candidates`, and a later TIDAL resolution updates the chart row without rewriting chart history.

Red:

- Add a backend test that ingests an unresolved row.
- Assert an external candidate exists with the normalized artist and title.
- Assert `chart_entry_resolutions.external_candidate_id` points at it.
- Resolve the candidate, then assert the chart endpoint reports `resolution_status = tidal`.

Green:

- Reuse `upsert_external_track_candidate`.
- Add the smallest chart-specific resolution update path.

### Slice 6: Frontend Matrix and Snapshot Rendering

Public behavior: `/charts` renders the provider matrix shell immediately, then renders a daily chart snapshot drilldown when a provider and region have data. Pending rows do not block resolved rows.

Red:

- Add a Svelte/Vitest test around the market pulse component with mocked matrix and snapshot payloads.
- Assert provider columns include iTunes, Spotify, Apple Music, YouTube, Shazam, and Deezer.
- Assert rank, title, source label, and pending state render in the drilldown.
- Assert the existing Spotify playlist metadata cache test still passes.

Green:

- Add a small `MarketPulseShelf` component.
- Wire it below the existing Last.fm shelf only after the component test passes.

## Grill Findings

### 1. The plan was too broad for a first implementation.

Recommended answer: ship only the snapshot shell plus `spotify_daily` first. The Kworb-style matrix can be the second UI once the read contract exists.

### 2. A generic metrics table can become a junk drawer.

Recommended answer: keep common columns for rank, movement, streams, views, audience, points, and provider positions, but preserve provider-specific leftovers in `raw_json`. Do not add a new column for every provider quirk until the UI needs it.

### 3. Chart resolution must not duplicate existing external candidate logic.

Recommended answer: chart rows should link to `external_track_candidates`. The chart table owns chart context. The external candidate table owns dedupe and long-lived resolution.

### 4. Scraping Kworb directly is a product and legal risk.

Recommended answer: use Kworb to define the product shape. Enable a Kworb provider only behind `NOOR_CHARTS_KWORB=1` after source permission and robots review. First production source should be official Spotify chart data where accessible.

### 5. The existing `/api/charts` contract is live data, not daily snapshots.

Recommended answer: do not mutate it first. Add `/api/charts/snapshots` and `/api/charts/matrix`, then later decide whether `/api/charts` should alias the new default.

### 6. The current `tidal_id = 0` placeholder is frontend-only and dangerous if persisted.

Recommended answer: tests must assert no durable chart row stores `tidal_id = 0`. Unresolved means `tidal_id NULL`, `external_candidate_id` set, and `status = pending` or `unresolved`.

### 7. Scheduler behavior can hide failures.

Recommended answer: ingestion should be callable through a normal Rust service API first, with scheduler as a thin caller. Test ingestion directly before testing hourly ticks.

### 8. The first UI must not wait for resolution.

Recommended answer: render chart rows from snapshots immediately. Resolution should enrich rows incrementally, never gate first paint.

## Test Plan

Backend:

- Parser fixtures for Spotify daily rows.
- Snapshot upsert idempotency.
- Resolver local match by external id, ISRC, and normalized artist/title.
- Retry behavior for unresolved rows.
- API contract tests for latest snapshot and empty source.

Frontend:

- Daily charts render from a cached snapshot payload.
- Pending rows do not block resolved rows.
- Source and region selection persists.
- Existing Spotify playlist cards still use their metadata cache.

## Open Decisions

- Whether to include third-party HTML ingestion at all, or only official sources.
- Which regions to enable by default: likely Global, US, UK, AU, CA, NZ, Germany, France, Japan.
- How much history to retain locally: recommended 90 daily snapshots per source and region.
- Whether chart data should influence automix/discovery scoring or stay display-only.
