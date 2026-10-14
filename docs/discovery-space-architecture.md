# Discovery Space architecture (hand-maintained; updated 2026-10-14)

The seed-branch discovery view ("Sound Space", frontend route /discoverspace)
is built from three backend layers plus a store-driven Svelte UI. Keep this
section current when touching any of them.

### Request flow

1. POST /api/discovery/space (noor-server/src/server/routes/discovery_space_routes.rs::get_discovery_space)
   - Candidate generation: services/radio.rs::orchestrate_song blending
     library embedding neighbors (track_neighbors table), Last.fm similar
     (6h cache), and engine candidates. The request's coherence (0..1,
     default 0.5) picks the RadioBlend band: >=0.67 Familiar, >=0.33 Mixed,
     else Adventurous. With fewer than 8 library candidates (a seed added
     since the last training run has no neighbors) the route adds
     radio::cold_seed_library_candidates: same artist, nearest DSP sound
     (capped at half) and genre, round-robin. Sound Space only; radio keeps
     its own TIDAL-mix rescue for thin seeds.
   - Untagged candidate reasons fall back by source (library/engine ->
     "similar", Last.fm/TIDAL -> "external"); metadata_similarity and
     audio_texture normalize to "similar", not "unknown".
   - Batched enrichment (DSP, listen stats, genres, cohorts), then artist
     genres for tracks with none (services/artist_genres.rs: the artist's
     library genres, video_artist_facets when that table exists, TIDAL
     artist genres; free tags resolved to canonical names), then the
     multi-signal re-rank in services/discovery_ranking.rs:
     shaped = base * m_genre * m_harmonic * m_energy * m_artist * m_taste.
     Every multiplier is exactly 1.0 on missing data; at coherence 0.5 with
     no signals shaped == base bit-for-bit (test-asserted).
   - Filters (SpaceFilters: bpm/energy ranges, key_compatible_only, era via
     albums.year, exclude_in_library, exclude_heard_session via
     listen_history.session_id) run post-enrichment, pre-prune, seed exempt.
     Missing signals PASS, except key_compatible_only which drops
     unanalyzed candidates by design.
   - normalize_scores_by_source + prune_graph (services/discovery_space.rs;
     PruneConfig::for_coherence scales hub suppression). A hub needs at least
     hub_min_in_degree (3) in-edges from other candidates (seed edges are
     excluded) and in-degree percentiles are mid-rank, so a sparse star map
     is never read as all hubs. Node radius = confidence. Then nodes serialize
     with why / why_signals / shaped_score and diagnostics gain coherence,
     filter_dropped_count, era_filter_coverage.
2. Blend endpoints (/api/discovery/blend/{space,add,play,radio}) share the
   same shaping via build_discovery_blend_space: seed features are the union
   of resolved anchors, the same-artist boost is off, and coherence scales
   the library-guide cap (identity at 0.5).
3. POST /api/discovery/feedback (allowlist dismiss|like|skip) records rows;
   POST /api/discovery/rerank rebuilds a session TasteVector
   (discovery_ranking::build_session_taste, canonical smart/taste_vector.rs
   type) from the last 50 rows and re-shapes the client's current candidate
   list statelessly. Clients send rawScore as base_score so shaping is not
   applied twice.
4. POST /api/discovery/space/queue queues/plays the client's ranked list
   through the same pending-queue pipeline as blends (nullable track_id,
   lazy resolve at play time).

### Scoring module contract (services/discovery_ranking.rs)

Pure functions over pre-fetched data; no DB access. Reuses
genre/jaccard.rs::weighted_jaccard and
services/audio_analysis::compute_harmonic_multiplier verbatim (harmonic tamed
by pow(alpha) + clamp [0.70, 1.40]). Why-related phrases derive only from
multipliers that actually fired, priority key+bpm > genre > artist > energy >
source fallback, max two phrases. Signal keys are stable API: key_bpm, key,
bpm, genre_strong, genre, artist, energy, embedding, lastfm, bridge.

### Signal availability (do not assume more)

audio_dsp_features: TIDAL library tracks only (about 9% of the library).
track_genres: about two thirds of the library; externals get none of their
own (external_track_candidates.genre_tags_json is empty in practice) and
borrow artist genres via services/artist_genres.rs.
track_neighbors: post-training. track_similarity: stale, NOT a ranking input.
Ranking code must degrade to neutral on absent data, never crash or bury.

### Frontend state model

frontend/src/lib/components/DiscoverSpace/discover_space_store.ts holds
coherence, filters, sessionId (persisted in sessionStorage keys
discoverspace.controls.v1 / session.v1, hydrated in +page.svelte onMount
before the first load). All space/blend requests spread
controlRequestFields(), so the WebSocket-driven background reload
(handleDiscoverySpaceRefreshed) inherits the user's controls. Contract tests
in frontend/scripts/discovery-space-contract.test.mjs guard this wiring; the
Rust route tests cover request defaults and shaping output.

### Visual encoding (frontend)

Layout lives in discover_space_physics.ts: distance from the anchor is rank
by the list's own sort (rerankScore > shapedScore > score), with rings at the
rank quartiles; the angle is a genre-family sector (discover_space_genre.ts,
untagged last); size is confidence. The simulation only eases nodes to those
targets and resolves overlaps; edges do not pull. Missing lens data draws
UNKNOWN_FILL grey, never a default value. Colour tables (EDGE_COLORS,
SOURCE_RING, energyColor) live in the renderer; the legend and help import
them. Labels are greedy and collision-free, by rank. Right-click on a star,
row or the card opens the shared track menu (discover_space_menu.ts).

### Branch state and ranked list (frontend)

branchHere(node) = lock node as seed + push the previous seed onto
branchPath (walkBack truncates; re-branching to a seed already in the path
truncates instead of looping). branchPath + lockedSeedId persist in
sessionStorage discoverspace.branch.v1 and rehydrate on mount, restoring
the tree position. DiscoverRankedList sorts by rerankScore > shapedScore >
score; like/skip posts feedback then /api/discovery/rerank and merges
scores/why without touching canvas positions (rerank sends rawScore as
base_score). Play all / Queue all go through /api/discovery/space/queue.
