# Atlas and Air: build plan

Date: 2026-10-14. Builds the video exploration end game ("Atlas and Air")
in one pass. Design sources: the end-game spec (Atlas and Air) and the
earlier On Air / Atlas / Wall draft; this plan is the merged, final version
and wins where they differ.

Contracts are code, not prose:
- Tables: `MIGRATION_077` in `noor-server/src/db/schema.rs`.
- Wire shapes: `frontend/src/lib/api/video_explore_types.ts`.
- Client calls: the "Atlas and Air" block in `frontend/src/lib/api/client.ts`.
- Routes: registered in `noor-server/src/server/routes.rs`, handlers in
  `video_intel_routes.rs`, `video_atlas_routes.rs`, `video_channel_routes.rs`.

## 1. What the data says (snapshot of the installed DB, 2026-10-14)

- 162,349 videos, 15,214 artists, 11,556 hours; 150,590 programmable.
- Graph: 88,175 edges (lastfm 33,192, featured 29,184, tidal 16,939,
  colist 8,844); 25,173 nodes; largest component 23,542. 5,180 artists with
  videos (34%) have no edge: uncharted. Edge weights pile at 0.8: treat them
  as ordinal.
- Louvain at resolution 1.0 gives 489 communities; splitting communities over
  5,000 videos at resolution 2.0 and dissolving those under 300 videos into
  their best-connected neighbour leaves about 83 scenes covering 150,871
  videos.
- Tags: only 19% of artists carry any genre tag, so auto-naming from tags
  fails (Volbeat lands in "Classical"). Name scenes by exemplars.
- Taste: library artists reach 61,157 videos (38%); watched artists reach
  10,592. Roots come from the library first.
- Watch behaviour is binary: of 238 timed plays, 97 stopped at 0-10%, 72 ran
  to the end. Zaps are the negative signal; completion the positive.
- Titles: TIDAL type says Live on 380 rows; titles say live on 9,402, about a
  third only in Spanish, Portuguese or French ("en vivo" 1,327, "ao vivo"
  911). The classifier is multilingual from day one.
- Songs: 130,734 song keys; 20,335 with 2+ cuts. Same-titled cuts are real
  versions (Westlife "Swear It Again" has 14, mostly concert-film clips under
  the bare title), never duplicates. 513 concert batches of 3+ live cuts.
- release_year is the upload year (55% say 2020s). Album link exists on 92
  rows, so album-year repair goes through library_videos -> tracks -> albums.

## 2. Final decisions (tweaks over the end-game spec)

1. Channel numbers are stable forever. A scene gets the next free number the
   first time it qualifies (30 h programmable and 40 artists) and keeps it
   (`video_scenes.channel_no`, matched across rebuilds by stable scene id).
   Appetite orders the On Air landing and Wild Card, never the numbers.
2. Fixed channels: 1 Your Channel, 2 Wild Card, 3 Premieres (ingested in the
   last 14 days), 4 Live Stage, 5 Sessions, 6 Concert Hall (whole concerts
   back to back), 7 Rewind (song_year decade of the day; locked until era
   coverage is 50%+ of the pool), 8 Spotlight (the day's artist, exists).
   Scene channels start at 10.
3. The Atlas renders with Canvas 2D and cached sprites (two prototypes drew
   15k artists smoothly). No WebGL dependency in this pass. The payload is
   columnar JSON; binary only if 60k artists ever profiles slow.
4. `/videos` opens On Air: a dial with what is on now on every channel
   (artwork, title, progress through the current video). Nothing autoplays on
   landing; picking a channel tunes in at the scheduled offset.
5. Tabs: On Air, Atlas, Archive, Logbook. Stations, Liked, History and TIDAL
   editorial stay reachable as child pages (Stations under On Air, Liked and
   History under Logbook, editorial under Archive).
6. Scene names: a short curated table maps a signature artist (by TIDAL id)
   to a display name for the big scenes so day one reads like a real dial
   ("Bay Area Rap", "K-pop"); every other scene is named by its top two
   exemplars ("Coldplay & Arcade Fire"). `pinned` marks curated names.
7. Era: no MusicBrainz in this pass. song_year comes from library matches,
   context era tags, the earliest cut year and the artist range, with source
   and confidence.
8. Light is computed per request (watch history and likes change all the
   time). Explore mode persists in server_config `video_air.explore`.
9. Session signals are read from video_history (zap = stopped inside 15 s or
   under 10%; hold = 70%+ or completed) plus the +/- ids the client sends with
   refills. No new table.
10. Routes (shortest weighted path between two artists, playable one video per
    hop) and probes (click an uncharted artist, the crawler takes it on the
    urgent path) ship in this pass: they are the Atlas's "wow".

## 3. Shared rules for every worker

- Work only in the files you own (section 4). If a shared file needs a
  change, make the smallest possible edit and list it in your final report.
- ASCII only in code, comments, docs and commit messages: no em dashes, no
  curly quotes, no non-ASCII symbols. Use -, ->, x, ...
- Commits: conventional style (`feat(videos): ...`), explain why in the body,
  no AI or co-author trailers. Stage only your own paths (`git add <paths>`).
  Never push.
- Rust: pure functions over pre-fetched data where possible; neutral on
  missing data, never panic on real data; `cargo test` for your module and
  `cargo clippy` clean for your files. Toolchain floor is 1.96 (run
  `rustup update stable` if rusqlite fails on `cfg_select`).
- Frontend: Svelte 5 runes, follow `frontend/STYLING.md` (tokens, `.t-*`
  type roles, command header, motion and reduced motion), the Search page's
  clean borderless card look, and the shared context menu builders on every
  track, artist and video reference (`buildVideoMenu`, `buildArtistMenu`).
  TIDAL artwork goes through `upscaleTidalArtwork(url, size)` with sizes
  80/160/320/640/750/1080/1280 and every `<img>` has an onerror fallback.
  Contract tests in the existing style (`*_contract.test.ts` reading source)
  plus unit tests for pure logic. Run `npx vitest run <your paths>` and
  `npx svelte-check --threshold error` before you finish.
- Never read or write the live database. Tests build their own in-memory
  databases (`crate::server::routes::tests::fresh_migrated_db()` gives a
  migrated one).

## 4. Work packages

### W1 Catalog (backend)

Owns: `services/video_intel/{cuts,songs,concerts,era,link,census,build,job}.rs`,
`server/routes/video_intel_routes.rs`, and the P0 edits in
`services/video_stations/{pool,pick}.rs`, `services/video_radio.rs`,
`services/video_sets.rs`.

- cuts: classify every catalog video into `VideoCut` (see the TS type) from
  title markers in English, Spanish, Portuguese, French, Italian and German
  (live: live, en vivo, ao vivo, en directo, en concierto, live at, in
  concert, concert, unplugged...; official: official video, clip officiel,
  videoclip, video oficial, clipe oficial...; lyric: lyric, letra, lyrics,
  paroles, testo; session: session, acoustic, acustico, acoustique, stripped,
  tiny desk; visualizer: visualizer, visualiser, official audio, (audio);
  remix: remix, rework, edit, version; dance: dance practice, choreography,
  vertical; interview: interview, behind the scenes, making of, commentary,
  documentary, episode, track by track; trailer: trailer, teaser), TIDAL type
  as a tiebreaker, duration under 90 s = clip, 30 min+ = longform.
  `programmable` = not clip, interview, trailer; not Various Artists (2935 or
  name key "various artists"). Song key: reuse `video_radio::video_song_key`.
  quality = popularity percentile ^0.5 x runtime sanity x artwork present.
  Fixtures from real titles.
- songs: roll up by song key, best cut in library_videos order (best match,
  then longest among equals; prefer official over live over lyric), cuts_mask.
- concerts: per artist and cut year, live cuts sorted by id, split on gaps
  over 400, runs of 3+; label from the common title suffix ("Live at Trianon
  2015") else "Concert, {year}"; long-form live singles are concerts of one.
- era: song_year precedence library album year (via library_videos ->
  tracks -> albums, or artist + normalized title match) > track_context_tags
  decade for matched tracks > earliest cut year across the song's cuts
  (conf low) ; era_source and era_conf.
- link: songs.scene_id from video_artist_facets; artist playable and hours;
  assign channel numbers to scenes that newly qualify (30 h, 40 artists),
  never reusing a number.
- census: every number in `VideoCensus`.
- job: 2 minutes after boot, then every 6 hours, rebuild on
  `db.open_isolated()` when never built, older than 24 h, or the catalog grew
  1%+ since the last run; one at a time; record each stage in
  video_intel_runs. `POST /api/videos/intel/rebuild` kicks it.
- routes: census, archive (lens query over video_songs with facet counts
  under the other active lenses, one tile per song, under 150 ms on 130k
  songs; light per row), song versions (labels: concert label, else the
  title's bracketed suffix, else "{cut}, {cut_year} upload batch" + runtime),
  concert setlist.
- P0: `pool::is_excluded` delegates to the cut classifier; video_radio and
  video_sets drop non-programmable videos; pick.rs samples the artist first,
  weighted by sqrt(playable videos), then the video.
- Done when: a full build over a copy of the real DB completes in under two
  minutes; census shows 500+ concerts and cut counts within 5% of section 1;
  no interview, commentary, trailer or sub-90 s clip on any station, radio or
  shelf; tests cover classifier fixtures in four languages, concert batching,
  best-cut order, era precedence, archive facets.

### W2 Atlas (backend)

Owns: `services/video_intel/{louvain,scenes,spread,layout,light}.rs`,
`server/routes/video_atlas_routes.rs`.

- louvain: CSR adjacency, deterministic (sorted ids, fixed tie-breaks),
  weighted modularity with a resolution parameter; no new crate.
- scenes: undirected noisy-OR merged graph from video_related_artists; split
  communities over 5,000 videos at resolution 2.0; dissolve under 300
  videos into the best-connected neighbour; stable ids (a new scene inherits
  the id, name, pinned flag, hue, position and channel_no of the previous
  scene whose members overlap with Jaccard >= 0.5); soft second scene when
  >= 30% of an artist's weight points there; exemplars = top artists by
  videos x degree; curated names table keyed by signature artist TIDAL id
  (build it from the snapshot's biggest scenes: Latin pop, trap, golden-era
  rap and R&B, songwriters, EDM, Nashville, alt rock and metal, indie,
  classic rock, new wave, francophone, UK rap and grime, underground
  electronic, Italia, crooners, reggae, euro dance, classical, gospel,
  trip-hop, funk and disco, Brazil, K-pop, Bollywood, Bay Area rap, drum and
  bass, jazz). Writes video_scenes and video_artist_facets (scene, charted,
  name, videos).
- spread: label propagation over edges from tagged artists (seed genres,
  library genres, Last.fm cache), inverse label frequency prior, 0.6 decay
  per hop, 2 hops, keep only tags that agree with the artist's scene
  majority. Target 85%+ of charted artists.
- layout: scene graph force layout (weight / sqrt(size_a x size_b)), disks
  sized by sqrt(videos) packed with a gap, artists inside their disk at a
  radius by rank (hubs centre, singles rim) and a hashed angle; warm start
  from previous positions; uncharted artists on an outer ring grouped by
  name initial. Hue = angle of the scene centre around the map centre.
  Map is 1000 x 1000.
- light: per artist 0..3 as defined in the TS `Light` type.
- routes: atlas (columnar, under 1 MB for 15k artists, built in under
  300 ms), names, artist card (best cuts, neighbours with light), scene card
  (top artists, spaced sample), probe (register the artist as a station root
  and request urgent crawler work through the existing `building` path;
  respect the discovery setting), route (Dijkstra over -ln(weight), 6-hop
  cap, one best programmable video per stop).
- Done when: a build over the real graph gives 60-100 scenes with stable ids
  across two consecutive builds (same ids, positions within 1 unit), genre
  coverage 85%+ of charted artists, the atlas payload under 1 MB; tests cover
  Louvain on known graphs, stable-id matching, spread on a toy graph, layout
  determinism, route search.

### W3 Air (backend)

Owns: `services/video_air/**`, `server/routes/video_channel_routes.rs`.

- taste: roots per artist = library artist 0.6 + 0.1 per liked track (cap
  1.0) + log1p(listens)/4 + watched-and-enjoyed (roots.rs) + saved 1.0 +
  station on air; relevance via video_discovery::graph propagate (3 hops,
  unchanged); scene appetite = root weight in scene / sqrt(scene artists),
  normalised, floor 0.05, +0.1 curiosity for scenes with no plays in 30 days.
- programmer: pool = programmable facets matching the channel; artist_w =
  sqrt(playable) x scene_fit x ring_weight(explore) x session_boost; video_w
  = quality x cut_pref x freshness x skip_damp x hold_rate. Rings by explore:
  familiar 80/18/2, mixed 60/30/10, adventurous 35/40/25 (near >= 0.3, mid
  >= 0.08, same thresholds as pick.rs). Format rules: no artist twice in 12
  slots, no song twice in a 6 h block, best cut unless the channel is about a
  cut, one frontier pick in four (appetite-weighted), concerts play whole on
  Concert Hall, heavy/medium/light rotation by quality. One why chip per
  pick, priority concert > near a root ("Near James Blake, 33 plays") >
  scene > cut > era > new.
- channels and clock: fixed channels 1-8 and scene channels (section 2).
  A channel's day is four 6-hour blocks built with seeds (channel, day,
  block); stored in video_channel_day; built lazily on first request of the
  day, all channels in one pass under 2 s. Locked channels report unlock
  progress. now/next by local time.
- routes: channels (every channel with now/next and offset), guide (window
  of programs), tune (scheduled video, offset, next 12), next (refill after
  the schedule for a session, honouring more/less ids and exclusions),
  explore get/put.
- Done when: channels answer in under 300 ms after the day's build; tests
  prove determinism per seed, no artist twice in 12 slots and no song twice
  per block over 10k simulated slots, concerts never split, locked rules,
  offsets correct across block boundaries and midnight.

### W4 Atlas (frontend)

Owns: `frontend/src/routes/videos/atlas/**`, `frontend/src/lib/video/atlas/**`
(renderer, camera, hit testing, sprites, labels, store, mini map component
`AtlasMiniMap.svelte` for the watch page).

- A full-bleed map under the command header. Canvas 2D with per-scene
  sprite caches, devicePixelRatio aware, level of detail by zoom (scene names
  zoomed out, hub names mid, every name in; reuse `placeLabels` from
  `lib/components/Genre/galaxyLabels.ts` without changing it).
- Two looks: "Light is knowledge" (lit glowing, warm full colour, dim
  quiet, uncharted grey ring) and "Color is place" (every point in its scene
  hue). Scene disks as soft halos, never hard circles.
- Interactions: drag to pan, wheel and pinch to zoom around the cursor,
  hover tooltip, click a scene -> scene card (Tune in -> On Air channel when
  channel_no exists, Open in Archive, sample strip), click an artist ->
  artist card (play best cuts, play radio, neighbours, Send a probe for
  uncharted), search field flies the camera (the section's search field when
  on this tab), Route mode (pick two artists, draw the path, Play route).
  Pulses where the crawler touched artists in the last hour. Keyboard: arrows
  pan, +/- zoom, F fit, Esc closes cards.
- 60 fps at 15k artists on a mid laptop; no allocation per frame in the draw
  loop.
- Done when: the page renders the real atlas, every interaction above works,
  reduced motion disables flights and pulses, tests cover camera math, hit
  testing, label LOD and the payload decoder.

### W5 On Air (frontend)

Owns: `frontend/src/routes/videos/+page.svelte`, `frontend/src/lib/video/onair/**`,
`frontend/src/lib/stores/video_session.ts`, `frontend/src/lib/components/video/VideoPlayer.svelte`,
`frontend/src/lib/components/video/VideoDock.svelte`,
`frontend/src/lib/components/video/FullscreenVideoQueue.svelte`, and the
stations page link.

- Landing: the dial. Each channel a row or card with its number in mono,
  title, subtitle, now-playing artwork and title, a thin progress bar
  through the current video that moves in real time, next up. Locked scene
  channels show "coming up" with hours progress. A small explore control
  (Familiar / Mixed / Adventurous). The day's channels in number order;
  a short "For you tonight" strip first, ordered by appetite.
- Tune-in: `playVideoChannel(id)` tunes via the API and starts at
  `offset_s` (VideoPlayer gains a start position on first load; skip to the
  next item when the offset is past 80%); refills through the channel next
  endpoint; zap/hold flow through the existing history recording.
- On the stage (watch page and fullscreen): a broadcast lower third (artist
  in Manrope, "song" in Newsreader italic, year, cut, runtime, why chip; in
  for 6 s at start and the last 10 s with Up next), a channel bug (number and
  name, top right), Up/Down to flip channels with a 160 ms luminance dip and
  the next channel's card shown instantly while the stream resolves, digits
  to type a channel, G for the guide overlay (grid of channels x time with
  scene-hue blocks and a now line), +/- more/less like this, Esc to leave.
  The stage stays dark in every theme.
- Done when: landing renders real channels, tune-in lands mid-video at the
  scheduled offset, flips work in fullscreen, the guide renders, reduced
  motion removes the dip, tests cover the key map, offset math, refill
  wiring and the lower-third timing.

### W6 Archive, Logbook, Stage (frontend)

Owns: `frontend/src/routes/videos/archive/**`, `frontend/src/routes/videos/logbook/**`,
`frontend/src/routes/videos/watch/+page.svelte`, `frontend/src/lib/video/archive/**`.

- Archive: a lens bar with live counts (scene chips with hue dots, cut,
  era scrubber drawn as a small river from the decade facet, depth big ->
  deep, length, source), sort, then one tile per song (artwork, title,
  artist, a light dot, an "x14 cuts" badge), infinite scroll. Clicking the
  badge opens the Versions drawer (the Liked version list's look: best
  first, label, cut chip, year, runtime, concert link); a concert opens its
  setlist with Play concert. TIDAL editorial stays reachable from here.
- Logbook: Liked, Saved and History as entries, plus the census as a calm
  page of numbers and per-scene coverage bars (seen N of M). No badges.
- Stage (watch page): below the player, rows: This concert (when the video
  is in one), Same scene, One step out, Other cuts of this song, built from
  the new endpoints; keep every existing watch-page contract; leave a
  `<div class="atlas-minimap-slot">` where the mini map will mount.
- Done when: lenses answer and update counts, versions and concerts play,
  Logbook links work, the watch page rows render, contract tests updated.

## 5. Integration (orchestrator)

Merge W1-W3 first (backend), then W4-W6. Wire the mini map into the watch
page, run the server and the dev UI from the worktree on ports 17610 and
17611 against a copy of the database, verify every surface in a browser,
polish visuals, and record leftovers in FOLLOWUPS.md.
