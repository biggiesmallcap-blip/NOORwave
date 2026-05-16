# Plan: True DASH segment seek (option C)

> Revision 7 (final). Four small cleanups on r6 - no architectural change,
> all minor: removed an undefined helper reference (`return_outcome`),
> dropped a duplicate paragraph, made the route's None-runtime branch
> explicit, and filled in `get_buffered_start_ms`'s body so the handle
> read pattern is unambiguous. Added one risk entry on serial command
> processing during transitions. Plan is decision-complete.
>
> --- r6 fixes (still apply) ---
>
> Three plan-quality fixes folded into r5 (no architectural
> change):
>
> A. **Move `evaluate_seek_decision` and `SeekDecision` out of `routes.rs`**
>    into the runtime module. r5 had the runtime consuming a helper that
>    lives in the server-routes file, which would create a wrong-direction
>    dependency (playback runtime depending on the HTTP layer). r6 puts the
>    pure helper in `noor-server/src/playback/runtime/mod.rs` (or a
>    runtime-private submodule); the route only consumes `SeekToOutcome`.
>
> B. **Fix the JSON shape in route examples.** `build_live_playback_snapshot`
>    returns `player::PlaybackSnapshot` (a struct with `.state` and `.queue`
>    - see routes.rs:5996-6002, 7430-7434, 7462). r5 examples wrote
>    `Json(json!({ "state": snapshot }))` which would double-wrap. Correct
>    shape: `Json(json!({ "state": snapshot.state }))`.
>
> C. **Document the borrow-checker pattern for the `SeekTo` handler.** Inside
>    `run_runtime_loop`, reading `state.engine.as_ref().shared` / `.job`
>    holds an immutable borrow of `state`. Calling `transition_to_job(state, ...)`
>    needs a mutable borrow. The handler must extract everything (clone
>    job, copy `offset_samples`, clone the resolved offsets if needed) into
>    locals BEFORE dropping the immutable borrow, then call
>    `transition_to_job` with the locals.
>
> --- r5 review (still applies) ---
>
> Folds in adversarial review of revision 4. Three blockers and
> one consistency issue:
>
> 1. **Unreachable on the UI** - `NowPlayingProgress` still clamps `scrubMax`
>    to `bufferedMs`, so the user physically can't drag past the buffered
>    region. The whole feature was reachable only by a stale client
>    bypassing the clamp. Fix: drop the forward clamp, let the user drag
>    to anywhere in `[0, duration]`. The buffered bar stays as a visual
>    cue. Backend's `SeekTo` decides what to do.
>
> 2. **Decoded range after segment-restart is `[offset, buffered]`, not
>    `[0, buffered]`.** The fast-path `target <= buffered` would accept a
>    backward seek that's BEFORE the current engine's offset, then the CPAL
>    callback's `saturating_sub` would clamp to buffer start, playing the
>    wrong audio while reporting the wrong position. Fix: fast-path needs
>    BOTH bounds (`offset <= target <= buffered`); anything outside takes
>    the segment-seek transition or rejection path.
>
> 3. **Legacy route path becomes incorrect once offsets exist.** A legacy
>    client seeking before the current segment offset would not be rejected
>    by `evaluate_seek_decision` (which only checks the upper bound), but
>    the in-buffer seek would land on garbage. Fix: route everything through
>    `SeekTo`. Don't keep two seek paths. The flag `allow_segment_seek`
>    becomes an input to the runtime's decision, not a route-level
>    bifurcation.
>
> 4. **Consistency: when does `Dispatched` get sent?** r4 said both
>    "before priming completes" and "after transition_to_job returns Ok".
>    Pick one. Revision 5: reply AFTER `transition_to_job` returns. Add
>    `SeekToOutcome::Failed` so transition errors propagate.

## Baseline (master @ 4b87f98)

(Same as revision 4 - all the r4 baseline facts still hold.)

Additional baseline relevant to r5 fixes:

- [`NowPlayingProgress.svelte:49-53`](frontend/src/lib/components/now-playing/NowPlayingProgress.svelte:49): `scrubMax` is `duration > 0 ? Math.min(duration, Math.max(scrubPosition, position, bufferedMs)) : 0`. The forward clamp to `bufferedMs` is what we shipped in #43 to prevent past-buffer drags. With option C, the backend handles past-buffer correctly, so this clamp is now what's BLOCKING the feature.
- [`evaluate_seek_decision`](noor-server/src/server/routes.rs:7481): checks only `target_samples > buffered_samples`. No lower bound. Correct as long as `offset == 0`; incorrect once segment-seek creates engines with non-zero offsets.

## Ownership model

Same as revisions 3-4:
- `PreparedPlaybackJob.{start_from_segment_index, start_from_offset_ms}` carries start params.
- `PlaybackSharedState.segment_offsets_ms: OnceLock<Vec<u64>>` populated by `decode_and_buffer_job`.
- `PlaybackEngine.job: PreparedPlaybackJob` cached at construction.
- `position_samples` and `buffered_samples` are ABSOLUTE-track samples.

New for r5:
- Route is the dumb dispatcher; the runtime's `SeekTo` handler is the single decision point. `evaluate_seek_decision` moves out of the route entirely (or is repurposed as a pure helper called from the runtime's handler).

## Approach

### 1. Surface segment timing in `StreamInfo`

(Unchanged from r4.) `parse_dash_segment_template` populates `pub segment_offsets_ms: Vec<u64>` on `StreamInfo` from `current_time * 1000 / timescale` in both parser branches.

### 2. Shared state: `OnceLock` for offsets, atomic for position offset

(Unchanged from r4.) `PlaybackSharedState` gains:
- `pub(crate) segment_offsets_ms: std::sync::OnceLock<Vec<u64>>`
- `pub(crate) position_offset_samples: AtomicU64`

`PlaybackSharedState::new` accepts the initial `position_offset_samples` value, seeded by the caller from `job.start_from_offset_ms`.

### 3. Job carries start params; engine caches job

(Unchanged from r4.) `PreparedPlaybackJob` gains `start_from_segment_index: usize` and `start_from_offset_ms: u64`. `PlaybackEngine` gains `pub(super) job: PreparedPlaybackJob`. `#[cfg(test)] PreparedPlaybackJob::test_fixture(track_id, generation)` exists for `test_with_shared`.

### 4. Decode path slicing

(Unchanged from r4.) `decode_and_buffer_job` slices `segment_urls.iter().skip(start_from_segment_index)` BEFORE computing `dash_initial_media_count`. `shared.segment_offsets_ms.set(stream_info.segment_offsets_ms.clone()).ok()` is called immediately after `resolve_stream()` returns.

### 5. Absolute-sample accounting

(Mostly unchanged from r4.) Quick recap:
- `position_samples.store(offset_samples, ...)` at [mod.rs:1043](noor-server/src/playback/runtime/mod.rs:1043), with `offset_samples` computed inline from `job.start_from_offset_ms`.
- CPAL callback's `seek_target_samples` is absolute; computes `local = abs.saturating_sub(offset)` before `guard.seek_to(local)`. **New for r5:** before issuing the local seek, also verify `abs >= offset`. If `abs < offset`, the seek is out-of-buffer and should be rejected (the CPAL callback's `saturating_sub` clamp would mask the error otherwise). Emit the existing "seek target is not decoded yet" WARN with a slightly different reason note. This is defense-in-depth - the runtime's `SeekTo` handler should already have rejected before writing `seek_target_samples`, but keeping the callback honest is cheap.
- `publish_buffered_samples(offset + guard.samples.len())` (absolute upper bound).

### 6. Add absolute lower bound to the live snapshot

Add `pub buffered_start_ms: i64` to `PlaybackState` ([db/models.rs](noor-server/src/db/models.rs)), `#[serde(default)]` for backwards-compat. Populated in `build_live_playback_snapshot` ([routes.rs:5951](noor-server/src/server/routes.rs:5951)) from a new handle method:

```rust
// On PlaybackRuntimeHandle, mirroring get_buffered_ms:
pub fn get_buffered_start_ms(&self, device_sample_rate: u32, device_channels: u16) -> i64 {
    if device_sample_rate == 0 || device_channels == 0 { return 0; }
    let samples = self.offset_source.lock().unwrap().load(Ordering::Relaxed);
    (samples * 1000 / (device_sample_rate as u64 * device_channels as u64)) as i64
}
```

Caller in `build_live_playback_snapshot` reads rate/channels from `state.playback_runtime_info` exactly like the existing `get_buffered_ms` overlay at [routes.rs:5963](noor-server/src/server/routes.rs:5963). Same pattern, no new infra.

Tradeoff acknowledgment: this DOES add a third redirect mirror, which r3/r4 deliberately avoided. But the alternatives (compute on-demand by reading the active engine's shared state through a separate path, or fold offset into the snapshot helper directly) all break the redirect-after-promote invariant that #43 carefully established. Adding `offset_source: Arc<Mutex<Arc<AtomicU64>>>` and redirecting it alongside the existing two is the consistent move.

Update `evaluate_seek_decision` ([routes.rs:7481](noor-server/src/server/routes.rs:7481)) to take both bounds and reject either way - **but** in r5 the route doesn't call it directly anymore (see step 9). Keep the helper as a pure function (it's still useful for the runtime's `SeekTo` handler):

```rust
pub(super) fn evaluate_seek_decision(
    target_samples: u64,
    buffered_start_samples: u64,
    buffered_samples: u64,
    runtime_active: bool,
) -> SeekDecision {
    if !runtime_active { return SeekDecision::Dispatch; }
    if buffered_samples == 0 { return SeekDecision::Dispatch; }
    if target_samples < buffered_start_samples || target_samples > buffered_samples {
        SeekDecision::RejectOutOfBuffer
    } else {
        SeekDecision::Dispatch
    }
}
```

(`RejectPastBuffer` renames to `RejectOutOfBuffer` to reflect the two-sided check.)

### 7. Runtime command + outcome enum

```rust
pub enum PlaybackRuntimeCommand {
    // ...existing variants...
    SeekTo {
        target_ms: i64,
        allow_segment_seek: bool,
        respond_to: std::sync::mpsc::Sender<SeekToOutcome>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekToOutcome {
    Dispatched,
    RejectedOutOfBuffer,
    Failed,  // transition_to_job errored
}
```

Note: `allow_segment_seek` is now part of the command, not a route-level switch. The runtime applies the right semantics. `mpsc::Sender` keeps `PlaybackRuntimeCommand: Clone` (same precedent as `TrackStatus`).

Handler in `run_runtime_loop` (borrow-check aware - see r6 fix C):

```rust
// Phase 1: snapshot everything we need under an immutable borrow.
let decision = {
    let Some(engine) = state.engine.as_ref() else {
        respond_to.send(SeekToOutcome::RejectedOutOfBuffer).ok();
        return;
    };
    let target_samples = (target_ms.max(0) as u64) * rate as u64 * channels as u64 / 1000;
    let offset_samples = engine.shared.position_offset_samples.load(Ordering::Relaxed);
    let buffered_samples = engine.shared.buffered_samples.load(Ordering::Relaxed);

    // Pure-helper call (lives in runtime module per r6 fix A).
    match evaluate_seek_decision(target_samples, offset_samples, buffered_samples, true) {
        SeekDecision::Dispatch => SeekHandling::InBuffer { target_samples },
        SeekDecision::RejectOutOfBuffer if !allow_segment_seek => SeekHandling::Reject,
        SeekDecision::RejectOutOfBuffer => {
            let Some(offsets) = engine.shared.segment_offsets_ms.get() else {
                respond_to.send(SeekToOutcome::RejectedOutOfBuffer).ok();
                return;
            };
            if offsets.is_empty() {
                respond_to.send(SeekToOutcome::RejectedOutOfBuffer).ok();
                return;
            }
            // Find largest N such that offsets[N] in samples <= target_samples.
            let n = offsets
                .iter()
                .rposition(|off_ms| (*off_ms * rate as u64 * channels as u64 / 1000) <= target_samples)
                .unwrap_or(0);
            let new_offset_ms = offsets[n];
            // Clone job so the borrow ends with this scope.
            let new_job = {
                let mut j = engine.job.clone();
                j.start_from_segment_index = n;
                j.start_from_offset_ms = new_offset_ms;
                j
            };
            SeekHandling::SegmentSeek { job: new_job }
        }
    }
}; // immutable borrow of state.engine ends here

// Phase 2: act on the decision under a mutable borrow.
match decision {
    SeekHandling::InBuffer { target_samples } => {
        state.engine.as_ref().unwrap().shared
            .seek_target_samples.store(target_samples, Ordering::Relaxed);
        respond_to.send(SeekToOutcome::Dispatched).ok();
    }
    SeekHandling::Reject => {
        respond_to.send(SeekToOutcome::RejectedOutOfBuffer).ok();
    }
    SeekHandling::SegmentSeek { job } => {
        match transition_to_job(
            config, &command_tx, device, output_config, output_sample_format,
            event_tx, state, job, volume_ctl, position_samples,
            position_source, buffered_source, /* force_restart */ true,
        ) {
            Ok(()) => { respond_to.send(SeekToOutcome::Dispatched).ok(); }
            Err(_) => { respond_to.send(SeekToOutcome::Failed).ok(); }
        }
    }
}
```

Reply confirms the runtime ACCEPTED the seek and (for segment-seek) finished promoting the new engine. It does NOT confirm audible playback - the new engine still primes samples after `transition_to_job` returns. Frontend's 1 Hz refresher converges within ~1s.

`rate` and `channels` for the `target_samples` math come from `state.device_sample_rate` plus the engine's stream config; the existing `transition_to_job` already passes these around. Read from whichever local is in scope at the handler call site (confirm at execution time - the `run_runtime_loop` body already has both for other reasons).

### 8. Public handle method

```rust
/// Segment-aware seek. Single entry point for all seek requests; the runtime
/// decides between in-buffer fast path, forced-restart segment-seek, or
/// rejection. The `allow_segment_seek` flag opts in to the segment-seek
/// transition; with `false`, the runtime treats out-of-buffer seeks as
/// rejected (legacy semantics).
///
/// Blocks up to 1500ms for the reply (segment-seek transitions can take
/// >250ms when the new engine has to spin up the decoder thread).
/// Returns Failed on timeout (treat as recoverable error; frontend retries).
pub fn seek_to_segment_aware(
    &self,
    position_ms: i64,
    allow_segment_seek: bool,
) -> SeekToOutcome {
    let (tx, rx) = std::sync::mpsc::channel();
    if self
        .send(PlaybackRuntimeCommand::SeekTo {
            target_ms: position_ms,
            allow_segment_seek,
            respond_to: tx,
        })
        .is_err()
    {
        return SeekToOutcome::Failed;
    }
    rx.recv_timeout(std::time::Duration::from_millis(1500))
        .unwrap_or(SeekToOutcome::Failed)
}
```

Timeout bumped from 250ms (r4) to 1500ms because the reply now waits for `transition_to_job` to return, which includes engine teardown + new engine spin-up. The fast path replies immediately (well under 1500ms).

Keep the legacy `pub fn seek(&self, position_ms: i64) -> Result<()>` as a thin wrapper for callers that don't care about the outcome detail:

```rust
pub fn seek(&self, position_ms: i64) -> Result<()> {
    match self.seek_to_segment_aware(position_ms, /* allow_segment_seek */ false) {
        SeekToOutcome::Dispatched => Ok(()),
        SeekToOutcome::RejectedOutOfBuffer => Err(anyhow!("seek target is out of buffer")),
        SeekToOutcome::Failed => Err(anyhow!("seek dispatch failed")),
    }
}
```

This way the old `Seek(i64)` command variant disappears - one less seek path in the codebase. Audit existing callers of `handle.seek(...)` and confirm they're OK with the slightly different error shape.

### 9. Route handler - one path, both flags

`POST /api/playback/position`:

```rust
#[derive(Deserialize)]
struct SetPlaybackPositionPayload {
    position_ms: i64,
    #[serde(default)]
    allow_segment_seek: bool,
}

async fn set_playback_position(...) -> Result<(StatusCode, Json<Value>), StatusCode> {
    // `playback_runtime` is Option (None on pre-first-play boot). Treat
    // no-runtime the same as the existing route (silent OK with current
    // snapshot); a seek with no runtime is a UI race we don't need to fail.
    let handle = {
        let g = state.read().await;
        g.playback_runtime.as_ref().map(|rt| rt.handle.clone())
    };
    let outcome = match handle {
        Some(handle) => {
            let allow = payload.allow_segment_seek;
            let pos = payload.position_ms;
            tokio::task::spawn_blocking(move || handle.seek_to_segment_aware(pos, allow))
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        None => SeekToOutcome::RejectedOutOfBuffer,
    };

    let snapshot = build_live_playback_snapshot(&state).await?;

    // r6 fix B: snapshot is PlaybackSnapshot; the JSON body shape is
    // { "state": PlaybackState } - serialize snapshot.state, not snapshot.
    match outcome {
        SeekToOutcome::Dispatched => Ok((
            StatusCode::ACCEPTED,
            Json(json!({ "state": snapshot.state })),
        )),
        SeekToOutcome::RejectedOutOfBuffer => Ok((
            StatusCode::CONFLICT,
            Json(json!({ "state": snapshot.state })),
        )),
        SeekToOutcome::Failed => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
```

`tokio::task::spawn_blocking` wraps the sync `recv_timeout` so it doesn't park an async executor thread. No DB write either way; `build_live_playback_snapshot` already overlays the runtime's live position.

The route no longer calls `evaluate_seek_decision` directly - the runtime owns the helper now (r6 fix A: the pure helper and `SeekDecision` enum live in `noor-server/src/playback/runtime/mod.rs`, not in `noor-server/src/server/routes.rs`). The runtime module exports `SeekToOutcome` publicly; the route consumes only that.

### 10. Frontend: drop the forward clamp

Update `NowPlayingProgress.svelte` ([49-53](frontend/src/lib/components/now-playing/NowPlayingProgress.svelte:49)):

```typescript
let scrubMax = $derived(duration > 0 ? duration : 0);
```

The buffered fill div (`np-progress-buffered`) keeps its current size (`(bufferedMs / duration) * 100%`) as a visual cue showing the user what's currently decoded. The dragging range is full track.

Optional polish: render `buffered_start_ms` as a secondary fill marker (e.g., a thin vertical line on the buffered bar) so the user can see "decoded range starts here." Skip unless smoke testing shows users get confused.

`setPlayerPosition` in [`player.ts:514`](frontend/src/lib/stores/player.ts:514) always sends `allow_segment_seek: true`. The existing 409 catch path stays for the pre-resolve / no-DASH-offsets fallback. New: handle the 500 status (treat as a recoverable error, show toast, allow retry - same shape as the existing 409 catch but with a different message).

### 11. Cancel-in-flight

(Unchanged from r4.) `transition_to_job` tears down the prior engine. Three rapid `SeekTo`s produce three transitions; only the last engine survives. Each call gets its own reply channel.

Edge case: rapid SeekTo while one is in flight inside `transition_to_job`. The runtime processes commands serially in `run_runtime_loop`, so a second SeekTo waits until the first transition completes. With 1500ms timeout, this is fine for normal user interaction; for spammed seeks, the second one waits and may see the new (post-transition) state. Acceptable.

### 12. Tests

Backend:
- (r4 list, plus the following.)
- **Lower-bound check**: build an engine with offset = 30000ms. Send `SeekTo` with `target_ms = 10000` (before offset). Assert reply is `RejectedOutOfBuffer` if `allow_segment_seek=false`, `Dispatched` (with transition) if `allow_segment_seek=true`.
- **Backward seek into in-buffer range**: with offset = 30000ms and buffered up to 50000ms, send `SeekTo(target_ms=40000)`. Fast path applies; no transition.
- **Pure helper**: unit test `evaluate_seek_decision(target, start, buffered, active)` with all combinations of inside/below-start/above-buffered/inactive.
- **Failed outcome**: mock `transition_to_job` to return Err; assert reply is `Failed`.
- **CPAL out-of-buffer rejection**: write `seek_target_samples = (offset - 1)` (below offset); assert the buffer's `read_pos` stays unchanged and the WARN fires.
- **handle.seek() wrapper**: confirm legacy callers see `Result<()>` and the right error variant.

Frontend:
- (r4 list, plus the following.)
- **Scrubber unclamped**: render `NowPlayingProgress` with `bufferedMs = 5000, duration = 100000, position = 1000`; assert the `<input>`'s `max` attribute is `100000`, not `5000`.
- **500 handling**: mock the route to return 500; assert `setPlayerPosition` sets `playerError` and the user can retry.

## Files to modify

- [noor-server/src/services/tidal/stream.rs](noor-server/src/services/tidal/stream.rs) - `StreamInfo.segment_offsets_ms`.
- [noor-server/src/playback/decode/mod.rs](noor-server/src/playback/decode/mod.rs) - slice segments by `start_from_segment_index` for BOTH prebuffer count and remainder; publish offsets to shared state.
- [noor-server/src/playback/runtime/shared.rs](noor-server/src/playback/runtime/shared.rs) - `OnceLock<Vec<u64>>` for offsets, `position_offset_samples` atomic, absolute-sample accounting in CPAL callback (with the new `< offset` rejection check).
- [noor-server/src/playback/runtime/engine.rs](noor-server/src/playback/runtime/engine.rs) - `PlaybackEngine.job`; `test_with_shared` uses `PreparedPlaybackJob::test_fixture`.
- [noor-server/src/playback/runtime/commands.rs](noor-server/src/playback/runtime/commands.rs) - `PlaybackRuntimeCommand::SeekTo { target_ms, allow_segment_seek, respond_to: mpsc::Sender<SeekToOutcome> }`; `SeekToOutcome::{Dispatched, RejectedOutOfBuffer, Failed}`. Remove the old `Seek(i64)` variant (or keep transitionally - audit and decide).
- [noor-server/src/playback/runtime/mod.rs](noor-server/src/playback/runtime/mod.rs) - `pub fn seek_to_segment_aware(...)` and the wrapper `pub fn seek(...)`; new `offset_source: Arc<Mutex<Arc<AtomicU64>>>` redirected at the same 4 sites as `position_source`/`buffered_source`; `pub fn get_buffered_start_ms(...)`; `SeekTo` handler in `run_runtime_loop` doing the in-buffer / segment-seek / reject decision and replying after `transition_to_job` returns (see r6 borrow-check pattern in §7); `transition_to_job` seeds `offset_samples` from `job.start_from_offset_ms` and `engine.job = job.clone()`. **Move `evaluate_seek_decision` and `SeekDecision` here from `server/routes.rs`** (r6 fix A) - extend the signature to take both bounds (`buffered_start_samples`, `buffered_samples`), rename rejection variant to `RejectOutOfBuffer`, and update the runtime-internal unit tests at routes.rs:11572-11611 to live alongside (the existing tests can come along to runtime tests with minor adjustments).
- [noor-server/src/playback/player.rs](noor-server/src/playback/player.rs) - `PreparedPlaybackJob` start fields; `test_fixture`.
- [noor-server/src/db/models.rs](noor-server/src/db/models.rs) - `PlaybackState.buffered_start_ms: i64` with `#[serde(default)]`.
- [noor-server/src/server/routes.rs](noor-server/src/server/routes.rs) - `SetPlaybackPositionPayload.allow_segment_seek: bool`; route always dispatches via `seek_to_segment_aware` and consumes `SeekToOutcome` only; `build_live_playback_snapshot` populates `buffered_start_ms` from `handle.get_buffered_start_ms(...)`. **Remove the in-file `evaluate_seek_decision`, `SeekDecision`, `evaluate_seek_against_buffer` and their unit tests** (moved to runtime per r6 fix A). The route's response body for 202/409 uses `snapshot.state`, not `snapshot` (r6 fix B - `build_live_playback_snapshot` returns `PlaybackSnapshot`, not bare `PlaybackState`; see existing routes.rs:5999/7433/7462 for the pattern).
- [frontend/src/lib/api/client.ts](frontend/src/lib/api/client.ts) - request body type gains `allow_segment_seek?: boolean`; `PlaybackState` TS type gains `buffered_start_ms?: number`.
- [frontend/src/lib/stores/player.ts](frontend/src/lib/stores/player.ts) - `setPlayerPosition` always sends `allow_segment_seek: true`; handle 500 with error toast; `buffered_start` writable populated from state if we want the secondary fill marker.
- [frontend/src/lib/components/now-playing/NowPlayingProgress.svelte](frontend/src/lib/components/now-playing/NowPlayingProgress.svelte) - `scrubMax = duration > 0 ? duration : 0` (drop the forward clamp). Optional: secondary fill marker for `buffered_start_ms`.
- Test files.

## Verification

End-to-end smoke (after build):
1. Play a TIDAL track. Within ~1.5s (post-resolve), drag scrubber to 75%. Expected: brief audio gap (<1.5s), playback resumes at ~75%. Buffered bar visually resets. **The scrubber should actually reach 75%** (this is the r5 fix verifying).
2. Seek immediately at track start (pre-resolve). Expected: 409 returned, scrubber snaps back, frontend handles via existing 409 catch. User retries; step 1 works.
3. After segment-restart at 75%, drag scrubber BACK to 20%. Expected: target is below the current engine's offset, runtime takes the segment-seek path again, new engine starts at ~20%.
4. After segment-restart at 75%, drag scrubber to 78% (small forward inside the current buffer). Expected: fast-path applies, no transition, audio jumps within ~50ms.
5. Rapid-fire seek (50% → 20% → 80% within ~500ms). Expected: each POST gets a reply (202 or 409). Only the last engine survives. No audio glitches beyond the single audible gap at the final seek.
6. Local-file track: `segment_offsets_ms` empty; past-buffer seek returns 409, frontend catches. Forward seek within buffered region works as today.
7. Tail noor-server.log during step 1: zero "seek target is not decoded yet" WARN lines from user seeks (runtime intercepts them).
8. Mid-seek, fire Stop. Expected: clean tear-down. Pending `SeekTo` reply gets dropped (Receiver returns `RecvError`); route returns 500 (treat as recoverable; frontend toast). No orphan decoder.

Tests:
```powershell
cargo test -p noor-server services::tidal::stream
cargo test -p noor-server playback::runtime
cargo test -p noor-server server::routes
cargo check -p noor-server
cargo fmt --all -- --check
cd frontend; npm run check; npm test
```

## Risks and open questions

- **Adding a third redirect (`offset_source`)** restores some of the complexity #43 minimized. It's worth it for r5's lower-bound check on the route side; the alternative (no `buffered_start_ms` exposed) means the route can't safely handle legacy clients seeking around a non-zero offset.
- **`recv_timeout(1500ms)` plus `spawn_blocking`** means at most one async worker is parked per concurrent seek. Acceptable for a UI-driven endpoint (1 user, 1 seek at a time).
- **Legacy `handle.seek()` wrapper** changes error shape slightly. Audit `git grep "\.seek(" noor-server/src/` for external callers and confirm none care about the specific error variant.
- **Removing the `Seek(i64)` command variant** vs keeping it transitionally: simpler to remove. If audit reveals a non-route caller that genuinely needs the old behavior, keep `Seek` as a thin wrapper that dispatches `SeekTo { allow_segment_seek: false }`.
- **Pre-resolve seeks return 409.** First ~half-second of a fresh track, OnceLock unset. Frontend's existing 409 catch handles it.
- **Init-segment URL expiry**, **TIDAL rate-limit**, **decoder teardown race**, **buffer mutex during restart**: same as r4.
- **Serial runtime-loop processing**: a `transition_to_job` in flight blocks ALL other commands (Pause, Stop, Volume changes, even another SeekTo) until it returns. This is pre-existing behavior (Switch / Next / Prev have the same shape), but option C makes seeks a new source of multi-hundred-ms commands. If smoke testing exposes pause-during-seek lag, the follow-up is to make `transition_to_job` yield via a state machine inside the loop rather than block synchronously. Out of scope for this plan.

## Out of scope (explicit)

- Pre-buffering ahead of the playhead beyond what segments provide.
- WebSocket "seek-pending" event.
- HLS video seek.
- Gapless seek via dual-engine crossfade.
- Runtime-driven DB writes for live position (hibernation accuracy).
