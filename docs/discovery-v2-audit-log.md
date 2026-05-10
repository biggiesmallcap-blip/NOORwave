# Discovery V2 Audit Log

## Phase 1 Core Trainer V2

Files changed:
- `noor-server/src/db/queries.rs`
- `noor-server/src/services/discovery_trainer.rs`
- `noor-server/src/services/learning.rs`

Behavior added:
- Added expanded DSP fields and metadata tokens for danceability, beat strength, and LUFS buckets.
- Bumped audio proxy feature version to `metadata-audio-proxy-v2`.
- Rejected cached audio proxy rows unless both vector dimension and feature version match.
- Created isolated V2 model rows with `family = "discovery-fusion-v2"` and run-scoped keys.
- Added typed evidence groups, event-level heldout examples, support buckets, and one-way direct transition handling.
- Added completion-weighted listen-history co-listen evidence.
- Added direct `transition_from_track_id -> track_id` listen-history evidence.
- Added direct playback-transition evidence with weight 2.0 through the evidence builder.
- Added directional transition score bonus capped at `0.18`.

Tests run:
- `cargo test -p noor-server discovery_trainer`
- `cargo test -p noor-server cached_audio_features_reject_old_feature_version`
- `cargo test -p noor-server create_embedding_model_inserts_run_scoped_rows_without_overwriting_active_model`
- `cargo test -p noor-server completion_weighted_listen_edges_downweight_skipped_tracks`
- `cargo test -p noor-server listen_history_transition_edges_preserve_source_and_completion_weight`

Audit findings:
- Direct transition support is one-way. Reverse edges do not inherit the direct-transition bonus.
- Bidirectional co-listen evidence still creates both directions.
- Heldout exclusion is event-level and leaves unrelated evidence for the same tracks in training.
- Feature cache rejects `metadata-audio-proxy-v1` rows.
- Model creation is isolated and does not overwrite active V1 or active V2 rows before activation.

Known follow-up items:
- External sidecar schema and automix integration are tracked in later phases.

Sign-off:
- Phase 1 implementation did not touch playback runtime, gapless, or audio runtime files.
- No past migration was edited.
- No bare `cargo update` was run.
- New SQL uses parameters for values.
- Rollback remains possible because V2 rows are isolated until activation.

## Phase 2 Support Persistence And Evaluation Gates

Files changed:
- `noor-server/src/db/schema.rs`
- `noor-server/src/db/queries.rs`
- `noor-server/src/services/discovery_trainer.rs`
- `noor-server/src/services/learning.rs`
- `noor-server/src/services/neighbor_refresh.rs`

Behavior added:
- Appended `MIGRATION_037` support columns to `track_neighbors`.
- Added support breakdown fields to `TrainerNeighbor`, `NeighborWriteRow`, and `EmbeddingNeighborRow`.
- Persisted support breakdowns in full graph replacement and seed-scoped replacement.
- Read support breakdowns from `get_track_neighbors`.
- Added by-key and by-family embedding model lookup helpers.
- Added bulk neighbor loading by model id and seed list.
- Added rollback helper that reactivates a prior ready model row without deleting newer rows.
- Counted typed evidence rows in the activation gate as real play signal.
- Added typed heldout metrics for direct transition evidence.
- Added diagnostics for skip-aware recall, skipped input rows, cold-track recall, and discovery lift.
- Added same-heldout active-model baseline evaluation from stored neighbor rows.
- Activation now requires no transition-recall regression against the active stored baseline when one exists.

Tests run:
- `cargo test -p noor-server neighbor_support_breakdown_round_trips_through_full_and_seed_replacement`
- `cargo test -p noor-server embedding_model_lookup_and_rollback_keep_rows_intact`
- `cargo test -p noor-server bulk_neighbor_loading_groups_by_seed_and_preserves_support_columns`
- `cargo test -p noor-server typed_heldout_metrics_are_labeled_by_evidence_kind`
- `cargo test -p noor-server typed_diagnostics_include_skip_aware_and_cold_track_recall`
- `cargo test -p noor-server discovery_lift_counts_low_play_neighbor_share`
- `cargo test -p noor-server stored_neighbor_baseline`

Audit findings:
- Support columns are written by both replacement paths.
- Scalar `support_count` remains a rounded integer support value.
- Rollback reactivates a ready prior model row and leaves newer V2 rows intact.
- Bulk neighbor loading reads stored rows for the requested model id and seed list, not metrics JSON.
- Same-heldout baseline comparison uses stored neighbor rows for the active model.
- Typed transition metrics are labeled separately from unlabeled aggregate recall.
- Skip-aware recall weights session co-listen heldout examples by completion weight.
- Cold-track recall is based on heldout targets with play count <= 1.
- Discovery lift reports the top-10 share of low-play and never-played neighbors.

Known follow-up items:
- External source-specific diagnostics are tracked in later phases.

Sign-off:
- Phase 2 changes are append-only for schema.
- No playback runtime, gapless, or audio runtime files were touched.
- No bare `cargo update` was run.
- New SQL uses parameterized values.
- Active model rollback remains possible after this phase.

## Phase 3 External Candidate Sidecar

Files changed:
- `noor-server/src/db/schema.rs`
- `noor-server/src/db/queries.rs`

Behavior added:
- Added sidecar tables for external candidates, sightings, audio features, embeddings, and library-to-external neighbors.
- Added non-null unique `dedupe_key` for unresolved fallback dedupe.
- Added partial unique indexes for `tidal_id` and `mbid`.
- Added explicit normalized artist/title/duration fallback identity columns and uniqueness for unresolved candidates.
- Added expiry indexes, sighting uniqueness, library/model/rank neighbor index, and cascade FKs.
- Added candidate upsert, sighting upsert, feature replacement, embedding replacement, neighbor replacement, transactional prune, and transactional merge.
- Added eligible-candidate loading for trainer input, excluding expired and resolved rows.
- Added trainer-side external audio-proxy scoring that emits library seed to external candidate neighbors.
- Added persistence of trainer external neighbors to the sidecar neighbor table.
- Added external-neighbor lookup that returns only TIDAL-resolved candidates by default.

Tests run:
- `cargo test -p noor-server external_candidate_upsert_dedupes_unresolved_rows`
- `cargo test -p noor-server external_candidate_upsert_dedupes_unresolved_rows_by_normalized_identity`
- `cargo test -p noor-server external_sightings_and_neighbors_replace_without_stale_rows`
- `cargo test -p noor-server external_features_embeddings_and_expired_rows_prune_transactionally`
- `cargo test -p noor-server external_candidate_merge_moves_sidecar_rows_before_deleting_loser`
- `cargo test -p noor-server external_candidates_emit_sidecar_neighbors_without_library_rows`
- `cargo test -p noor-server external_candidates_for_training_skip_expired_and_resolved_rows`
- `cargo test -p noor-server external_neighbor_lookup_returns_only_tidal_resolved_candidates_by_default`

Audit findings:
- Unresolved candidates cannot duplicate when `tidal_id` and `mbid` are null because `dedupe_key` is non-null and normalized fallback identity is unique.
- Sighting upsert is unique by candidate, seed track, and source.
- Library-to-external neighbor replacement removes stale rows for the library seed and model.
- Pruning deletes dependent sidecar rows in one transaction before deleting expired candidates.
- Merge moves sightings, features, embeddings, and neighbors before deleting the loser candidate.
- External candidates are scored as sidecar neighbors and do not become fake `tracks` rows.
- External neighbor reads can require `tidal_id`, keeping unresolved candidates diagnostics-only for playback consumers.

Known follow-up items:
- Live provider refresh is wired before trainer input build. It uses Last.fm similar tracks and the existing TIDAL editorial track endpoint as the current new-release source.
- External metrics remain lightweight counts plus refresh-budget diagnostics. Rich source-specific diagnostics remain follow-up work.
- Automix consumption of sidecar candidates is implemented in Phase 4.

Sign-off:
- Phase 3 changes are sidecar-only and do not insert external candidates into `tracks`.
- No playback runtime, gapless, or audio runtime files were touched.
- No past migration was edited beyond the current appended `MIGRATION_037` under development.
- No bare `cargo update` was run.
- New SQL uses parameterized values except static placeholder generation for seed lists.

## Phase 4 Automix Integration

Files changed:
- `noor-server/src/playback/player.rs`
- `noor-server/src/server/routes.rs`
- `noor-server/src/db/queries.rs`
- `noor-server/src/services/discovery_trainer.rs`
- `noor-server/src/services/learning.rs`

Behavior added:
- Library automix still reads through `get_track_neighbors`.
- `automix_allow_external = false` keeps the existing library-only path.
- `automix_allow_external = true` reads TIDAL-resolved sidecar neighbors and appends pending queue rows with `queue::append_external_track`.
- External pending rows use `pending_artist`, `pending_title`, `tidal_id_hint`, and source label `automix-new`.
- External sidecar refill overfetches and skips already queued external hints/title pairs before appending.
- Pending resolver promotion now writes the resolved local track id back to matching external candidates.
- External candidate trainer tokens include source provenance and freshness buckets.
- Provider refresh budget constants are enforced by a planner: 100 seed tracks, 20 Last.fm rows per seed, and 500 TIDAL new-release rows.
- Fresh incremental provider refreshes are skipped by the planner until 24 hours have elapsed.
- Training metrics record the external refresh budget decision and caps.
- Training attempts sidecar refresh before trainer input is built, so refreshed rows are eligible for the same run's audio-proxy stage.
- Last.fm `track.getSimilar` rows are upserted with `lastfm_similar` sightings.
- Last.fm refresh is throttled with a 500 ms delay between seed calls and stops with cooldown diagnostics on rate-limit errors.
- TIDAL editorial track rows are upserted with `tidal_new_release` sightings.
- Provider refresh failures are non-blocking and recorded as zero-row refresh diagnostics.
- Source weighting now covers both `listen_history.source` and `playback_transitions.transition_source`.
- `playback_transitions.completed_prev` now acts as a weak fallback multiplier rather than a primary confidence signal.
- External refresh metrics now include source-specific candidate, sighting, and skipped-row counters for `lastfm_similar` and `tidal_new_release`.
- Route-level automix fallback logic is now covered so sidecar-filled `automix-new` rows suppress blind discovery injection.

Tests run:
- `cargo test -p noor-server promote_pending_row_emit_marks_external_candidate_resolved`
- `cargo test -p noor-server external_candidate_tokens_include_provenance_and_freshness`
- `cargo test -p noor-server external_candidates_for_training_skip_expired_and_resolved_rows`
- `cargo test -p noor-server external_provider_refresh_budget`
- `cargo test -p noor-server external_provider_refresh`
- `cargo test -p noor-server provider_rate_limit_errors_are_detected_for_cooldown`
- `cargo test -p noor-server transition_source_weighting_prefers_manual_completed_edges`
- `cargo test -p noor-server external_provider_refresh_persists_lastfm_and_tidal_candidates`
- `cargo test -p noor-server automix_discover_new_fallback_waits_when_sidecar_new_rows_fill_slots`
- `cargo test -p noor-server ensure_automix_external_overfetches_past_already_queued_candidates`
- `cargo test -p noor-server discovery`
- `cargo test -p noor-server playback::player`
- `cargo test -p noor-server promote_pending_row_emit`

Audit findings:
- Automix sidecar candidates are queued as pending rows, not fake library tracks.
- Only TIDAL-resolved sidecar candidates are read for playback by default.
- Resolver writeback updates sidecar candidate storage after successful pending promotion.
- The existing pending resolver remains the only external playback resolution path.
- The source label `automix-new` remains unchanged.
- Queue exclusion handling skips already queued external candidates and overfetches to avoid starving refill.
- Blind `automix_discover_new` injection counts existing `automix-new` rows, so sidecar-filled external slots suppress competing fallback injection.
- Provider caps are represented by tested constants, recorded metrics, and enforced before Last.fm or TIDAL refresh calls.
- Last.fm refresh uses a lower 100-seed cap and a 500 ms inter-call delay to avoid burst behavior.
- Rate-limit-like Last.fm errors stop the refresh early and record cooldown metrics.
- The current TIDAL source uses the existing editorial track endpoint because the repo does not yet have a confirmed dedicated new-release endpoint.
- Source-specific external diagnostics distinguish Last.fm similar rows from TIDAL new-release rows.
- Route-level fallback coverage confirms blind discovery injection waits when sidecar `automix-new` rows already fill the external slots.
- TIDAL endpoint verification checked the official Developer Portal and SDK/API reference. No confirmed dedicated new-release endpoint was found, so the editorial endpoint remains the safe fallback.

Known follow-up items:
- Replace the TIDAL editorial endpoint with a dedicated new-release endpoint if TIDAL documents or exposes one.

Sign-off:
- Phase 4 did not touch playback runtime, gapless, or audio runtime files.
- No past migration was edited.
- No bare `cargo update` was run.
- New SQL uses parameterized values except static placeholder generation for seed lists.
- Rollback remains possible because V2 activation and prior ready-model reactivation are independent of sidecar rows.

## Final Audit

Files changed:
- `docs/discovery-v2-audit-log.md`
- `noor-server/src/db/queries.rs`
- `noor-server/src/db/schema.rs`
- `noor-server/src/playback/player.rs`
- `noor-server/src/server/routes.rs`
- `noor-server/src/services/discovery_trainer.rs`
- `noor-server/src/services/learning.rs`
- `noor-server/src/services/neighbor_refresh.rs`

Behavior added:
- V2 model training remains isolated under `discovery-fusion-v2`.
- V2 activation uses typed heldout metrics and stored active-model comparison.
- V1 rollback remains possible through ready-model reactivation.
- External storage remains sidecar-only until pending playback resolution.
- Automix external candidates are queued as pending rows and resolved through the existing pending resolver.
- Metrics distinguish transition recall, skip-aware recall, cold-track recall, discovery lift, and external refresh diagnostics.
- Provider budget metrics separate external refresh from core library training.
- External refresh diagnostics now split Last.fm similar and TIDAL new-release source counters.
- Route-level fallback coverage verifies sidecar `automix-new` rows suppress blind TIDAL discovery injection.

Tests run:
- `cargo fmt --all -- --check`
- `cargo test -p noor-server discovery`
- `cargo test -p noor-server discovery_trainer`
- `cargo test -p noor-server learning`
- `cargo test -p noor-server db::queries`
- `cargo test -p noor-server playback::queue`
- `cargo test -p noor-server playback::player`
- `cargo test -p noor-server promote_pending_row_emit`
- `cargo test -p noor-server automix_discover_new_fallback_waits_when_sidecar_new_rows_fill_slots`

Audit findings:
- Drift check found no edits to `noor-server/src/playback/runtime.rs`, `noor-server/src/playback/gapless.rs`, or `noor-server/src/playback/wasapi_exclusive.rs`.
- `git diff --check` passed.
- Added diff contains no em dashes.
- The focused `db::queries` suite exposed and fixed a sidecar merge regression where fallback identity lookup matched a TIDAL-resolved winner.
- The focused `playback::player` suite exposed and fixed test fixture schema drift after the external candidate identity columns were added.
- The analytics empty-ridgeline test was corrected to match existing zero-filled ridgeline behavior.
- TIDAL endpoint verification used the official Developer Portal and SDK/API reference. No confirmed dedicated new-release endpoint was found.

Known follow-up items:
- Replace the TIDAL editorial endpoint with a dedicated new-release endpoint if TIDAL documents or exposes one.

Sign-off:
- Final audit did not drift from the Discovery Engine V2 phased plan.
- No past migration was edited.
- No bare `cargo update` was run.
- Active model rollback remains possible after V2 activation.

## Engine Selector Follow-up

Files changed:
- `frontend/src/lib/api/client.ts`
- `frontend/src/routes/settings/+page.svelte`
- `noor-server/src/db/models.rs`
- `noor-server/src/db/queries.rs`
- `noor-server/src/playback/player.rs`
- `noor-server/src/server/routes.rs`
- `noor-server/src/services/learning.rs`
- `noor-server/src/services/neighbor_refresh.rs`

Behavior added:
- Added persisted `discovery_engine` setting with V2 as the default.
- Added `GET /api/discovery/train/engine` and `POST /api/discovery/train/engine`.
- Runtime discovery reads now use the selected engine family instead of the global active model.
- V1 selection reads existing `discovery-fusion` models only when explicitly selected.
- V2 remains the only trainable engine in this branch.
- Training while V1 is selected returns `legacy_trainer_unavailable` before creating a run or model.
- Settings now shows a Discovery engine dropdown with V2 recommended and V1 legacy.
- Training buttons are disabled while the selected engine is read-only.

Tests run:
- `cargo test -p noor-server discovery_engine`
- `cargo test -p noor-server selected_discovery_model_lookup_uses_configured_engine_family`
- `cargo test -p noor-server start_training_refuses_legacy_engine_without_starting_v2`
- `cargo test -p noor-server discovery`
- `cargo test -p noor-server learning`
- `cargo test -p noor-server db::queries`
- `cargo test -p noor-server playback::player`
- `cargo test -p noor-server neighbor_refresh`
- `cargo test -p noor-server automix_discover_new_fallback_waits_when_sidecar_new_rows_fill_slots`
- `cargo fmt --all -- --check`
- `git diff --check`

Audit findings:
- V2 is default when no setting exists.
- V1 must be explicitly selected and does not dual-run with V2.
- Selected-family lookup allows V1 fallback reads without reactivating or deleting V2 rows.
- The training guard runs before creating `training_runs` or `embedding_models` rows.
- Automix and seed refresh reads honor the selected engine family.
- The player test fixture needed `server_config` after selected-engine lookup was introduced.
- The neighbor refresh fixture needed a ready V2-family model after selected-engine lookup was introduced.
- Frontend `pnpm lint` and `pnpm check` were blocked because `frontend/node_modules` is absent in this worktree.

Known follow-up items:
- Install frontend dependencies before running Svelte and stylelint checks.
- Restore a dedicated V1 trainer only if legacy retraining becomes a product requirement.

Sign-off:
- Follow-up did not touch playback runtime, gapless, or WASAPI exclusive files.
- No past migration was edited.
- No bare `cargo update` was run.
- No dual-run behavior was added.

## Laptop Safety Follow-up

Files changed:
- `frontend/src/lib/api/client.ts`
- `frontend/src/routes/settings/+page.svelte`
- `noor-server/src/db/queries.rs`
- `noor-server/src/server/routes.rs`
- `noor-server/src/services/learning.rs`

Behavior added:
- Discovery training now runs inside a dedicated Rayon pool instead of the global pool.
- Added training safety profiles: `laptop_safe`, `balanced`, and `performance`.
- `balanced` is the default and uses up to 8 workers while keeping two cores free when available.
- `laptop_safe` uses up to 4 workers while keeping one core free.
- `performance` is opt-in and uses up to 16 workers while keeping one core free.
- Added a cooperative watchdog timeout: 30 minutes for Low and Medium, 60 minutes for Max.
- Watchdog cancellation persists a clear cancellation reason in `training_runs.error_text`.
- Settings shows the selected CPU safety profile, worker count, and watchdog cap.
- Settings shows a calm safety notice if discovery training is cancelled by the watchdog.

Tests run:
- `cargo test -p noor-server training_safety_timeout_scales_by_intensity`
- `cargo test -p noor-server training_worker_cap_adapts_by_safety_profile`
- `cargo test -p noor-server training_safety_profile_defaults_to_balanced_and_round_trips`
- `cargo test -p noor-server finish_training_run_with_error_preserves_cancel_reason`
- `cargo test -p noor-server learning`
- `cargo test -p noor-server db::queries`
- `cargo test -p noor-server discovery`
- `cargo fmt --all -- --check`
- `git diff --check`

Audit findings:
- No universal 4-worker cap remains.
- Balanced default avoids full CPU saturation but does not unnecessarily punish high-core machines.
- Performance mode is explicit opt-in.
- The watchdog is cooperative and uses the existing trainer cancel checks.
- Frontend `pnpm lint` and `pnpm check` remain blocked because `frontend/node_modules` is absent in this worktree.

Known follow-up items:
- Install frontend dependencies before running Svelte and stylelint checks.

Sign-off:
- Follow-up did not touch playback runtime, gapless, or WASAPI exclusive files.
- No past migration was edited.
- No bare `cargo update` was run.
