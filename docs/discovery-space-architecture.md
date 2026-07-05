# Discovery Space architecture (hand-maintained; updated 2026-07-05)

The seed-branch discovery view ("Sound Space", frontend route /discoverspace)
is built from three backend layers plus a store-driven Svelte UI. Keep this
section current when touching any of them.

### Request flow

1. POST /api/discovery/space (noor-server/src/server/routes/discovery_space_routes.rs::get_discovery_space)
   - Candidate generation: services/radio.rs::orchestrate_song blending
     library embedding neighbors (track_neighbors table), Last.fm similar
     (6h cache), and engine candidates. The request's coherence (0..1,
     default 0.5) picks the RadioBlend band: >=0.67 Familiar, >=0.33 Mixed,
     else Adventurous.
   - Batched enrichment (DSP, listen stats, genres, cohorts), then the
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
     PruneConfig::for_coherence scales hub suppression), then nodes serialize
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

audio_dsp_features: TIDAL library tracks only. track_genres: favorites-heavy
coverage; externals only have external_track_candidates.genre_tags_json.
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
