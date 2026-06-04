# Playback queue inventory

Every code path that puts tracks into the NOORwave playback queue, end to
end. Captured 2026-04-30 ahead of a radio-quality regression diagnosis;
the goal is to see at a glance which paths share which infrastructure
and where Phase 1 / Phase 2a work actually lands.

No fixes proposed here. Pure reference document.

## Headline picture

There are **three orthogonal playback surfaces** the user can drive into:

1. **Library queue** — the `queue` table, mutated through
   `playback/queue.rs`'s API. Survives restarts. This is what every "Play
   X / Add Y / Shuffle Z / Radio R" action ultimately writes to.
2. **Ephemeral Tidal playback** — `AppState.ephemeral_tidal_track` plus
   `external_playback_track`, used when the user plays a Tidal-only track
   that isn't in their library. Does **not** touch the `queue` table; the
   ephemeral track is overlayed on top of `current_track` reads. Cleared
   on next library queue interaction.
3. **Discovery feed surface** — `Discovery*` JSON responses surfaced in
   the Discover Space and Discover Panel UIs. **No queue write path**
   from discovery exists; the user must explicitly save a discovery
   candidate (which imports it to the library, then takes the standard
   library queue path).

Phase 1 (commits `adf0739`, `e6d9dcc`) only altered automix scoring.
Phase 2a (commits `782c408`, `7e7f6d9`) only altered radio. Neither phase
touched single-track / album / artist / shuffle play, queue mutation
endpoints, or ephemeral Tidal playback.

## Queue mutation surface

Public functions in [noor-server/src/playback/queue.rs](../noor-server/src/playback/queue.rs):

| Function (line) | Effect |
|---|---|
| `append_tracks` (queue.rs:77) | Push tracks to the end with a `source` tag (e.g. `"playback"`, `"automix"`, `"test"`). |
| `replace_queue` (queue.rs:94) | Wipe and replace; preserves nothing. |
| `clear_queue` (queue.rs:99) | Wipe rows; current_track_id stays set. |
| `remove_queue_item` (queue.rs:104) | Remove one item by queue_id. |
| `move_queue_item` (queue.rs:114) | Reorder one item to a new position. |
| `apply_shuffle_with_seed` (queue.rs) | In-place shuffle of the current queue with persisted debug seed metadata. |

Every code path below resolves to one of these calls.

## Path-by-path inventory

Each section uses the same eight-field shape: entry point, orchestrator,
candidate sources, scoring, filtering, sequencing, tests, phase touch.

### 1. Play (single track from track row click)

| Field | Detail |
|---|---|
| Entry point | Track row click in [TrackRow.svelte:103](../frontend/src/lib/components/TrackRow.svelte#L103). Dispatches to a parent `play` handler. |
| Frontend → API | [stores/player.ts:249](../frontend/src/lib/stores/player.ts#L249) (`playTrackNow`) → `POST /api/playback/play` |
| Backend handler | `play_track` ([routes.rs:4493](../noor-server/src/server/routes.rs#L4493)) |
| Orchestrator | None. Streams via `playback::runtime` directly; **does not write to the queue table**. The displayed "now playing" comes from `playback_state.current_track_id` getting set. |
| Candidate sources | Single library lookup by id. |
| Scoring | None. |
| Filtering | None. |
| Sequencing | N/A (single track). |
| Tests | None. |
| Phase touch | Untouched. |

**Note**: Single-track play does not enqueue. To get into the queue table the user has to use Add to Queue, Play Album, etc.

### 2. Add to queue (single track)

| Field | Detail |
|---|---|
| Entry point | Add-to-queue button on [TrackRow.svelte:75](../frontend/src/lib/components/TrackRow.svelte#L75) and equivalent positions. Context menu item via [track_menu.ts](../frontend/src/lib/player/track_menu.ts). |
| Frontend → API | `addTrackToQueue` ([player.ts:447](../frontend/src/lib/stores/player.ts#L447)) → `POST /api/playback/queue/add` |
| Backend handler | `add_queue_track` ([routes.rs:5298](../noor-server/src/server/routes.rs#L5298)) → `player::enqueue_track` ([player.rs:229](../noor-server/src/playback/player.rs#L229)) |
| Queue mutation | `queue::append_tracks` |
| Candidate sources | Single library lookup by id. |
| Scoring | None. |
| Filtering | None. |
| Sequencing | Appended to end. |
| Tests | None. |
| Phase touch | Untouched. |

### 3. Play next (insert after current)

| Field | Detail |
|---|---|
| Entry point | Context-menu item, [track_menu.ts:59-61](../frontend/src/lib/player/track_menu.ts#L59-L61). |
| Frontend → API | `playTrackNext` ([player.ts:838](../frontend/src/lib/stores/player.ts#L838)) — appends via `/queue/add` then immediately reorders via `/queue/move`. |
| Backend handler | Two sequential calls: `add_queue_track` then `move_queue_track` ([routes.rs:5344](../noor-server/src/server/routes.rs#L5344)). |
| Queue mutation | `queue::append_tracks` then `queue::move_queue_item`. |
| Sources / Scoring / Filtering | None. |
| Sequencing | Inserted directly after the current track via the move. |
| Tests | None. |
| Phase touch | Untouched. |

### 4. Play album

| Field | Detail |
|---|---|
| Entry point | Album page hero button at [albums/[id]/+page.svelte:126-131](../frontend/src/routes/albums/[id]/+page.svelte#L126); context-menu via [track_menu.ts:80-82](../frontend/src/lib/player/track_menu.ts#L80-L82). |
| Frontend → API | `playAlbum` ([player.ts:675](../frontend/src/lib/stores/player.ts#L675)) — `GET /api/albums/{id}/tracks`, then `POST /api/playback/queue` (replace), then `POST /api/playback/play` for the start track. |
| Backend handler | `replace_playback_queue` ([routes.rs:5313](../noor-server/src/server/routes.rs#L5313)) → `player::replace_queue_with_tracks` ([player.rs:235](../noor-server/src/playback/player.rs#L235)). |
| Queue mutation | `queue::replace_queue`. |
| Candidate sources | Library DB direct lookup of the album's tracks (via `GET /api/albums/{id}/tracks`). No recommendation. |
| Scoring | None. |
| Filtering | None. |
| Sequencing | Disc + track number order from the album tracks query (set in `tracks` table). |
| Tests | None. |
| Phase touch | Untouched. |

### 5. Shuffle album

| Field | Detail |
|---|---|
| Entry point | Album page button + [track_menu.ts:85-87](../frontend/src/lib/player/track_menu.ts#L85-L87). |
| Frontend → API | `shuffleAlbum` ([player.ts:696](../frontend/src/lib/stores/player.ts#L696)) — same shape as Play Album but with frontend Fisher-Yates of the track id list before sending. |
| Backend handler | `replace_playback_queue` (same as Play Album). |
| Sequencing | Whatever order the frontend sends. |
| Tests | None. |
| Phase touch | Untouched. |

### 6. Play artist

| Field | Detail |
|---|---|
| Entry point | Artist page hero button at [artists/[id]/+page.svelte:160](../frontend/src/routes/artists/[id]/+page.svelte#L160) and row click at line 318. |
| Frontend → API | `playArtist` ([player.ts:712](../frontend/src/lib/stores/player.ts#L712)) — `GET /api/artists/{id}/tracks` then replace + play. |
| Backend handler | `replace_playback_queue` (same shared handler as Play Album). |
| Candidate sources | Library DB direct lookup of all artist tracks. |
| Sequencing | Server-side ordering from the artist-tracks query (typically by album then track number). |
| Tests | None. |
| Phase touch | Untouched. |

### 7. Shuffle artist

| Field | Detail |
|---|---|
| Entry point | Artist page button at [artists/[id]/+page.svelte:272](../frontend/src/routes/artists/[id]/+page.svelte#L272). |
| Frontend → API | `shuffleArtist` ([player.ts:733](../frontend/src/lib/stores/player.ts#L733)) — same as Play Artist with frontend shuffle. |
| Phase touch | Untouched. |

### 8. Song radio (right-click "Start from this song")

| Field | Detail |
|---|---|
| Entry point | Context-menu via [track_menu.ts:72-75](../frontend/src/lib/player/track_menu.ts#L72-L75). |
| Frontend → API | `startSongRadio` ([player.ts:748](../frontend/src/lib/stores/player.ts#L748)) — `POST /api/radio/song`, then `POST /api/playback/queue` to write the result, then `POST /api/playback/play`. |
| Backend handler | `radio_song` ([routes.rs:2819](../noor-server/src/server/routes.rs#L2819)) → `services::radio::orchestrate_song` ([radio.rs:92](../noor-server/src/services/radio.rs#L92)). |
| Queue mutation | The orchestrator returns a `RadioQueue` JSON; the **frontend** then calls `replace_playback_queue` with the result. The radio orchestrator does not touch the queue table directly. |
| Candidate sources | **Three**, in the order they're built: Library via `services::learning::radio_from_neighbors` ([radio.rs:139](../noor-server/src/services/radio.rs#L139)) — embedding-model neighbours; Last.fm via `LastFmClient.track_get_similar` ([radio.rs:172](../noor-server/src/services/radio.rs#L172)); Engine via `engine_results_from_track_similarity` ([radio.rs:399](../noor-server/src/services/radio.rs#L399)) reading `track_similarity` table — **filled in Phase 2a Stage 2 (`7e7f6d9`)**. |
| Scoring | Each source produces its own native `similarity_score` ∈ `[0, 1]`. Library scores are post-multiplied by a creativity factor (Familiar 0.85x, Mixed 0.70x, Adventurous 0.50x via `1 − creativity*0.35`). After dedup, `apply_taste_signals` ([radio.rs:591](../noor-server/src/services/radio.rs#L591)) multiplies score by `1.0 + (pos*0.05) − (neg*0.07)` per resolved artist_id from `ArtistResolver`. |
| Filtering | Hard suppression: drop candidates whose `track_id` (library candidates only — last.fm hits have track_id=0) is in `taste.skipped_track_ids`. Dedup: `combine_with_dedup` groups by normalised `(artist, title)`, library wins iff its score is ≥95% of the best non-library score, otherwise highest score; tie-break by source priority (Library > Engine > Lastfm). Caller-supplied `exclude_track_ids`. |
| Sequencing | `blend_interleave` ([radio.rs:612](../noor-server/src/services/radio.rs#L612)) sorts each source bucket by score desc, computes per-source quotas from the blend weights `(library, lastfm, engine)`, then picks per slot by which source is "most behind" its quota. |
| Tests | `radio_phase2_tests` in [radio.rs:761+](../noor-server/src/services/radio.rs#L761): 14 tests covering dedup tie-break and `apply_taste_signals` (Stage 1), plus 5 covering engine slot output and the before/after diff (Stage 2). No end-to-end orchestrator test; the embedding model fixture would be substantial. |
| Phase touch | **Directly modified by Phase 2a, both stages.** |

### 9. Album radio

| Field | Detail |
|---|---|
| Entry point | Frontend `startAlbumRadio` ([player.ts:818](../frontend/src/lib/stores/player.ts#L818)). |
| Backend handler | `radio_album` ([routes.rs:2861](../noor-server/src/server/routes.rs#L2861)) → `services::radio::orchestrate_album` ([radio.rs:223](../noor-server/src/services/radio.rs#L223)). |
| Candidate sources | Picks the first 3 album tracks by disc/track number, calls `orchestrate_song` per seed, unions the results. Each per-seed call goes through the full library + lastfm + engine pipeline above. |
| Scoring / Filtering | Inherited from `orchestrate_song` per-seed, then a second `combine_with_dedup` + `blend_interleave` pass over the union. **Note**: the union pass dedups but does not run `apply_taste_signals` again — that ran already per-seed. **Note 2**: taste profile is rebuilt 3× (once per `orchestrate_song`) — wasteful but correct. Phase 2b candidate. |
| Tests | None at the album orchestrator level. |
| Phase touch | Directly modified by Phase 2a (transitively via `orchestrate_song`). |

### 10. Artist radio

| Field | Detail |
|---|---|
| Entry point | Context-menu via [track_menu.ts:92-96](../frontend/src/lib/player/track_menu.ts#L92-L96). |
| Frontend → API | `startArtistRadio` ([player.ts:798](../frontend/src/lib/stores/player.ts#L798)) → `POST /api/radio/artist`. |
| Backend handler | `radio_artist` ([routes.rs:2903](../noor-server/src/server/routes.rs#L2903)) → `services::radio::orchestrate_artist` ([radio.rs:281](../noor-server/src/services/radio.rs#L281)). |
| Candidate sources | Picks artist's top-3 tracks by `play_count` desc then `last_played_at` desc, calls `orchestrate_song` per seed, unions. Same shape as Album Radio but with a different seed-selection rule. |
| Scoring / Filtering | Inherited from `orchestrate_song`. |
| Tests | None at the artist orchestrator level. |
| Phase touch | Directly modified by Phase 2a (transitively). |

### 11. Automix queue extension (silent, fires on next/peek)

| Field | Detail |
|---|---|
| Entry point | Triggered by `/api/playback/next` (`next_track` handler) and `/api/playback/peek-next` (`peek_next_track`) when `automix_enabled = 1` and queue depth below `AUTOMIX_MIN_UPCOMING = 8`. |
| Backend handler | `player::next_track` / `peek_next_track` → `ensure_automix_queue_depth` ([player.rs:593](../noor-server/src/playback/player.rs#L593)) → `build_automix_extension` ([player.rs:640](../noor-server/src/playback/player.rs#L640)). |
| Queue mutation | `queue::append_tracks` with `source = "automix"`. |
| Candidate sources | **Two-tier**. (a) If an active embedding model exists AND `automix_use_learning = true`: `queries::get_track_neighbors` returns up to `needed × 4` neighbours (max 24), and that's the entire candidate set. (b) Otherwise the heuristic path runs `queries::get_similar_tracks` against `track_similarity` for up to `MAX_CANDIDATES = 500`, falling back to a 500-track random pool if the similarity result is too small. |
| Scoring | The embedding fast-path bypasses scoring entirely — neighbours are taken in their returned order. The heuristic path runs `automix_score` ([player.rs:976](../noor-server/src/playback/player.rs#L976)) which reads from `TasteVector` + `SeedContext` (built via `build_session_taste_profile` then `from_session_profile`). Multiplicative; coefficients verbatim from pre-Phase-1 source. |
| Filtering | Excludes everything currently in the queue plus `taste.recent_track_ids` (60-row window from listen_history). Hard suppression for `skipped_track_ids` (×0.1 score, not removal). |
| Sequencing | Embedding fast-path: returned order. Heuristic path: score desc, then declustered by album so consecutive tracks are not from the same album. |
| Tests | Two tests in `playback::player::tests`: `next_track_extends_queue_when_automix_is_enabled` (player.rs:1399), `peek_next_track_can_see_generated_automix_track` (player.rs:1419). **Both currently fail** because their in-memory test fixture lacks the `embedding_models` table, broken since commit `2ee0fbb` (267 commits ago). Plus `parity_tests::automix_score_parity_top_30` which gates score-function migration. |
| Phase touch | **Directly modified by Phase 1** (`adf0739`, `e6d9dcc`) — `automix_score`, `matches_preferred_genres`, `order_automix_candidates`, `build_automix_extension` all migrated to `TasteVector` + `SeedContext`. Phase 2a only bumped `build_session_taste_profile` to `pub(crate)` so radio could call it; no scoring change. |

### 12. Play playlist

| Field | Detail |
|---|---|
| Entry point | "Play all" button at [playlists/+page.svelte:660](../frontend/src/routes/playlists/+page.svelte#L660). |
| Frontend → API | `playPlaylist` ([playlists/+page.svelte:298-304](../frontend/src/routes/playlists/+page.svelte#L298-L304)) — `POST /api/playback/queue` (replace) then `POST /api/playback/play`. |
| Backend handler | `replace_playback_queue` (same shared handler as Play Album). |
| Candidate sources | Library DB lookup of playlist tracks (frontend already has the list from the playlist view). |
| Scoring / Filtering | None. |
| Sequencing | Playlist order as displayed. |
| Tests | None. |
| Phase touch | Untouched. |

### 13. Shuffle playlist

| Field | Detail |
|---|---|
| Entry point | Frontend `shufflePlaylist` ([player.ts:765](../frontend/src/lib/stores/player.ts#L765)) — frontend Fisher-Yates then replace. |
| Backend handler | `replace_playback_queue`. |
| Phase touch | Untouched. |

### 14. Start playlist radio

| Field | Detail |
|---|---|
| Entry point | Frontend `startPlaylistRadio` ([player.ts:774-779](../frontend/src/lib/stores/player.ts#L774-L779)). |
| Implementation | Picks one track from the playlist (the first) and delegates to `startSongRadio`. **Effectively a Song Radio with the playlist's first track as seed**; not a true playlist-aware radio. |
| Phase touch | Indirectly modified by Phase 2a via the Song Radio path. |

### 15. Tidal track / album / playlist play (ephemeral)

| Field | Detail |
|---|---|
| Entry point | Discovery space → `play` ([DiscoverPanel.svelte:142](../frontend/src/lib/components/Discover/DiscoverPanel.svelte#L142)); Tidal album/playlist play from search/discover pages. |
| Frontend → API | `playTidalTrackNow` ([player.ts:854](../frontend/src/lib/stores/player.ts#L854)) → `POST /api/tidal/play`. Album/playlist variants (`playTidalAlbum` at player.ts:900, `playTidalPlaylist` at player.ts:781) fetch tracks from `/api/tidal/albums/{id}/tracks` etc. and call `playTidalTrackNow` per track. |
| Backend handler | `play_tidal_ephemeral` ([routes.rs:5990](../noor-server/src/server/routes.rs#L5990)). |
| Queue mutation | **None.** Sets `AppState.ephemeral_tidal_track` and resolves a stream URL. The ephemeral track is overlayed on `current_track` reads ([routes.rs:3667](../noor-server/src/server/routes.rs#L3667)). |
| Candidate sources / Scoring / Filtering | Direct Tidal API; no recommendation, no scoring, no filtering. |
| Sequencing | Single track per call — album/playlist variants drive sequencing from the frontend by calling repeatedly. |
| Tests | None. |
| Phase touch | Untouched. |

### 16. Discovery → playback (no direct queue path)

| Field | Detail |
|---|---|
| Entry point | Discover Space candidate click → `play` ([DiscoverPanel.svelte:51-68](../frontend/src/lib/components/Discover/DiscoverPanel.svelte#L51-L68)); Discover prompt-driven results. |
| Backend handler | `play_discovery_track` ([routes.rs:1439](../noor-server/src/server/routes.rs#L1439)). |
| Queue mutation | None — uses ephemeral Tidal playback (sets `external_playback_track`). |
| Save-then-queue | If the user explicitly saves a discovery candidate via `save_discovery_track` ([routes.rs:1401](../noor-server/src/server/routes.rs#L1401)), the track is imported to the library; **then** any subsequent normal Add to Queue / Play / Play Album action takes the standard library queue path. |
| Candidate generation | `smart::discovery::build_preview` (library candidates) and `smart::external_discovery::build_external_feed` (Tidal candidates), with optional embedding overlay via `services::learning::build_prompt_preview`. None of this output writes to the queue. |
| Phase touch | Untouched in both phases. |

### 17. Discover Space → Add to queue (Tidal external)

| Field | Detail |
|---|---|
| Entry point | "Add to queue" button on a Discover Panel candidate at [DiscoverPanel.svelte:104](../frontend/src/lib/components/Discover/DiscoverPanel.svelte#L104). |
| Frontend → API | `addTidalTrackToQueue` ([player.ts:896](../frontend/src/lib/stores/player.ts#L896)). **Currently a stub that calls `playTidalTrackNow`** — so it actually plays now via ephemeral path, not enqueues. UI affordance is misleading. |
| Phase touch | Untouched. |

### 18. Drag-and-drop queue reorder, remove from queue, clear queue

| Field | Detail |
|---|---|
| Entry points | Drag-drop in queue panel at [+layout.svelte:1154-1157](../frontend/src/routes/+layout.svelte#L1154-L1157); per-row remove via [track_menu.ts:124-134](../frontend/src/lib/player/track_menu.ts#L124-L134); clear via [track_menu.ts:136-141](../frontend/src/lib/player/track_menu.ts#L136-L141). |
| Backend handler | `move_queue_track` / `remove_queue_track` / `clear_queue_route`. |
| Phase touch | Untouched. |

## Shared primitives

The infrastructure that multiple paths depend on. Cross-references for
"if I change X, who notices".

### track_similarity table

Read sites:
- `services::radio::engine_results_from_track_similarity` ([radio.rs:399](../noor-server/src/services/radio.rs#L399)) — Song / Album / Artist Radio's engine slot. **Phase 2a Stage 2 added this caller.**
- `playback::player::build_automix_extension` ([player.rs:691](../noor-server/src/playback/player.rs#L691)) — heuristic-path candidate source (when no embedding model OR no neighbours).
- `server::routes::preview_discovery` ([routes.rs:1283](../noor-server/src/server/routes.rs#L1283)) — read-only Discover UI preview.
- `server::routes::preview_discovery_search` ([routes.rs:1982](../noor-server/src/server/routes.rs#L1982)) — same.

Write site:
- `queries::compute_track_similarity` ([queries.rs:2288](../noor-server/src/db/queries.rs#L2288)) — manual trigger via `POST /api/discovery/compute-similarity` ([routes.rs:2077](../noor-server/src/server/routes.rs#L2077)). **Not on a schedule.** If the table is stale or sparse, both Engine Radio and the automix heuristic fallback see thin candidates.

Phase impact: Phase 1 untouched. Phase 2a added the radio caller.

### Embedding model neighbours

Read sites:
- `playback::player::build_automix_extension` ([player.rs:654](../noor-server/src/playback/player.rs#L654)) — embedding fast-path, primary candidate source for automix.
- `services::learning::radio_from_neighbors` ([learning.rs:267](../noor-server/src/services/learning.rs#L267)) — Library source for Song / Album / Artist Radio. **The dominant library recall path for radio.**
- `services::learning::query_candidate_neighborhood` ([learning.rs:411](../noor-server/src/services/learning.rs#L411)) — discovery prompt embedding overlay.

Build / refresh:
- `services::learning::replace_track_embeddings` ([learning.rs:171](../noor-server/src/services/learning.rs#L171)) on training run completion.
- `services::learning::load_active_learning_model` ([learning.rs:237](../noor-server/src/services/learning.rs#L237)) hydrates vectors from disk on first call per process.

**Failure mode**: if no active embedding model exists, `radio_from_neighbors` returns `Ok(None)` and radio's library source is empty. The Stage 1 radio fixture exposed this — radio with no embedding model + None lastfm + empty engine produces an empty queue. Also relevant: the radio library source is the only place creativity is applied (`creativity *= 0.35`); with no embedding model, the creativity blend setting effectively disappears.

Phase impact: Both phases preserved this primitive unchanged. Phase 2a's library-source logic continued to call `radio_from_neighbors` exactly as before.

### Last.fm API

Read sites:
- `services::radio::orchestrate_song` ([radio.rs:172](../noor-server/src/services/radio.rs#L172)) — Last.fm source for radio.
- Discovery search query augmentation ([routes.rs:3430](../noor-server/src/server/routes.rs#L3430), [routes.rs:3459](../noor-server/src/server/routes.rs#L3459)).

Construction: `LastFmClient::load(http_client, &db)` is called per-request inside the radio routes. Tries DB-stored API key first, falls back to `LASTFM_API_KEY` env var. Returns `None` on missing key, in which case radio's Last.fm source is empty.

Phase impact: Both phases preserved unchanged.

### TasteVector / SessionTasteProfile

Construction sites:
- `services::radio::build_taste_inputs` ([radio.rs:343](../noor-server/src/services/radio.rs#L343)) — Phase 2a addition. Loads seed track, builds `SessionTasteProfile`, converts via `from_session_profile`, also loads `ArtistResolver`. On any DB error returns empty defaults and logs.
- `playback::player::build_automix_extension` ([player.rs:677-685](../noor-server/src/playback/player.rs#L677-L685)) — same conversion, used inside automix.

Consumption (scoring) sites:
- `playback::player::automix_score` ([player.rs:976](../noor-server/src/playback/player.rs#L976)) — multiplicative, automix coefficients.
- `services::radio::apply_taste_signals` ([radio.rs:591](../noor-server/src/services/radio.rs#L591)) — multiplicative artist nudge with much smaller coefficients (0.05/0.07 vs 0.5/0.65) because radio scores are bounded `[0, 1]`.

Phase impact: Phase 1 introduced `TasteVector` and migrated automix. Phase 2a added the radio consumer and the resolver-driven artist lookup.

### ArtistResolver

Single load site: `build_taste_inputs` ([radio.rs:352](../noor-server/src/services/radio.rs#L352)). Single lookup site: `apply_taste_signals` ([radio.rs:601-611](../noor-server/src/services/radio.rs#L601-L611)).

Phase 2a introduced; nothing else uses it yet (`from_taste_mesh` and `from_analytics_overview` adapters defined in Phase 1 are unused placeholders).

### Tidal API for direct playback

`play_tidal_ephemeral` and `tidal_search` paths produce `Track` rows that are NOT in the library. They never reach `queue::*` mutations — they live in `AppState.ephemeral_tidal_track` and `external_playback_track`, overlayed on `current_track` reads. To get a Tidal track into the queue the user must explicitly save it (which imports to library), then re-trigger any standard play action.

## Test coverage matrix

| Path | Test file | Status |
|---|---|---|
| Add to queue | — | None |
| Play next | — | None |
| Play / Shuffle album | — | None |
| Play / Shuffle artist | — | None |
| Single-track play | — | None |
| Song / Album / Artist Radio (component-level) | services/radio.rs `radio_phase2_tests` | 19 tests, all pass |
| Song / Album / Artist Radio (orchestrator end-to-end) | — | None |
| Automix scoring | playback/player.rs `parity_tests` | 1 test, passes |
| Automix queue extension (orchestrator) | playback/player.rs `tests::next_track_*`, `peek_next_track_*` | 2 tests, **both fail** since `2ee0fbb` (missing `embedding_models` table in fixture) |
| Discovery save / play | — | None |
| Tidal ephemeral playback | — | None |
| Queue mutation routes (add/remove/move/clear/replace) | — | None |
| Playlist play / shuffle / radio | — | None |
| Drag-drop reorder | — | None |

**Bottom line**: scoring functions and dedup logic have unit tests. Every orchestrator-level path operates without an end-to-end test.

## Phase 1 / Phase 2a touch matrix

| Path | Phase 1 | Phase 2a |
|---|---|---|
| Single-track play | — | — |
| Add to queue | — | — |
| Play next | — | — |
| Play / Shuffle album | — | — |
| Play / Shuffle artist | — | — |
| Song Radio | — | **Direct** (TasteVector + dedup fix + engine slot) |
| Album Radio | — | **Direct** (transitively via Song Radio) |
| Artist Radio | — | **Direct** (transitively) |
| Automix queue extension | **Direct** (TasteVector + scoring rewrite) | Indirect (visibility bump only) |
| Playlist play / shuffle | — | — |
| Playlist radio | — | Indirect (delegates to Song Radio) |
| Tidal ephemeral playback | — | — |
| Discovery → save → play | — | — |
| Queue mutation routes | — | — |

## Cross-cutting observations

These are observational, not prescriptive — listed because they jump out of the inventory.

1. **Radio's library source is single-recall (embedding model only).** Automix has a two-tier source path (embedding fast-path → track_similarity heuristic → random pool). Radio's library source is `radio_from_neighbors` only, which returns empty if no active embedding model exists. Phase 2a added the engine slot (track_similarity) as a separate radio source, but it's not a library-source fallback — it goes into a different blend bucket.

2. **`apply_taste_signals` runs on the post-dedup combined list.** Library candidates that lost the dedup (because some other source had a higher score and library was below the 5% threshold) never get the artist-affinity nudge applied — only the survivors do. Whether this is intended is worth checking; the previous "library wins everything" behaviour meant library candidates always survived dedup and always got scored.

3. **Album / Artist Radio rebuild taste 3x.** Each per-seed `orchestrate_song` call inside the album/artist orchestrator runs `build_taste_inputs` independently, producing the same TasteVector and ArtistResolver three times. Wasteful but correct. Phase 2b candidate.

4. **The radio frontend does its own queue replace.** `startSongRadio` and friends call `/api/radio/song`, get a JSON queue back, then call `/api/playback/queue` (replace) themselves. The radio orchestrator never touches the queue table. This means a successful radio request can still result in an empty queue if the frontend's follow-up replace fails (silently or otherwise).

5. **Automix's embedding fast-path bypasses TasteVector entirely.** When an active embedding model exists, `build_automix_extension` returns `get_track_neighbors` results directly without scoring. `skipped_track_ids` hard suppression and artist/genre affinity from `TasteVector` are NOT applied on the embedding fast-path. Heuristic-path candidates do get scored via `automix_score`. Asymmetric.

6. **Discovery → queue requires a library import.** There is no path from Discovery results into the playback queue without first calling `save_discovery_track` (which imports to library). The "Play" button on a discovery candidate uses ephemeral Tidal playback, not the queue.

7. **Tests are concentrated at the function-unit boundary.** Dedup, scoring, and resolver have unit tests. Every orchestrator and every queue mutation route has zero coverage. The two automix orchestrator tests that exist have been broken for 267 commits.

8. **`compute_track_similarity` is manually triggered.** No scheduled job, no startup recompute. If the table is stale, Phase 2a's engine slot returns thin or empty results and Song Radio leans more heavily on Last.fm + the embedding model.
