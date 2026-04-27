# Discovery Engine Stop Button Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Stop button to the Discovery engine settings panel that cancels an in-flight training run, plus a prominent CPU/heat warning above the panel's controls.

**Architecture:** Mirrors the existing `lastfm_enrich_cancel: Arc<AtomicBool>` cancel pattern. A new `discovery_train_cancel` flag on `AppState` is reset on training start and flipped to `true` by a new `POST /api/discovery/train/stop` endpoint. The flag is polled at every iteration of `similarity_neighbors`'s rayon loop (where ~all the runtime lives) and at every stage boundary in `start_training`. On cancel, persistence stages are skipped, the model is not activated, and the `training_runs` row is closed with status `"cancelled"`.

**Tech Stack:** Rust (axum, rayon, tokio), Svelte 5 (runes), TypeScript.

**Spec:** `docs/superpowers/specs/2026-04-26-discovery-stop-button-design.md`

---

## Files touched

**Backend (Rust):**
- `noor-server/src/main.rs` — add `discovery_train_cancel: Arc<AtomicBool>` to `AppState` (struct + initializer).
- `noor-server/src/services/discovery_trainer.rs` — accept optional cancel flag in `run_discovery_training` and `similarity_neighbors`; poll it at every iteration of the per-track closure; add unit test.
- `noor-server/src/services/learning.rs` — accept cancel flag in `start_training`; check at stage boundaries; on cancel skip DB persistence + model activation and finish the run as `"cancelled"`.
- `noor-server/src/server/routes.rs` — register `POST /api/discovery/train/stop`; modify `start_discovery_training` to reset the flag and pass it through; add `stop_discovery_training` handler.

**Frontend (Svelte/TS):**
- `frontend/src/lib/api/client.ts` — add `stopDiscoveryTraining()` API method.
- `frontend/src/routes/settings/+page.svelte` — `stopDiscoveryTraining` function, `discoveryIsRunning` derived state, Stop button + disabled-when-running on Start buttons, warning card markup, scoped CSS for `.discovery-warning`.

---

## Task 1: Plumb cancel flag through the trainer (TDD)

**Files:**
- Modify: `noor-server/src/services/discovery_trainer.rs` (functions `run_discovery_training` at line 614, `similarity_neighbors` at line 334)
- Test: `noor-server/src/services/discovery_trainer.rs` (new `#[cfg(test)] mod tests` block at the end of the file)

This task uses TDD: write the failing cancel test first, then add the cancel flag to the function signatures and the per-iteration check.

- [ ] **Step 1: Add the failing unit test**

Open `noor-server/src/services/discovery_trainer.rs`. The file currently has no `tests` submodule. Append a new test module at the end of the file (after the closing brace of the last function):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn make_test_input(track_count: usize, dim: usize) -> (
        Vec<EmbeddingTrackRow>,
        HashMap<i64, Vec<f64>>,
        HashMap<i64, TrainerAudioFeature>,
        HashMap<i64, Vec<f64>>,
    ) {
        let tracks: Vec<EmbeddingTrackRow> = (0..track_count as i64)
            .map(|i| EmbeddingTrackRow {
                track_id: i,
                title: format!("track_{i}"),
                artist_name: Some(format!("artist_{}", i / 10)),
                album_title: None,
                duration_ms: Some(180_000),
                best_quality: None,
                source: "local".to_string(),
                play_count: 0,
                is_favorite: false,
                playlist_memberships: 0,
                genre_paths: Vec::new(),
                bpm: None,
                energy: None,
                camelot_key: None,
            })
            .collect();

        let unit = 1.0_f64 / (dim as f64).sqrt();
        let behavioral: HashMap<i64, Vec<f64>> = tracks
            .iter()
            .map(|t| (t.track_id, vec![unit; dim]))
            .collect();
        let audio: HashMap<i64, TrainerAudioFeature> = tracks
            .iter()
            .map(|t| {
                (
                    t.track_id,
                    TrainerAudioFeature {
                        vector: vec![unit; dim],
                        clip_start_ms: 0,
                        clip_duration_ms: 20_000,
                        feature_version: "test".to_string(),
                    },
                )
            })
            .collect();
        let fusion: HashMap<i64, Vec<f64>> = tracks
            .iter()
            .map(|t| (t.track_id, vec![unit; dim]))
            .collect();

        (tracks, behavioral, audio, fusion)
    }

    #[test]
    fn similarity_neighbors_aborts_when_cancel_flag_set() {
        let (tracks, behavioral, audio, fusion) = make_test_input(200, 32);
        let cancel = Arc::new(AtomicBool::new(true));

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            10,
            None,
            Some(&cancel),
        );

        assert!(
            result.is_empty(),
            "expected zero neighbors when cancel is pre-set, got {}",
            result.len(),
        );
    }

    #[test]
    fn similarity_neighbors_runs_normally_without_cancel() {
        let (tracks, behavioral, audio, fusion) = make_test_input(50, 32);
        let cancel = Arc::new(AtomicBool::new(false));

        let result = similarity_neighbors(
            &tracks,
            &behavioral,
            &audio,
            &fusion,
            10,
            None,
            Some(&cancel),
        );

        // 50 tracks × top_k=10, all vectors identical → every track has 10 neighbors
        assert_eq!(result.len(), 500, "expected 50*10 = 500 neighbor rows");
    }
}
```

The two tests pin both directions: cancel pre-set → empty result, cancel clear → normal output. The second test guards against accidentally short-circuiting the loop in the implementation step.

- [ ] **Step 2: Run the test and confirm it fails to compile**

Run: `cargo test -p noor-server --lib services::discovery_trainer::tests`

Expected: compile error along the lines of `this function takes 6 arguments but 7 arguments were supplied` because `similarity_neighbors` does not yet accept a cancel parameter.

- [ ] **Step 3: Add cancel parameter to `similarity_neighbors`**

In `noor-server/src/services/discovery_trainer.rs`, modify the `similarity_neighbors` signature (currently at line 334) to add a trailing optional cancel flag:

```rust
fn similarity_neighbors(
    tracks: &[EmbeddingTrackRow],
    behavioral: &HashMap<i64, Vec<f64>>,
    audio: &HashMap<i64, TrainerAudioFeature>,
    fusion: &HashMap<i64, Vec<f64>>,
    top_k: usize,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<TrainingProgressUpdate>>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Vec<TrainerNeighbor> {
```

Inside the per-track closure passed to `into_par_iter().map(|idx| { ... })` (currently starts at line 399), add a cancel check as the very first statement of the closure, before the existing `if idx % 500 == 0` progress block:

```rust
.map(|idx| {
    // Cancel check — cheap atomic load, runs every iteration so Stop is responsive.
    if let Some(flag) = cancel {
        if flag.load(std::sync::atomic::Ordering::Relaxed) {
            return Vec::<TrainerNeighbor>::new();
        }
    }

    // Progress every 500 tracks
    if idx % 500 == 0 && total > 0 {
        // … existing progress reporting unchanged …
    }
    // … rest of closure unchanged …
})
```

Leave the existing `if idx % 500 == 0 && total > 0 { … }` progress-reporting block exactly as it is.

- [ ] **Step 4: Add cancel parameter to `run_discovery_training`**

In the same file, modify `run_discovery_training` (currently at line 614) to accept and forward the cancel flag:

```rust
pub fn run_discovery_training(
    input: TrainerInput,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<TrainingProgressUpdate>>,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> TrainerOutput {
```

Update the single call to `similarity_neighbors` inside this function (currently at line 664) to pass the new argument:

```rust
let neighbors = similarity_neighbors(&tracks, &behavioral, &audio, &fusion, top_k, progress_tx, cancel);
```

- [ ] **Step 5: Update the existing caller in `learning.rs` to pass `None` for now**

In `noor-server/src/services/learning.rs` at line 100-104, change the call from:

```rust
let output = tokio::task::spawn_blocking(move || {
    run_discovery_training(input, Some(&progress_tx_clone))
})
```

to:

```rust
let output = tokio::task::spawn_blocking(move || {
    run_discovery_training(input, Some(&progress_tx_clone), None)
})
```

We pass `None` here intentionally; Task 3 wires the real cancel flag through.

- [ ] **Step 6: Run the tests and confirm they pass**

Run: `cargo test -p noor-server --lib services::discovery_trainer::tests`

Expected:
```
test services::discovery_trainer::tests::similarity_neighbors_aborts_when_cancel_flag_set ... ok
test services::discovery_trainer::tests::similarity_neighbors_runs_normally_without_cancel ... ok
```

If the second test fails with a different count than 500, recheck that the closure's cancel check happens *only* when `cancel.is_some() && flag.load() == true` — a stray early return in the no-cancel case is the likely cause.

- [ ] **Step 7: Commit**

```bash
git add noor-server/src/services/discovery_trainer.rs noor-server/src/services/learning.rs
git commit -m "feat(discovery): plumb cancel flag through trainer hot loop"
```

---

## Task 2: Add `discovery_train_cancel` to `AppState`

**Files:**
- Modify: `noor-server/src/main.rs:55-72` (struct field) and `noor-server/src/main.rs:200-217` (initializer)

- [ ] **Step 1: Add the field to the `AppState` struct**

In `noor-server/src/main.rs`, find the `AppState` struct (line 32). After the existing `lastfm_enrich_cancel: Arc<AtomicBool>` field (line 61), add a new field grouped with the discovery-training comments:

```rust
    /// Discovery training cancel flag — flipped to true by POST /api/discovery/train/stop,
    /// reset to false at the start of each training run.
    pub discovery_train_cancel: Arc<AtomicBool>,
```

- [ ] **Step 2: Initialize the field in the `AppState` constructor**

In the same file at line 200-217, find the `AppState { … }` initializer and add the new field right after the `lastfm_enrich_started_at` line (around line 215):

```rust
        lastfm_enrich_started_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        discovery_train_cancel: Arc::new(AtomicBool::new(false)),
        server_token,
```

- [ ] **Step 3: Build to confirm it compiles**

Run: `cargo build -p noor-server`

Expected: clean build, no errors.

- [ ] **Step 4: Commit**

```bash
git add noor-server/src/main.rs
git commit -m "feat(discovery): add discovery_train_cancel atomic flag to AppState"
```

---

## Task 3: Wire cancel into `start_training` with stage-boundary checks

**Files:**
- Modify: `noor-server/src/services/learning.rs:33-199`

- [ ] **Step 1: Update `start_training` signature**

In `noor-server/src/services/learning.rs` at line 33, change the signature to accept the cancel flag. Add the imports if not already present at the top of the file:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
```

Then change the function:

```rust
pub async fn start_training(
    db: Database,
    event_tx: Sender<AppEvent>,
    full_mode: bool,
    rebuild_audio: bool,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
```

- [ ] **Step 2: Add a cancel helper closure inside `start_training`**

Right after creating the training run (after line 58, the `let (model, run) = …` block), add a helper that knows how to mark the run as cancelled and short-circuit:

```rust
    // If cancel is requested at any stage boundary, mark the run as cancelled
    // and skip remaining persistence + model activation.
    let bail_if_cancelled = |stage: &str| -> Result<bool> {
        if cancel.load(Ordering::Relaxed) {
            tracing::info!(
                target: "noor.discovery.training",
                run_id = run.id,
                stage = stage,
                "discovery training cancelled by user"
            );
            db.with_conn(|conn| queries::finish_training_run(conn, run.id, "cancelled"))?;
            return Ok(true);
        }
        Ok(false)
    };
```

Note: this closure clones nothing on the hot path — it just reads the `Arc<AtomicBool>` and (only on cancel) writes one row.

- [ ] **Step 3: Forward the cancel flag into `run_discovery_training`**

At line 100-104, change the spawn-blocking call from:

```rust
    let output = tokio::task::spawn_blocking(move || {
        run_discovery_training(input, Some(&progress_tx_clone), None)
    })
```

to:

```rust
    let cancel_for_trainer = cancel.clone();
    let output = tokio::task::spawn_blocking(move || {
        run_discovery_training(input, Some(&progress_tx_clone), Some(&cancel_for_trainer))
    })
```

- [ ] **Step 4: Add cancel checks at every stage boundary after the trainer returns**

After line 108 (`let _ = log_task.await;`) and *before* line 110 (`db.with_conn(|conn| { queries::update_training_run_progress(conn, run.id, "audio", "running", 0.55, None, 0) })?;`), add:

```rust
    if bail_if_cancelled("audio")? {
        return Ok(());
    }
```

Repeat the same pattern before:
- The `replace_track_audio_features` call (currently line 128) — stage label `"audio"`.
- The `replace_track_embeddings` call (currently line 141) — stage label `"fusion"`.
- The `replace_track_neighbors` call inside the `with_conn` block at line 171-174 — stage label `"neighbors"`.
- The final `with_conn` block that activates the model and finishes the run (line 189-196) — stage label `"evaluate"`.

For the final block, replace the entire `db.with_conn(|conn| { … queries::finish_training_run(conn, run.id, "completed") })?;` with a leading cancel check:

```rust
    if bail_if_cancelled("evaluate")? {
        return Ok(());
    }
    db.with_conn(|conn| {
        queries::update_training_run_progress(conn, run.id, "evaluate", "running", 0.96, None, 0)?;
        queries::update_embedding_model_metrics(conn, model.id, "ready", Some(&metrics_json))?;
        if should_activate {
            queries::activate_embedding_model(conn, model.id)?;
        }
        queries::finish_training_run(conn, run.id, "completed")
    })?;
```

The pattern is consistent: every checkpoint reads the flag, and if set, finishes the run with `"cancelled"` and returns cleanly without writing more data or activating the model.

- [ ] **Step 5: Build to confirm it compiles**

Run: `cargo build -p noor-server`

Expected: a single error in `routes.rs` at line 1606 about `start_training` taking 5 args instead of 4. That's the call site we change in Task 4. Resolving that is the final compile step.

- [ ] **Step 6: Don't commit yet**

This task's compile error is resolved by Task 4. Hold off on committing until both tasks build clean. (We'll commit them together at the end of Task 4.)

---

## Task 4: Add `/api/discovery/train/stop` endpoint and wire `start_discovery_training`

**Files:**
- Modify: `noor-server/src/server/routes.rs:268-269` (route registration), `noor-server/src/server/routes.rs:1574-1618` (start handler), and append a new handler near line 1618.

- [ ] **Step 1: Register the new route**

In `noor-server/src/server/routes.rs` at line 269 (right after the `/api/discovery/train/status` route registration), add:

```rust
        .route("/api/discovery/train/stop", post(stop_discovery_training))
```

So the surrounding lines now read:

```rust
        .route("/api/discovery/train", post(start_discovery_training))
        .route("/api/discovery/train/status", get(get_discovery_training_status))
        .route("/api/discovery/train/stop", post(stop_discovery_training))
        .route("/api/discovery/feedback", post(record_discovery_feedback))
```

- [ ] **Step 2: Modify `start_discovery_training` to reset the flag and forward it**

In `routes.rs` at line 1574-1618, replace the `start_discovery_training` body so that:

1. The cancel flag is grabbed from state alongside the DB.
2. The flag is reset to `false` synchronously *before* `tokio::spawn`.
3. The flag is cloned into the spawned task and passed to `start_training`.

The replacement function:

```rust
async fn start_discovery_training(
    State(state): State<SharedState>,
    Json(payload): Json<DiscoveryTrainRequest>,
) -> Result<Json<Value>, StatusCode> {
    use std::sync::atomic::Ordering;

    let mode = payload.mode.as_deref().unwrap_or("incremental");
    let full_mode = mode == "full";
    let rebuild_audio = payload.rebuild_audio.unwrap_or(false);
    let (db, cancel) = {
        let guard = state.read().await;
        (guard.db.clone(), guard.discovery_train_cancel.clone())
    };

    // Guard: reject if a run is already in progress
    let already_running = db
        .with_conn(|conn| queries::get_latest_training_run(conn))
        .ok()
        .flatten()
        .map(|run| run.status == "running")
        .unwrap_or(false);

    if already_running {
        return Ok(Json(json!({
            "status": "already_running",
            "mode": mode
        })));
    }

    // Reset cancel flag synchronously before spawning so that a Stop request
    // arriving immediately after this call reaches the spawned task.
    cancel.store(false, Ordering::SeqCst);

    tokio::spawn(async move {
        let event_tx = {
            let guard = state.read().await;
            guard.event_tx.clone()
        };
        if let Err(error) =
            discovery_learning::start_training(db, event_tx, full_mode, rebuild_audio, cancel).await
        {
            tracing::error!(
                target: "noor.discovery.training",
                error = %error,
                "discovery learning pipeline failed"
            );
        }
    });
    Ok(Json(json!({
        "status": "training_started",
        "mode": if full_mode { "full" } else { "incremental" }
    })))
}
```

- [ ] **Step 3: Add the `stop_discovery_training` handler**

Append the new handler immediately after `start_discovery_training` (so it lives next to it in the file):

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

This is a near-clone of `stop_lastfm_enrichment` at line 7458.

- [ ] **Step 4: Build to confirm everything compiles**

Run: `cargo build -p noor-server`

Expected: clean build with no errors. Both Task 3 and Task 4 changes now compile together.

- [ ] **Step 5: Run the trainer tests to confirm nothing regressed**

Run: `cargo test -p noor-server --lib services::discovery_trainer::tests`

Expected: both tests still pass.

- [ ] **Step 6: Commit Tasks 3 and 4 together**

```bash
git add noor-server/src/services/learning.rs noor-server/src/server/routes.rs
git commit -m "feat(discovery): /api/discovery/train/stop endpoint + cancel-aware start_training"
```

---

## Task 5: Add `stopDiscoveryTraining` API method

**Files:**
- Modify: `frontend/src/lib/api/client.ts:872-877`

- [ ] **Step 1: Add the API method**

In `frontend/src/lib/api/client.ts`, find `startDiscoveryTraining` at line 872. Immediately after that method (before the `recordDiscoveryFeedback` method at line 879), add:

```ts
	stopDiscoveryTraining() {
		return fetchApi<{ status: string }>('/api/discovery/train/stop', undefined, {
			method: 'POST',
		});
	},
```

Tab-indentation matches the surrounding methods.

- [ ] **Step 2: Type-check the frontend**

Run: `cd frontend && npm run check`

Expected: no new errors introduced by this change.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/api/client.ts
git commit -m "feat(frontend): stopDiscoveryTraining API method"
```

---

## Task 6: Frontend Stop button + warning card

**Files:**
- Modify: `frontend/src/routes/settings/+page.svelte` — script block around line 652 (`startDiscoveryTraining` function), template around line 1210-1250 (Discovery section), and the `<style>` block at the bottom.

- [ ] **Step 1: Add the `stopDiscoveryTraining` function and `discoveryIsRunning` derived state**

In `frontend/src/routes/settings/+page.svelte`, find the `async function startDiscoveryTraining(mode: 'full' | 'incremental')` definition (around line 652). Immediately after that function, add:

```ts
	async function stopDiscoveryTraining() {
		try {
			await api.stopDiscoveryTraining();
			await loadDiscoveryStatus();
		} catch (err) {
			console.error('Failed to stop discovery training', err);
		}
	}

	let discoveryIsRunning = $derived(
		discoveryStatus?.latest_run?.status === 'running'
	);
```

`loadDiscoveryStatus` is the existing status-refresh function defined at line 640 in the same file; `startDiscoveryTraining` calls it the same way at line 655.

- [ ] **Step 2: Update the Discovery section template**

Find the Discovery section block at line 1210-1250. Replace the entire block (from `{#if activeCategory === 'discovery'}` through its matching `{/if}`) with:

```svelte
			{#if activeCategory === 'discovery'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Learning" title="Discovery engine" subtitle="Track how much of the library the learned radio engine has covered, and refresh it when listening behavior changes." />

				<div class="discovery-warning glass-panel">
					<h4>⚠ Heads up — this runs hot.</h4>
					<p>
						A retrain pegs every CPU core for 10–30 seconds on a typical library, longer on bigger ones. Your fans will spin up. If you're on a laptop or somewhere thermally constrained, expect heat. Hit <strong>Stop</strong> any time.
					</p>
				</div>

				<div class="stat-grid inner-metrics">
					<MetricPair label="Coverage" value={discoveryStatus ? `${Math.round(discoveryStatus.coverage_ratio * 100)}%` : '—'} copy="Playable tracks with learned neighborhoods." />
					<MetricPair label="Embedded" value={discoveryStatus?.embedded_tracks?.toLocaleString() ?? '0'} copy="Tracks with stored embedding vectors." />
				</div>

				<div class="portable-card glass">
					<div class="info-list">
						<div class="info-row">
							<span>Active model</span>
							<strong>{discoveryStatus?.active_model?.model_key ?? 'Fallback only'}</strong>
						</div>
						<div class="info-row">
							<span>Last trained</span>
							<strong>{discoveryStatus?.active_model?.trained_at ? new Date(discoveryStatus.active_model.trained_at + 'Z').toLocaleString() : '—'}</strong>
						</div>
						<div class="info-row">
							<span>Clip features</span>
							<strong>{discoveryStatus?.clip_cache_tracks?.toLocaleString() ?? '0'}</strong>
						</div>
						<div class="info-row">
							<span>Latest run</span>
							<strong>
								{#if discoveryStatus?.latest_run}
									{discoveryStatus.latest_run.status} · {discoveryStatus.latest_run.stage} · {Math.round(discoveryStatus.latest_run.progress * 100)}%
								{:else}
									idle
								{/if}
							</strong>
						</div>
					</div>
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void startDiscoveryTraining('incremental')} disabled={discoveryIsRunning}>Incremental refresh</button>
					<button class="btn btn-glass" onclick={() => void startDiscoveryTraining('full')} disabled={discoveryIsRunning}>Full retrain</button>
					{#if discoveryIsRunning}
						<button class="btn btn-glass" onclick={() => void stopDiscoveryTraining()}>Stop</button>
					{/if}
				</div>
			</section>
			{/if}
```

The diff against the existing block: warning card inserted between `<SectionHeader />` and `<div class="stat-grid inner-metrics">`; both Start buttons gain `disabled={discoveryIsRunning}`; a new `{#if discoveryIsRunning}` Stop button appears at the end of the action row.

- [ ] **Step 3: Add scoped CSS for `.discovery-warning`**

In the same file, scroll to the bottom `<style>` block. Add the following rules near the other discovery/section styles (placement within the block doesn't matter — pick a spot near `.enrichment-progress` or at the end):

```css
	.discovery-warning {
		border-left: 4px solid rgba(220, 70, 70, 0.6);
		padding: 1rem 1.25rem;
		margin-bottom: 1.25rem;
	}

	.discovery-warning h4 {
		font-size: 1.05rem;
		font-weight: 600;
		margin: 0 0 0.4rem;
	}

	.discovery-warning p {
		margin: 0;
		line-height: 1.5;
	}
```

- [ ] **Step 4: Type-check and lint the frontend**

Run: `cd frontend && npm run check`

Expected: no new errors. If `discoveryStatus.latest_run` flags a possibly-undefined warning, the `?.` chain on the `$derived` line should already handle it.

- [ ] **Step 5: Manual smoke test**

Start the backend and frontend. Open the Settings → Discovery tab. Verify:

1. The red-bordered warning card sits at the top of the Discovery section.
2. Click "Full retrain". The Start buttons disable; a "Stop" button appears at the end of the action row.
3. Click "Stop". Within ~1 second, the Latest run line shows `cancelled · <stage> · …`, the Stop button disappears, and the Start buttons re-enable.
4. Click "Incremental refresh" and let it run to completion. The Latest run line shows `completed`. The model's `Last trained` timestamp updates.
5. Inspect the database (`sqlite3 <db-path> "SELECT id, status, stage FROM training_runs ORDER BY id DESC LIMIT 5;"`). The cancelled run has `status = 'cancelled'` and the most recent completed run has `status = 'completed'`.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/routes/settings/+page.svelte
git commit -m "feat(frontend): Discovery Stop button + CPU/heat warning card"
```

---

## Done

After Task 6's manual smoke test passes:
- Backend: `discovery_train_cancel` flag exists, is reset on start, polled in the trainer hot loop and at every stage boundary, and the run is finalized as `"cancelled"` with no model activation.
- Frontend: Stop button appears only while a run is active; warning card is permanently visible at the top of the Discovery section.
- Tests: two new unit tests in `discovery_trainer::tests` pin the cancel-flag wiring.

The plan is complete.
