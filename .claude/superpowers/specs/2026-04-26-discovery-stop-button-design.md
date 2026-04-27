# Discovery Engine Stop Button + CPU/Heat Warning

**Date:** 2026-04-26
**Scope:** Settings → Discovery section only.

## Goal

Add a Stop button that cancels an in-flight discovery training run and a prominent warning that the training pipeline is CPU- and heat-intensive. No other behavioral changes.

## Background

The discovery engine training runs as a background `tokio::spawn` task started by `POST /api/discovery/train`. The pipeline lives in `noor-server/src/services/learning.rs::start_training` and delegates the heavy math to `noor-server/src/services/discovery_trainer.rs::run_discovery_training`. The expensive stage is `similarity_neighbors` — a rayon-parallelized O(n²) cosine sweep that, per its own header comment, takes 10–30 seconds on a 32k-track library and pegs every CPU core.

Today there is no way to cancel a run once it starts. The settings page exposes Incremental refresh and Full retrain buttons, but no Stop. The MusicBrainz and Last.fm enrichment sections in the same settings page already use a `Arc<AtomicBool>` cancel pattern; this spec applies that same pattern to discovery training.

## Non-goals

- No automatic retraining when listening behavior changes.
- No new "enrichment buttons" inside the Discovery section.
- No changes to the Discovery section's existing metrics, model display, or other surfaces.
- No structural rework of the trainer beyond threading a cancel flag.
- No confirm-on-click dialog; the inline warning is the only friction.

## Architecture

### Cancel flag

Add a single shared `Arc<AtomicBool>` to `AppState`, mirroring the existing `lastfm_enrich_cancel`:

- Field: `discovery_train_cancel: Arc<AtomicBool>` on `AppState` in `noor-server/src/main.rs`.
- Initialized to `false` in the `AppState` constructor.

### Cancellation points

The flag is polled at two granularities:

1. **Stage boundaries in `start_training`** — between the cheap stages (corpus → behavioral → audio → fusion → DB writes) the flag is checked. If set, the function records the run as `cancelled` and returns early without persisting embeddings, neighbors, or audio features and without activating the model.
2. **Inside `similarity_neighbors`'s hot loop** — the cancel flag is polled at the top of every per-track closure (an `AtomicBool::load(Relaxed)` read is single-digit ns and dwarfed by the O(n) cosine work that follows it). When set, the closure returns an empty `Vec<TrainerNeighbor>` instead of computing scores; the rayon collect drains within tens of milliseconds. Progress reporting stays at the existing per-500-track cadence — only the cancel check moves to every iteration.

The hot-loop check is the load-bearing one: without it, Stop pressed during Stage 4 has up to ~30 seconds of lag, which contradicts the warning copy. The stage-boundary checks close the smaller window where Stop is pressed during the cheap stages.

### Trainer signature change

`run_discovery_training` and `similarity_neighbors` accept an additional optional cancel flag:

```rust
pub fn run_discovery_training(
    input: TrainerInput,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<TrainingProgressUpdate>>,
    cancel: Option<Arc<AtomicBool>>,
) -> TrainerOutput
```

`Option<Arc<AtomicBool>>` keeps the trainer callable from anywhere that doesn't have an `AppState`. When `None`, behavior is identical to today.

### DB state on cancel

When `start_training` detects the cancel flag is set, it:

1. Skips remaining DB persistence (`replace_track_audio_features`, `replace_track_embeddings`, `replace_track_neighbors`).
2. Does **not** call `activate_embedding_model`.
3. Calls `finish_training_run(conn, run.id, "cancelled")` instead of `"completed"` or `"failed"`.

The `training_runs.status` column is a free-form text field (it already accepts `running`, `completed`, `failed`); `cancelled` is a new value but no schema change is needed. We will verify at implementation time that no query filters explicitly on `status IN ('completed', 'failed', 'running')`.

### Endpoint

New route: `POST /api/discovery/train/stop`.

Handler is a near-clone of `stop_lastfm_enrichment` at `routes.rs:7458`:

```rust
async fn stop_discovery_training(
    State(state): State<SharedState>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;
    let s = state.read().await;
    s.discovery_train_cancel.store(true, Ordering::Relaxed);
    Ok(Json(json!({ "status": "stopping" })))
}
```

`start_discovery_training` (`routes.rs:1574`) is modified to:

1. Reset the cancel flag to `false` *before* spawning the training task (matches the audio-analysis pattern at `routes.rs:7493`).
2. Clone the flag into the spawned task and pass it through to `start_training`.

The handler's existing "already_running" guard remains unchanged.

### Frontend

#### Stop button

In `frontend/src/lib/api/client.ts`, add:

```ts
stopDiscoveryTraining() {
  return fetchApi<{ status: string }>('/api/discovery/train/stop', undefined, {
    method: 'POST',
  });
}
```

In `frontend/src/routes/settings/+page.svelte`:

- Add `async function stopDiscoveryTraining()` calling the API method.
- Add `let discoveryIsRunning = $derived(discoveryStatus?.latest_run?.status === 'running');`.
- The action row at line 1246 grows a third button:
  - Incremental refresh — `disabled={discoveryIsRunning}`
  - Full retrain — `disabled={discoveryIsRunning}`
  - Stop (rendered only when `discoveryIsRunning`) — `class="btn btn-glass"`, calls `stopDiscoveryTraining`.

This mirrors the Last.fm Stop pattern at line 1201.

#### Warning card

Inserted at the **top** of the Discovery section (above the stat-grid, above the model info card). Markup:

```svelte
<div class="discovery-warning glass-panel">
  <h4>⚠ Heads up — this runs hot.</h4>
  <p>
    A retrain pegs every CPU core for 10–30 seconds on a typical library,
    longer on bigger ones. Your fans will spin up. If you're on a laptop or
    somewhere thermally constrained, expect heat. Hit <strong>Stop</strong>
    any time.
  </p>
</div>
```

Styling (scoped to the existing `<style>` block in `+page.svelte`):

- Reuses `.glass-panel` for base treatment so it sits inside the page's visual language.
- Adds left border `~3-4px` in desaturated red (`rgba(220, 70, 70, 0.6)`).
- Heading `font-size: 1.05rem; font-weight: 600`.
- Body copy at the section's existing default size.
- Background remains the existing glass tint — no solid fill.

The warning is persistent: it does not dismiss. Rationale: this is the only place in the app that warns about heat, and users may bounce in and out of the section between training runs.

## Data flow

```
[Settings UI: Stop click]
   │
   └─ POST /api/discovery/train/stop
        │
        └─ stop_discovery_training handler
             │
             └─ AppState.discovery_train_cancel.store(true)
                  │
                  ├─ similarity_neighbors hot loop next ≤500-track checkpoint:
                  │     reads flag → returns empty Vec from per-thread closure
                  │
                  └─ start_training next stage boundary:
                        reads flag → skips persistence,
                        finishes run as "cancelled", returns Ok(())

[Settings UI: poll discoveryStatus]
   │
   └─ GET /api/discovery/train/status
        │
        └─ training_runs.status = "cancelled"
             discoveryIsRunning becomes false
             Stop button hides; Start buttons re-enable
```

## Error handling

- **Stop pressed when no run is active:** the flag is flipped, no spawned task is reading it, no-op. The Stop button is hidden in that state, so this is purely defensive.
- **Stop pressed multiple times:** idempotent; AtomicBool stays `true`.
- **Process crash mid-run:** out of scope. Pre-existing behavior leaves the run in `running` status; this spec does not address it.
- **Cancel flag race:** flag is reset to `false` synchronously inside `start_discovery_training` *before* `tokio::spawn`. A Stop request that arrives between reset and the first poll inside the spawned task is preserved because the task's first poll reads `true`.
- **Trainer panic:** existing `spawn_blocking` `.context("discovery trainer panicked")` handling is unchanged. A panic is not a cancel; the run is recorded as `failed` by upstream error handling, not `cancelled`.

## Testing

Manual verification (the codebase has no existing unit tests for `start_training` or `similarity_neighbors`):

1. Start a training run via the UI; verify Stop button appears and Start buttons disable.
2. Press Stop during Stage 4 (the long stage); verify the run terminates within ~1 second and the UI returns to idle within one status-poll interval.
3. Inspect `training_runs` table: cancelled run has `status = 'cancelled'`.
4. Verify no new rows in `track_embeddings`, `track_neighbors`, or `track_audio_features` from the cancelled run.
5. Verify `embedding_models.is_active` was not flipped by the cancelled run.
6. Start a new run after a cancellation; verify it proceeds normally.
7. Visual check: warning card renders at the top of the section with the red left border and bold heading.

One unit test is in scope: a focused test for `similarity_neighbors` verifying that with `cancel = Some(flag_set_to_true)`, the function returns within a small bounded number of iterations (e.g. construct a synthetic input with 5000 fake tracks, set the flag before calling, assert the result vec is empty or near-empty and the call returns in well under 1 second). This locks in the cancel-flag wiring at the load-bearing site.

## Files touched

**Backend:**

- `noor-server/src/main.rs` — add `discovery_train_cancel` field, initialize.
- `noor-server/src/server/routes.rs` — register `/api/discovery/train/stop`, add handler, modify `start_discovery_training`.
- `noor-server/src/services/learning.rs` — accept cancel flag, check at stage boundaries, persist `cancelled` status, short-circuit DB writes on cancel.
- `noor-server/src/services/discovery_trainer.rs` — accept cancel flag, poll inside the existing per-500-track checkpoint in `similarity_neighbors`.

**Frontend:**

- `frontend/src/lib/api/client.ts` — add `stopDiscoveryTraining()`.
- `frontend/src/routes/settings/+page.svelte` — `stopDiscoveryTraining` function, `discoveryIsRunning` derived state, Stop button, warning card markup, scoped CSS for `.discovery-warning`.

## Out of scope (explicitly)

- Auto-refresh on listening-behavior change.
- Enrichment buttons inside Discovery section.
- Recovery of `running` rows after process crash.
- Cancellation during the small DB-write tail end after Stage 4 — those writes are fast and atomic; cancel during them is allowed to complete.
- Test coverage for the broader pipeline beyond the one targeted unit test above.
